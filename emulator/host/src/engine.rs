//! QEMU process lifecycle and inherited IPC endpoints; no GUI dependencies.
use crate::{
    input::{EngineInput, InputHub},
    qmp::QmpClient,
    shared_memory::SharedRam,
};
use std::{
    env,
    ffi::OsString,
    fs::{self, File},
    io,
    os::{
        fd::AsRawFd,
        unix::{net::UnixStream, process::CommandExt},
    },
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::Arc,
    time::Duration,
};

#[derive(Clone)]
pub struct Options {
    pub qemu: Option<PathBuf>,
    pub firmware_dir: PathBuf,
    pub logs: PathBuf,
    pub sd_image: Option<PathBuf>,
    pub no_sd: bool,
    pub state_dir: Option<PathBuf>,
    pub trace_sd: bool,
    pub usb_enabled: bool,
    pub host_midi: bool,
    pub timing: Timing,
    pub ui_input_smoke: bool,
    pub update_bootloader: bool,
    /// Optional local GDB socket for deterministic firmware diagnostics.
    pub gdb_socket: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug)]
pub enum Timing {
    /// 8 ns per instruction, with QEMU sleeping when ahead of wall time.
    Paced,
    /// QEMU continuously adjusts the instruction/time ratio.
    Adaptive,
}

impl Timing {
    fn icount(self) -> &'static str {
        match self {
            Self::Paced => "shift=3,align=on,sleep=on",
            Self::Adaptive => "shift=auto,sleep=on",
        }
    }
}

fn take_path(args: &mut impl Iterator<Item = OsString>, flag: &str) -> Result<PathBuf, String> {
    args.next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("{flag} requires a path"))
}

impl Options {
    pub fn parse() -> Result<Self, String> {
        Self::parse_args(env::args_os().skip(1))
    }
    pub fn parse_args(arguments: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let workspace = crate::paths::repository_root();
        let mut options = Self {
            qemu: env::var_os("L6_QEMU").map(PathBuf::from),
            firmware_dir: workspace.join("unpacked"),
            logs: workspace.join("emulator/logs"),
            sd_image: None,
            no_sd: false,
            state_dir: Some(workspace.join("emulator/state")),
            trace_sd: false,
            usb_enabled: false,
            host_midi: false,
            timing: Timing::Paced,
            ui_input_smoke: false,
            update_bootloader: false,
            gdb_socket: None,
        };
        let mut args = arguments.into_iter();
        while let Some(arg) = args.next() {
            match arg.to_string_lossy().as_ref() {
                "--ui-input-smoke" => options.ui_input_smoke = true,
                "--update-bootloader" => options.update_bootloader = true,
                "--qemu" => options.qemu = Some(take_path(&mut args, "--qemu")?),
                "--firmware-dir" => options.firmware_dir = take_path(&mut args, "--firmware-dir")?,
                "--logs" => options.logs = take_path(&mut args, "--logs")?,
                "--gdb-socket" => options.gdb_socket = Some(take_path(&mut args, "--gdb-socket")?),
                "--usb" => {
                    options.usb_enabled = true;
                    options.host_midi = true;
                }
                "--trace-sd" => options.trace_sd = true,
                "--sd-image" => options.sd_image = Some(take_path(&mut args, "--sd-image")?),
                "--no-sd" => options.no_sd = true,
                "--state-dir" => options.state_dir = Some(take_path(&mut args, "--state-dir")?),
                "--volatile" => options.state_dir = None,
                "--timing" => {
                    options.timing = match args.next().as_deref().and_then(|value| value.to_str()) {
                        Some("paced") => Timing::Paced,
                        Some("adaptive") => Timing::Adaptive,
                        _ => return Err("--timing requires paced or adaptive".into()),
                    };
                }
                "--help" | "-h" => {
                    println!(
                        "Engine options: [QEMU] [--qemu PATH] [--firmware-dir DIR] [--logs DIR] [--state-dir DIR | --volatile] [--sd-image FILE | --no-sd] [--update-bootloader] [--usb] [--trace-sd] [--timing paced|adaptive] [--ui-input-smoke] [--gdb-socket PATH]"
                    );
                    std::process::exit(0);
                }
                value if value.starts_with('-') => return Err(format!("unknown option: {value}")),
                _ if options.qemu.is_none() => options.qemu = Some(PathBuf::from(arg)),
                _ => return Err(format!("unexpected argument: {}", arg.to_string_lossy())),
            }
        }
        if options.no_sd && options.sd_image.is_some() {
            return Err("--no-sd conflicts with --sd-image".into());
        }
        if options.qemu.is_none() {
            let default = PathBuf::from("/private/tmp/l6-qemu-src/build/qemu-system-arm");
            if default.is_file() {
                options.qemu = Some(default);
            }
        }
        Ok(options)
    }
}

