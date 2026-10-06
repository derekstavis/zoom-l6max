use std::{
    env, fs,
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, ExitCode, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
struct Options {
    qemu: Option<PathBuf>,
    seconds: f64,
    logs: Option<PathBuf>,
    sd_image: Option<PathBuf>,
    trace_sd: bool,
    service_audio_queue: bool,
    panel_keys: Vec<String>,
    main_keys: Vec<String>,
    probes: Vec<(u32, usize, bool)>,
}

struct Monitor {
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl Monitor {
    fn new(child: &mut Child) -> Result<Self, String> {
        let input = child.stdin.take().ok_or("QEMU monitor has no input pipe")?;
        let output = child
            .stdout
            .take()
            .ok_or("QEMU monitor has no output pipe")?;
        let mut monitor = Self {
            input,
            output: BufReader::new(output),
        };
        monitor.read_prompt()?;
        Ok(monitor)
    }

    fn read_prompt(&mut self) -> Result<String, String> {
        let mut result = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            let count = self
                .output
                .read(&mut byte)
                .map_err(|error| format!("reading QEMU monitor: {error}"))?;
            if count == 0 {
                return Err("QEMU monitor closed".into());
            }
            result.push(byte[0]);
            if result.ends_with(b"(qemu)") {
                return Ok(String::from_utf8_lossy(&result).into_owned());
            }
        }
    }

    fn command(&mut self, command: &str) -> Result<String, String> {
        writeln!(self.input, "{command}")
            .and_then(|_| self.input.flush())
            .map_err(|error| format!("writing QEMU monitor: {error}"))?;
        self.read_prompt()
    }
}

struct Guest {
    name: &'static str,
    child: Child,
    monitor: Monitor,
}

impl Drop for Guest {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM);
        }
        for _ in 0..20 {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn fail(message: impl std::fmt::Display) -> ExitCode {
    eprintln!("qemu-run: {message}");
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fail(error),
    }
}

fn parse_num(value: &str) -> Result<u32, String> {
    if let Some(hex) = value.strip_prefix("0x") {
        u32::from_str_radix(hex, 16).map_err(|_| format!("invalid number: {value}"))
    } else {
        value
            .parse()
            .map_err(|_| format!("invalid number: {value}"))
    }
}

fn parse_probe(value: &str, panel: bool) -> Result<(u32, usize, bool), String> {
    let (address, size) = value.split_once(':').ok_or("probe must be ADDRESS:SIZE")?;
    let address = parse_num(address)?;
    let size = parse_num(size)? as usize;
    if size == 0 || size > 0x100000 {
        return Err("probe size must be between 1 and 0x100000".into());
    }
    Ok((address, size, panel))
}

fn validate_panel_key(key: &str) -> Result<(), String> {
    let values: Result<Vec<u32>, _> = key.split(':').map(str::parse).collect();
    let values = values.map_err(|_| "--panel-key expects ROW:COL:START_MS:END_MS")?;
    if values.len() != 4 || values[0] >= 8 || values[1] >= 5 || values[2] >= values[3] {
        return Err("--panel-key expects row 0..7, column 0..4, and start < end".into());
    }
    Ok(())
}

fn validate_main_key(key: &str) -> Result<(), String> {
    let (name, interval) = key
        .split_once(':')
        .ok_or("--main-key expects NAME:START_MS:END_MS")?;
    if !matches!(name, "menu" | "play" | "record") {
        return Err("--main-key name must be menu, play, or record".into());
    }
    let (start, end) = interval
        .split_once(':')
        .ok_or("--main-key expects NAME:START_MS:END_MS")?;
    let start: u32 = start.parse().map_err(|_| "invalid --main-key start")?;
    let end: u32 = end.parse().map_err(|_| "invalid --main-key end")?;
    if start >= end {
        return Err("--main-key requires start < end".into());
    }
    Ok(())
}

