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
    /// Optional QEMU `-audiodev` specification, including its `id=` field.
    pub audio_device: Option<String>,
    pub firmware_dir: PathBuf,
    /// An L6 or L6max update package. When `firmware_dir` has no unpacked
    /// images, this (or a package found by [`Options::prepare`]) is unpacked.
    pub firmware_package: Option<PathBuf>,
    /// Optional target package used by the patched-update smoke lifecycle.
    /// The running firmware still comes from `firmware_dir`/`firmware_package`.
    pub update_package: Option<PathBuf>,
    /// Build QEMU and unpack firmware on first use. Packaged apps ship both and
    /// turn this off.
    pub auto_setup: bool,
    pub logs: PathBuf,
    pub sd_image: Option<PathBuf>,
    pub no_sd: bool,
    pub state_dir: Option<PathBuf>,
    pub trace_sd: bool,
    /// Connect the USB cable (and its 3 MIDI ports) as soon as the engine
    /// starts, rather than waiting for a later [`Engine::connect_usb`] call.
    pub usb_enabled: bool,
    /// Present USB VBUS before firmware boot without starting the native MIDI
    /// bridge. Typed USB scenarios own the host tokens and use this mode.
    pub usb_cable_at_start: bool,
    /// Connect the DIN/TRS MIDI IN/OUT jacks as soon as the engine starts,
    /// rather than waiting for a later [`Engine::connect_midi`] call.
    pub midi_enabled: bool,
    pub timing: Timing,
    pub smoke_scenario: Option<PathBuf>,
    pub update_bootloader: bool,
    /// Optional local GDB socket for deterministic firmware diagnostics.
    pub gdb_socket: Option<PathBuf>,
    /// Optional local socket for `l6ctl` commands to a running emulator.
    pub control_socket: Option<PathBuf>,
    /// Leave all guest CPUs paused after QMP setup so a diagnostic can attach
    /// GDB breakpoints before firmware execution begins.
    pub start_paused: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timing {
    /// 2 ns per instruction, with QEMU sleeping when ahead of wall time.
    Paced,
    /// QEMU continuously adjusts the instruction/time ratio.
    Adaptive,
    /// Use QEMU's host virtual clock without instruction counting.
    HostClock,
    /// Use the host virtual clock and one TCG execution thread per vCPU.
    HostClockMulti,
    /// Run MAIN and the panel in separate paced QEMU processes, each with its
    /// own instruction rate and host thread. Runs are not deterministic.
    Split,
}

impl Timing {
    fn accelerator(self) -> &'static str {
        match self {
            Self::Paced | Self::Adaptive | Self::HostClock | Self::Split => "tcg,thread=single",
            Self::HostClockMulti => "tcg,thread=multi",
        }
    }

    fn icount(self) -> Option<&'static str> {
        match self {
            Self::Paced | Self::Split => Some("shift=1,align=on,sleep=on"),
            Self::Adaptive => Some("shift=auto,sleep=on"),
            Self::HostClock | Self::HostClockMulti => None,
        }
    }
}

#[cfg(test)]
mod timing_tests {
    use super::{Options, Timing};

    #[test]
    fn host_clock_disables_icount_without_changing_tcg_threading() {
        assert_eq!(Timing::HostClock.accelerator(), "tcg,thread=single");
        assert_eq!(Timing::HostClockMulti.accelerator(), "tcg,thread=multi");
        assert_eq!(Timing::HostClock.icount(), None);
        assert_eq!(Timing::HostClockMulti.icount(), None);
        assert_eq!(Timing::Paced.icount(), Some("shift=1,align=on,sleep=on"));
        assert_eq!(Timing::Adaptive.icount(), Some("shift=auto,sleep=on"));
        assert_eq!(Timing::Split.icount(), Some("shift=1,align=on,sleep=on"));
    }

    #[test]
    fn host_clock_multi_is_selectable_for_diagnostics() {
        let options = Options::parse_args(["--timing".into(), "host-clock-multi".into()]).unwrap();
        assert!(matches!(options.timing, Timing::HostClockMulti));
    }