pub struct Engine {
    children: Vec<Child>,
    pub inputs: Option<Arc<InputHub>>,
    pub management: Vec<QmpClient>,
    pub display_memory: Option<SharedRam>,
    pub usb: Option<Arc<crate::usb::UsbHost>>,
    #[cfg(target_os = "macos")]
    pub midi: Option<crate::usb_midi::MidiBridge>,
}

impl Engine {
    pub fn start(options: &Options) -> Result<Self, String> {
        let mut engine = Self {
            children: Vec::new(),
            inputs: None,
            management: Vec::new(),
            display_memory: None,
            usb: None,
            #[cfg(target_os = "macos")]
            midi: None,
        };
        let Some(qemu) = &options.qemu else {
            return Ok(engine);
        };
        let qemu = qemu
            .canonicalize()
            .map_err(|e| format!("cannot find QEMU: {e}"))?;
        fs::create_dir_all(&options.logs).map_err(|e| e.to_string())?;
        let logs = options.logs.canonicalize().map_err(|e| e.to_string())?;
        let firmware = options
            .firmware_dir
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let sd = options
            .sd_image
            .as_ref()
            .map(|p| p.canonicalize())
            .transpose()
            .map_err(|e| format!("cannot open SD image: {e}"))?;
        for image in ["main_firmware.bin", "secondary_firmware.bin"] {
            if !firmware.join(image).is_file() {
                return Err(format!("missing firmware: {image}"));
            }
        }
        if options.update_bootloader {
            let state = options
                .state_dir
                .as_ref()
                .ok_or("--update-bootloader requires persistent --state-dir storage")?;
            let outcome = crate::update_bootloader::install(state, sd.as_deref())
                .map_err(|e| format!("replacement update bootloader: {e}"))?;
            fs::write(logs.join("update-bootloader.log"), format!("{outcome:?}\n"))
                .map_err(|e| e.to_string())?;
        }
        engine.display_memory = Some(
            SharedRam::new(crate::display::DISPLAY_BYTES)
                .map_err(|e| format!("create anonymous display memory: {e}"))?,
        );
        let display_fd = engine.display_memory.as_ref().unwrap().fd();
        let (main_input, main_input_guest) = EngineInput::pair().map_err(|e| e.to_string())?;
        let (panel_input, panel_input_guest) = EngineInput::pair().map_err(|e| e.to_string())?;
        let (qmp_parent, qmp_guest) = UnixStream::pair().map_err(|e| e.to_string())?;
        let (usb_host, usb_guest) = UnixStream::pair().map_err(|e| e.to_string())?;
        let mut fds = vec![
            main_input_guest.as_raw_fd(),
            panel_input_guest.as_raw_fd(),
            qmp_guest.as_raw_fd(),
            display_fd,
        ];
        if options.usb_enabled {
            fds.push(usb_guest.as_raw_fd());
        }
        let mut command = Command::new(&qemu);
        command
            .args([
                "-M",
                "l6max-dual",
                "-accel",
                "tcg,thread=single",
                "-icount",
                options.timing.icount(),
                "-smp",
                "2",
                "-kernel",
            ])
            .arg(firmware.join("main_firmware.bin"))
            .args([
                "-S",
                "-no-shutdown",
                "-display",
                "none",
                "-monitor",
                "none",
                "-serial",
                "none",
                "-chardev",
            ])
            .arg(format!("socket,id=management,fd={}", qmp_guest.as_raw_fd()))
            .args([
                "-object",
                "monitor-qmp,id=management-monitor,chardev=management",
                "-d",
                "guest_errors,unimp",
                "-D",
            ])
            .arg(logs.join("engine.trace"))
            .current_dir(&logs)
            .env("L6_PANEL_FIRMWARE", firmware.join("secondary_firmware.bin"))
            .env("L6_MAIN_INPUT_FD", main_input_guest.as_raw_fd().to_string())
            .env(
                "L6_PANEL_INPUT_FD",
                panel_input_guest.as_raw_fd().to_string(),
            )
            .env("L6_DISPLAY_FD", display_fd.to_string())
            .env_remove("L6_EXTERNAL_RAM_FD")
            .env("L6_SERVICE_AUDIO_QUEUE", "1")
            .env_remove("L6_INPUT_FD")
            .env_remove("L6_BUTTON_CONTROL_FILE")
            .env_remove("L6_EXTERNAL_RAM_FILE")
            .env_remove("L6_PANEL_KEYS")
            .env_remove("L6_MAIN_KEYS")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(
                File::create(logs.join("engine.stderr")).map_err(|e| e.to_string())?,
            ));
        if let Some(socket) = &options.gdb_socket {
            command
                .arg("-gdb")
                .arg(format!("unix:{},server=on,wait=off", socket.display()));
        }
        if let Some(sd) = &sd {
            command
                .arg("-drive")
                .arg(format!("if=sd,format=raw,id=l6-sd,file={}", sd.display()));
        }
        command.env_remove("L6_USB_FD");
        command.env_remove("L6_STATE_DIR");
        if let Some(directory) = &options.state_dir {
            fs::create_dir_all(directory).map_err(|e| e.to_string())?;
            command.env(
                "L6_STATE_DIR",
                directory.canonicalize().map_err(|e| e.to_string())?,
            );
        }
        if options.usb_enabled {
            command.env("L6_USB_FD", usb_guest.as_raw_fd().to_string());
        }
        if options.trace_sd {
            command.args(["-trace", "sdhci_*"]);
        }
        if options.usb_enabled {
            engine.usb = Some(Arc::new(
                crate::usb::UsbHost::new(usb_host).map_err(|e| e.to_string())?,
            ));
        }
        // Only the selected IPC and display descriptors survive exec.
        unsafe {
            command.pre_exec(move || {
                for &fd in &fds {
                    let flags = libc::fcntl(fd, libc::F_GETFD);
                    if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        engine.children.push(
            command
                .spawn()
                .map_err(|e| format!("start QEMU engine:{e}"))?,
        );
        engine
            .management
            .push(QmpClient::connect(qmp_parent).map_err(|e| {
                let detail = fs::read_to_string(logs.join("engine.stderr")).unwrap_or_default();
                format!("QMP handshake:{e}; QEMU: {detail}")
            })?);
        if !engine.connected() {
            return Err("a firmware process exited during startup".into());
        }
        for management in &engine.management {
            management
                .resume()
                .map_err(|e| format!("resume guest: {e}"))?;
        }
        engine.inputs = Some(Arc::new(InputHub::new(main_input, panel_input)));
        #[cfg(target_os = "macos")]
        if options.host_midi {
            engine.midi = Some(
                crate::usb_midi::MidiBridge::start(engine.usb.as_ref().unwrap().clone())
                    .map_err(|e| e.to_string())?,
            );
        }
        Ok(engine)
    }
    pub fn connected(&mut self) -> bool {
        !self.children.is_empty()
            && self
                .children
                .iter_mut()
                .all(|child| matches!(child.try_wait(), Ok(None)))
    }

    pub fn process_id(&self) -> Option<u32> {
        self.children.first().map(Child::id)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        drop(self.midi.take());
        for child in &mut self.children {
            if matches!(child.try_wait(), Ok(None)) {
                unsafe {
                    libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
                }
            }
        }
        // One shared deadline avoids waiting separately for each process.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline
            && self
                .children
                .iter_mut()
                .any(|c| matches!(c.try_wait(), Ok(None)))
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        for child in &mut self.children {
            if !matches!(child.try_wait(), Ok(Some(_))) {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
}