fn parse_options() -> Result<Options, String> {
    let mut options = Options {
        seconds: 2.0,
        ..Options::default()
    };
    let mut args = env::args().skip(1);
    while let Some(flag) = args.next() {
        let value =
            |args: &mut std::iter::Skip<std::env::Args>, name: &str| -> Result<String, String> {
                args.next()
                    .ok_or_else(|| format!("{name} requires a value"))
            };
        match flag.as_str() {
            "--qemu" => options.qemu = Some(PathBuf::from(value(&mut args, "--qemu")?)),
            "--logs" => options.logs = Some(PathBuf::from(value(&mut args, "--logs")?)),
            "--sd-image" => options.sd_image = Some(PathBuf::from(value(&mut args, "--sd-image")?)),
            "--seconds" => {
                options.seconds = value(&mut args, "--seconds")?
                    .parse()
                    .map_err(|_| "invalid --seconds")?
            }
            "--trace-sd" => options.trace_sd = true,
            "--service-audio-queue" => options.service_audio_queue = true,
            "--panel-key" => options.panel_keys.push(value(&mut args, "--panel-key")?),
            "--main-key" => options.main_keys.push(value(&mut args, "--main-key")?),
            "--panel-probe" => options
                .probes
                .push(parse_probe(&value(&mut args, "--panel-probe")?, true)?),
            "--probe" => options
                .probes
                .push(parse_probe(&value(&mut args, "--probe")?, false)?),
            "--button" => {
                let text = value(&mut args, "--button")?;
                let parts: Vec<_> = text.split(':').collect();
                if parts.len() != 3 {
                    return Err("--button expects NAME:START_MS:END_MS".into());
                }
                let start: u32 = parts[1].parse().map_err(|_| "invalid button start time")?;
                let end: u32 = parts[2].parse().map_err(|_| "invalid button end time")?;
                if start >= end {
                    return Err("--button requires START_MS < END_MS".into());
                }
                let (chip, key) = match parts[0] {
                    "menu" => ("main", "menu"),
                    "up" => ("panel", "5:4"),
                    "down" => ("panel", "4:4"),
                    "confirm" => ("panel", "7:4"),
                    "play" => ("main", "play"),
                    "record" => ("main", "record"),
                    "bounce" => ("panel", "6:4"),
                    _ => return Err(format!("unknown button: {}", parts[0])),
                };
                let interval = format!("{key}:{start}:{end}");
                if chip == "panel" {
                    options.panel_keys.push(interval);
                } else {
                    options.main_keys.push(interval);
                }
            }
            "--help" | "-h" => {
                println!(
                    "Usage: qemu-run --qemu PATH [--seconds N] [--button NAME:START:END] [--panel-key ROW:COL:START:END] [--main-key NAME:START:END] [--probe ADDRESS:SIZE]"
                );
                std::process::exit(0);
            }
            _ => return Err(format!("unknown option: {flag}")),
        }
    }
    if options.qemu.is_none() {
        return Err("--qemu is required".into());
    }
    if !options.seconds.is_finite() || options.seconds <= 0.0 {
        return Err("--seconds must be positive".into());
    }
    if options.panel_keys.len() > 32 || options.main_keys.len() > 32 {
        return Err("at most 32 key intervals per chip are supported".into());
    }
    for key in &options.panel_keys {
        validate_panel_key(key)?;
    }
    for key in &options.main_keys {
        validate_main_key(key)?;
    }
    Ok(options)
}

fn spawn_guest(
    qemu: &Path,
    logs: &Path,
    link: &Path,
    name: &'static str,
    machine: &str,
    firmware: &Path,
    panel_keys: &[String],
    main_keys: &[String],
    options: &Options,
) -> Result<Guest, String> {
    let mut command = Command::new(qemu);
    command
        .arg("-M")
        .arg(machine)
        .arg("-kernel")
        .arg(firmware)
        .args(["-display", "none", "-monitor", "stdio", "-serial"])
        .arg(if name == "main" {
            format!("unix:{},server=on,wait=off", link.display())
        } else {
            format!("unix:{}", link.display())
        })
        .args(["-d", "in_asm,guest_errors,unimp", "-D"])
        .arg(logs.join(format!("{name}.trace")))
        .current_dir(logs)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(
            fs::File::create(logs.join(format!("{name}.stderr")))
                .map_err(|error| format!("create {name} stderr log: {error}"))?,
        ));
    if name == "panel" && !panel_keys.is_empty() {
        command.env("L6_PANEL_KEYS", panel_keys.join(","));
    }
    if name == "main" && !main_keys.is_empty() {
        let ids = main_keys
            .iter()
            .map(|key| {
                let (name, interval) = key.split_once(':').ok_or("bad main key")?;
                let id = match name {
                    "menu" => "0",
                    "play" => "1",
                    "record" => "2",
                    _ => return Err("invalid main key name"),
                };
                Ok(format!("{id}:{interval}"))
            })
            .collect::<Result<Vec<_>, &str>>()
            .map_err(str::to_owned)?;
        command.env("L6_MAIN_KEYS", ids.join(","));
    }
    if name == "main" && options.service_audio_queue {
        command.env("L6_SERVICE_AUDIO_QUEUE", "1");
    }
    if name == "main" && options.trace_sd {
        command.args(["-trace", "sdhci_*"]);
    }
    if name == "main" {
        if let Some(sd) = &options.sd_image {
            command
                .arg("-drive")
                .arg(format!("if=sd,format=raw,file={}", sd.display()));
        }
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("start {name} QEMU: {error}"))?;
    let monitor = Monitor::new(&mut child)?;
    Ok(Guest {
        name,
        child,
        monitor,
    })
}