    #[test]
    fn usb_cable_can_be_present_without_starting_the_midi_bridge() {
        let options = Options::parse_args(["--usb-cable-at-start".into()]).unwrap();
        assert!(options.usb_cable_at_start);
        assert!(!options.usb_enabled);
    }

    #[test]
    fn legacy_ui_smoke_flag_directs_to_the_supervised_driver() {
        let error = match Options::parse_args(["--ui-input-smoke".into()]) {
            Ok(_) => panic!("legacy UI smoke flag unexpectedly parsed"),
            Err(error) => error,
        };
        assert!(error.contains("migrated"));
        assert!(error.contains("-p l6test"));
        assert!(error.contains("--backend gui"));
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
            audio_device: env::var("L6_AUDIODEV").ok(),
            firmware_dir: workspace.join("unpacked"),
            firmware_package: env::var_os("L6_FIRMWARE").map(PathBuf::from),
            update_package: None,
            auto_setup: true,
            logs: workspace.join("emulator/logs"),
            sd_image: None,
            no_sd: false,
            state_dir: Some(workspace.join("emulator/state")),
            trace_sd: false,
            usb_enabled: false,
            usb_cable_at_start: false,
            midi_enabled: false,
            timing: Timing::Paced,
            smoke_scenario: None,
            update_bootloader: false,
            gdb_socket: None,
            control_socket: None,
            start_paused: false,
        };
        let mut args = arguments.into_iter();
        while let Some(arg) = args.next() {
            match arg.to_string_lossy().as_ref() {
                "--ui-input-smoke" => {
                    return Err("the standalone UI smoke was migrated; run `cargo run -p l6test -- run --case panel --model MODEL --backend gui` instead".into());
                }
                "--smoke-scenario" => {
                    options.smoke_scenario = Some(take_path(&mut args, "--smoke-scenario")?)
                }
                "--start-paused" => options.start_paused = true,
                "--update-bootloader" => options.update_bootloader = true,
                "--qemu" => options.qemu = Some(take_path(&mut args, "--qemu")?),
                "--audio-device" => {
                    options.audio_device = Some(
                        args.next()
                            .and_then(|value| value.into_string().ok())
                            .ok_or("--audio-device requires a QEMU audiodev specification")?,
                    )
                }
                "--firmware-dir" => options.firmware_dir = take_path(&mut args, "--firmware-dir")?,
                "--firmware-package" => {
                    options.firmware_package = Some(take_path(&mut args, "--firmware-package")?)
                }
                "--update-package" => {
                    options.update_package = Some(take_path(&mut args, "--update-package")?)
                }
                "--no-auto-setup" => options.auto_setup = false,
                "--logs" => options.logs = take_path(&mut args, "--logs")?,
                "--gdb-socket" => options.gdb_socket = Some(take_path(&mut args, "--gdb-socket")?),
                "--control-socket" => {
                    options.control_socket = Some(take_path(&mut args, "--control-socket")?)
                }
                "--usb" => options.usb_enabled = true,
                "--usb-cable-at-start" => options.usb_cable_at_start = true,
                "--midi" => options.midi_enabled = true,
                "--trace-sd" => options.trace_sd = true,
                "--sd-image" => options.sd_image = Some(take_path(&mut args, "--sd-image")?),
                "--no-sd" => options.no_sd = true,
                "--state-dir" => options.state_dir = Some(take_path(&mut args, "--state-dir")?),
                "--volatile" => options.state_dir = None,
                "--timing" => {
                    options.timing = match args.next().as_deref().and_then(|value| value.to_str()) {
                        Some("paced") => Timing::Paced,
                        Some("adaptive") => Timing::Adaptive,
                        Some("host-clock") => Timing::HostClock,
                        Some("host-clock-multi") => Timing::HostClockMulti,
                        Some("split") => Timing::Split,
                        _ => return Err(
                            "--timing requires paced, adaptive, host-clock, host-clock-multi, or split"
                                .into(),
                        ),
                    };
                }
                "--help" | "-h" => {
                    println!(
                        "Engine options: [QEMU] [--qemu PATH] [--audio-device 'driver=...,id=...'] [--firmware-dir DIR | --firmware-package PACKAGE] [--update-package PACKAGE] [--no-auto-setup] [--logs DIR] [--state-dir DIR | --volatile] [--sd-image FILE | --no-sd] [--update-bootloader] [--usb] [--midi] [--trace-sd] [--timing paced|adaptive|host-clock|host-clock-multi|split] [--smoke-scenario FILE] [--gdb-socket PATH] [--control-socket PATH] [--start-paused]"
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
        options.qemu = options.qemu.or_else(crate::setup::existing_qemu);
        Ok(options)
    }
}

pub struct Engine {
    model: l6fw::Model,
    children: Vec<Child>,
    pub inputs: Option<Arc<InputHub>>,
    pub management: Vec<QmpClient>,
    pub display_memory: Option<SharedRam>,
    pub usb: Option<Arc<crate::usb::UsbHost>>,
    pub midi: Option<crate::usb_midi::MidiBridge>,
    pub din_midi: Option<crate::din_midi::DinMidiBridge>,
    control_server: Option<crate::control::ControlServer>,
}

impl Engine {
    pub fn model(&self) -> l6fw::Model {
        self.model
    }

    pub fn start(options: &Options) -> Result<Self, String> {
        let mut prepared;
        let options = if options.auto_setup {
            prepared = options.clone();
            prepared.prepare()?;
            &prepared
        } else {
            options
        };
        let mut engine = Self {
            model: l6fw::Model::L6max,
            children: Vec::new(),
            inputs: None,
            management: Vec::new(),
            display_memory: None,
            usb: None,
            midi: None,
            din_midi: None,
            control_server: None,
        };
        let Some(qemu) = &options.qemu else {
            return Ok(engine);
        };
        let qemu = qemu
            .canonicalize()
            .map_err(|e| format!("cannot find QEMU at {}: {e}", qemu.display()))?;
        fs::create_dir_all(&options.logs).map_err(|e| e.to_string())?;
        let logs = options.logs.canonicalize().map_err(|e| e.to_string())?;
        let firmware = options.firmware_dir.canonicalize().map_err(|e| {
            format!(
                "firmware directory {} is not usable: {e}",
                options.firmware_dir.display()
            )
        })?;
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
        let main = fs::read(firmware.join("main_firmware.bin"))
            .map_err(|e| format!("cannot read MAIN firmware image: {e}"))?;
        let model = match options.state_dir.as_ref() {
            Some(state) => match fs::read(state.join("main-nor.bin")) {
                Ok(nor) => l6fw::model_from_main_nor(&nor)
                    .map_err(|e| format!("cannot identify installed MAIN firmware: {e}"))?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    l6fw::model_from_main(&main)?
                }
                Err(error) => return Err(format!("cannot read installed MAIN NOR: {error}")),
            },
            None => l6fw::model_from_main(&main)?,
        };
        engine.model = model;
        if options.update_bootloader {
            let state = options
                .state_dir
                .as_ref()
                .ok_or("--update-bootloader requires persistent --state-dir storage")?;
            let outcome = crate::update_bootloader::install(state, sd.as_deref(), model)
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
        let (din_midi_host, din_midi_guest) = UnixStream::pair().map_err(|e| e.to_string())?;
        let split = options.timing == Timing::Split;
        let (link_main, link_panel) = UnixStream::pair().map_err(|e| e.to_string())?;
        let (control_main, control_panel) = UnixStream::pair().map_err(|e| e.to_string())?;
        // Both the USB and DIN MIDI sockets are always inherited, like the
        // real chip's always-present USB and UART silicon: whether a cable
        // is actually "plugged in" is a host-side runtime toggle
        // (`Engine::connect_usb`/`connect_midi`), not a boot-time choice.
        let mut fds = vec![
            main_input_guest.as_raw_fd(),
            panel_input_guest.as_raw_fd(),
            qmp_guest.as_raw_fd(),
            display_fd,
            usb_guest.as_raw_fd(),
            din_midi_guest.as_raw_fd(),
        ];
        if split {
            fds.retain(|&fd| fd != panel_input_guest.as_raw_fd());
            fds.extend([link_main.as_raw_fd(), control_main.as_raw_fd()]);
        }
        let audio_id = options
            .audio_device
            .as_deref()
            .map(|spec| {
                spec.split(',')
                    .find_map(|part| part.strip_prefix("id="))
                    .filter(|id| !id.is_empty())
                    .ok_or("--audio-device must contain a nonempty id= field")
            })
            .transpose()?;
        let machine_type = if split { "l6max-main" } else { "l6max-dual" };
        let machine = audio_id.map_or_else(
            || machine_type.to_owned(),
            |id| format!("{machine_type},audiodev={id}"),
        );
        let mut command = Command::new(&qemu);
        command
            .arg("-M")
            .arg(&machine)
            .args(["-accel", options.timing.accelerator()]);
        if let Some(plugin) = env::var_os("L6_QEMU_PLUGIN") {
            command.arg("-plugin").arg(plugin);
        }
        if split && model == l6fw::Model::L6 {
            // L6's full-rate audio fits 4 ns per instruction, and it cannot
            // hold real time during playback at 2 ns on one host thread.
            command.args(["-icount", "shift=2,align=on,sleep=on"]);
        } else if let Some(icount) = options.timing.icount() {
            command.args(["-icount", icount]);
        }
        if !split {
            command.args(["-smp", "2"]);
        }
        command
            .arg("-kernel")
            .arg(firmware.join("main_firmware.bin"))
            .args([
                "-S",
                "-no-shutdown",
                "-display",
                "none",
                "-monitor",
                "none",
                "-serial",
                if split { "chardev:panel-link" } else { "none" },
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
            .env_remove("L6_MODEL")
            .env("L6_MAIN_INPUT_FD", main_input_guest.as_raw_fd().to_string())
            .env(
                "L6_PANEL_INPUT_FD",
                panel_input_guest.as_raw_fd().to_string(),
            )
            .env("L6_DISPLAY_FD", display_fd.to_string())
            .env_remove("L6_EXTERNAL_RAM_FD")
            .env(
                "L6_SERVICE_AUDIO_QUEUE",
                env::var("L6_SERVICE_AUDIO_QUEUE").unwrap_or_else(|_| "1".into()),
            )
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
        if split {
            command
                .args(["-chardev"])
                .arg(format!(
                    "socket,id=panel-link,fd={},server=off",
                    link_main.as_raw_fd()
                ))
                .env("L6_INPUT_FD", main_input_guest.as_raw_fd().to_string())
                .env("L6_PANEL_CONTROL_FD", control_main.as_raw_fd().to_string());
        }
        if let Some(spec) = &options.audio_device {
            command.args(["-audiodev", spec]);
        }
        if let Some(socket) = &options.gdb_socket {
            command
                .arg("-gdb")
                .arg(format!("unix:{},server=on,wait=off", socket.display()));
        }
        // The SD slot is always inherited, like the real card-edge connector
        // is always present; whether a card is in it is a runtime QMP
        // eject/insert (`Engine::eject_sd`/`insert_sd`), not a boot-time
        // choice, unless `sd` already names one to start with.
        command.arg("-drive").arg(match &sd {
            Some(sd) => format!("if=sd,format=raw,id=l6-sd,file={}", sd.display()),
            None => "if=sd,id=l6-sd".to_owned(),
        });
        command.env_remove("L6_USB_FD");
        command.env_remove("L6_MAIN_MIDI_UART_FD");
        command.env_remove("L6_STATE_DIR");
        if let Some(directory) = &options.state_dir {
            fs::create_dir_all(directory).map_err(|e| e.to_string())?;
            command.env(
                "L6_STATE_DIR",
                directory.canonicalize().map_err(|e| e.to_string())?,
            );
        }
        command.env("L6_USB_FD", usb_guest.as_raw_fd().to_string());
        command.env(
            "L6_MAIN_MIDI_UART_FD",
            din_midi_guest.as_raw_fd().to_string(),
        );
        if options.trace_sd {
            let trace_file = logs.join("sdhci.trace");
            let trace_options = format!("enable=sdhci_*,file={}", trace_file.display());
            command.args(["-trace", trace_options.as_str()]);
        }
        engine.usb = Some(Arc::new(
            crate::usb::UsbHost::new(usb_host).map_err(|e| e.to_string())?,
        ));
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
        if split {
            let (qmp_parent, qmp_guest) = UnixStream::pair().map_err(|e| e.to_string())?;
            let fds = [
                panel_input_guest.as_raw_fd(),
                qmp_guest.as_raw_fd(),
                link_panel.as_raw_fd(),
                control_panel.as_raw_fd(),
            ];
            let mut panel = Command::new(&qemu);
            panel
                .args(["-M", "l6max-panel", "-accel", options.timing.accelerator()])
                // The firmware times its LED PWM pulses in software, so ring
                // brightness scales with this cost; 64 ns per instruction
                // reaches the rings' measured full scale on both models.
                .args(["-icount", "shift=6,sleep=off", "-kernel"])
                .arg(firmware.join("secondary_firmware.bin"))
                .args(["-S", "-no-shutdown", "-display", "none", "-monitor", "none"])
                .args(["-serial", "chardev:panel-link", "-chardev"])
                .arg(format!(
                    "socket,id=panel-link,fd={},server=off",
                    link_panel.as_raw_fd()
                ))
                .arg("-chardev")
                .arg(format!("socket,id=management,fd={}", qmp_guest.as_raw_fd()))
                .args([
                    "-object",
                    "monitor-qmp,id=management-monitor,chardev=management",
                    "-d",
                    "guest_errors,unimp",
                    "-D",
                ])
                .arg(logs.join("engine-panel.trace"))
                .current_dir(&logs)
                .env_clear()
                .env(
                    "L6_MODEL",
                    if model == l6fw::Model::L6 {
                        "l6"
                    } else {
                        "l6max"
                    },
                )
                .env("L6_PANEL_REALTIME", "1")
                .env("L6_INPUT_FD", panel_input_guest.as_raw_fd().to_string())
                .env("L6_PANEL_CONTROL_FD", control_panel.as_raw_fd().to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::from(
                    File::create(logs.join("engine-panel.stderr")).map_err(|e| e.to_string())?,
                ));
            if let Some(directory) = &options.state_dir {
                panel.env(
                    "L6_STATE_DIR",
                    directory.canonicalize().map_err(|e| e.to_string())?,
                );
            }
            // Only the selected IPC descriptors survive exec.
            unsafe {
                panel.pre_exec(move || {
                    for &fd in &fds {
                        let flags = libc::fcntl(fd, libc::F_GETFD);
                        if flags < 0
                            || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0
                        {
                            return Err(io::Error::last_os_error());
                        }
                    }
                    Ok(())
                });
            }
            engine.children.push(
                panel
                    .spawn()
                    .map_err(|e| format!("start panel QEMU engine:{e}"))?,
            );
            engine
                .management
                .push(QmpClient::connect(qmp_parent).map_err(|e| {
                    let detail =
                        fs::read_to_string(logs.join("engine-panel.stderr")).unwrap_or_default();
                    format!("panel QMP handshake:{e}; QEMU: {detail}")
                })?);
        }
        if !engine.connected() {
            return Err("a firmware process exited during startup".into());
        }
        if options.usb_cable_at_start {
            engine
                .usb
                .as_ref()
                .ok_or("USB cable connection needs a USB host socket")?
                .connect(true)
                .map_err(|error| format!("connect USB cable before boot: {error}"))?;
        }
        if !options.start_paused {
            for management in &engine.management {
                management
                    .resume()
                    .map_err(|e| format!("resume guest: {e}"))?;
            }
        }
        engine.inputs = Some(Arc::new(InputHub::new(main_input, panel_input, model)));
        if let Some(path) = options.control_socket.clone() {
            engine.control_server = Some(crate::control::ControlServer::start(
                path,
                engine.inputs.as_ref().unwrap().clone(),
                model,
            )?);
        }
        // The DIN MIDI bridge is always running, draining the UART socket so
        // the guest's transmit side never blocks; whether it is also wired to
        // a native host MIDI port is the `midi_enabled` connect state.
        engine.din_midi = Some(
            crate::din_midi::DinMidiBridge::start(
                din_midi_host,
                options.midi_enabled,
                crate::host_midi::din_port_name_for_model(model),
            )
            .map_err(|e| e.to_string())?,
        );
        if options.usb_enabled {
            engine.connect_usb()?;
        }
        Ok(engine)
    }
    /// Connect the USB cable: enables the 3 USB MIDI ports and lets firmware
    /// enumerate. A no-op if already connected.
    pub fn connect_usb(&mut self) -> Result<(), String> {
        if self.midi.is_none() {
            let usb = self
                .usb
                .clone()
                .ok_or("USB is not available for this engine")?;
            self.midi = Some(
                crate::usb_midi::MidiBridge::start(
                    usb,
                    crate::host_midi::port_names_for_model(self.model),
                )
                .map_err(|e| e.to_string())?,
            );
        }
        Ok(())
    }
    /// Disconnect the USB cable, like unplugging it: stops the 3 USB MIDI
    /// ports and tells firmware the device is gone, without tearing down the
    /// inherited socket, so a later `connect_usb` can reconnect in place.
    pub fn disconnect_usb(&mut self) {
        if let Some(midi) = self.midi.take() {
            midi.disconnect();
        }
        if let Some(usb) = &self.usb {
            let _ = usb.connect(false);
        }
    }
    pub fn usb_connected(&self) -> bool {
        self.midi.is_some()
    }
    /// Connect the DIN/TRS MIDI IN/OUT jacks to the host MIDI port. Unlike
    /// USB, firmware has no way to observe this: a real TRS jack has no
    /// plug-detection either, so this only toggles the host-side bridge.
    pub fn connect_midi(&self) {
        if let Some(din_midi) = &self.din_midi {
            din_midi.set_connected(true);
        }
    }
    pub fn disconnect_midi(&self) {
        if let Some(din_midi) = &self.din_midi {
            din_midi.set_connected(false);
        }
    }
    pub fn midi_connected(&self) -> bool {
        self.din_midi
            .as_ref()
            .is_some_and(crate::din_midi::DinMidiBridge::is_connected)
    }
    /// Insert an SD card image, like inserting a card into the slot: no
    /// restart needed, the slot is always present (see `Engine::start`).
    pub fn insert_sd(&self, image: &std::path::Path) -> Result<(), String> {
        self.management
            .first()
            .ok_or("no management connection")?
            .insert_sd(image)
            .map_err(|e| e.to_string())
    }
    /// Eject the SD card, like pulling it out of the slot.
    pub fn eject_sd(&self) -> Result<(), String> {
        self.management
            .first()
            .ok_or("no management connection")?
            .eject_sd()
            .map_err(|e| e.to_string())
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
        drop(self.control_server.take());
        drop(self.midi.take());
        for management in &self.management {
            let _ = management.quit();
        }
        // Give QEMU's QMP quit path time to close audio backends and flush
        // diagnostics before escalating to a signal. Sending SIGTERM
        // immediately after quit can interrupt that cleanup.
        let graceful_deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < graceful_deadline
            && self
                .children
                .iter_mut()
                .any(|c| matches!(c.try_wait(), Ok(None)))
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        for child in &mut self.children {
            if matches!(child.try_wait(), Ok(None)) {
                unsafe {
                    libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
                }
            }
        }
        let terminate_deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < terminate_deadline
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