fn snapshot(
    guest: &mut Guest,
    logs: &Path,
    address: u32,
    size: usize,
    filename: &str,
) -> Result<PathBuf, String> {
    let path = logs.join(filename);
    let _ = fs::remove_file(&path);
    guest
        .monitor
        .command(&format!("pmemsave 0x{address:x} {size} {filename}"))?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while !path.is_file() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    if !path.is_file() || path.metadata().map_err(|e| e.to_string())?.len() != size as u64 {
        return Err(format!("QEMU did not save {filename}"));
    }
    Ok(path)
}

fn write_pgm(raw: &[u8], path: &Path) -> Result<usize, String> {
    if raw.len() != 1024 {
        return Err("framebuffer must be 1024 bytes".into());
    }
    let mut pixels = vec![0u8; 128 * 64];
    for y in 0..64usize {
        for x in 0..128usize {
            let set = (raw[((63 - y) / 8) * 128 + x] >> ((63 - y) % 8)) & 1;
            pixels[y * 128 + x] = if set == 1 { 255 } else { 0 };
        }
    }
    let lit = raw.iter().map(|byte| byte.count_ones() as usize).sum();
    let mut file = fs::File::create(path).map_err(|e| e.to_string())?;
    file.write_all(b"P5\n128 64\n255\n")
        .and_then(|_| file.write_all(&pixels))
        .map_err(|e| e.to_string())?;
    image::GrayImage::from_raw(128, 64, pixels)
        .ok_or("could not construct framebuffer image")?
        .save(path.with_extension("png"))
        .map_err(|e| format!("saving PNG framebuffer: {e}"))?;
    Ok(lit)
}

fn run() -> Result<(), String> {
    let options = parse_options()?;
    let qemu = options
        .qemu
        .as_ref()
        .unwrap()
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest
        .parent()
        .and_then(Path::parent)
        .ok_or("workspace path missing")?;
    let firmware = workspace.join("unpacked");
    let logs = options
        .logs
        .clone()
        .unwrap_or_else(|| workspace.join("emulator/logs"));
    fs::create_dir_all(&logs).map_err(|e| e.to_string())?;
    let logs = logs.canonicalize().map_err(|e| e.to_string())?;
    let link = logs.join("panel-link.sock");
    let _ = fs::remove_file(&link);
    let mut main = spawn_guest(
        &qemu,
        &logs,
        &link,
        "main",
        "l6max-main",
        &firmware.join("main_firmware.bin"),
        &[],
        &options.main_keys,
        &options,
    )?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while !link.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    if !link.exists() {
        return Err("main QEMU did not open the panel serial socket".into());
    }
    let mut panel = spawn_guest(
        &qemu,
        &logs,
        &link,
        "panel",
        "l6max-panel",
        &firmware.join("secondary_firmware.bin"),
        &options.panel_keys,
        &[],
        &options,
    )?;
    thread::sleep(Duration::from_secs_f64(options.seconds));

    for guest in [&mut main, &mut panel] {
        let registers = guest.monitor.command("info registers")?;
        fs::write(logs.join(format!("{}.registers", guest.name)), &registers)
            .map_err(|e| e.to_string())?;
        let pc = registers
            .split_whitespace()
            .find_map(|field| {
                field
                    .strip_prefix("R15=")
                    .map(|value| format!("0x{}", value.to_lowercase()))
            })
            .unwrap_or_else(|| "unknown".into());
        println!("{} live PC: {pc}", guest.name);
    }
    let raw_path = snapshot(&mut main, &logs, 0x80528c24, 1024, "main.framebuffer.bin")?;
    let raw = fs::read(&raw_path).map_err(|e| e.to_string())?;
    let lit = write_pgm(&raw, &logs.join("main.framebuffer.pgm"))?;
    let display_task_seen = fs::read_to_string(logs.join("main.trace"))
        .unwrap_or_default()
        .contains("0x80085030:");
    fs::write(
        logs.join("main.display-ready"),
        if display_task_seen && lit > 0 {
            "yes\n"
        } else {
            "no\n"
        },
    )
    .map_err(|e| e.to_string())?;
    println!(
        "LCD framebuffer: {lit}/8192 set bits; DisplayDriver task observed: {display_task_seen}; {}",
        logs.join("main.framebuffer.pgm").display()
    );
    for (address, size, panel_probe) in &options.probes {
        let guest = if *panel_probe { &mut panel } else { &mut main };
        let name = if *panel_probe { "panel" } else { "main" };
        let filename = format!("{name}.{address:08x}.{size:x}.bin");
        snapshot(guest, &logs, *address, *size, &filename)?;
        println!("RAM probe: {}", logs.join(filename).display());
    }
    let main_trace = fs::read_to_string(logs.join("main.trace")).unwrap_or_default();
    let panel_trace = fs::read_to_string(logs.join("panel.trace")).unwrap_or_default();
    println!(
        "UART traces: panel TX {}, RX {}; main LPUART RX {}",
        panel_trace.matches("l6 panel USART1 TX ").count(),
        panel_trace.matches("l6 panel USART1 RX ").count(),
        main_trace.matches("l6 main LPUART1 RX ").count()
    );
    Ok(())
}
