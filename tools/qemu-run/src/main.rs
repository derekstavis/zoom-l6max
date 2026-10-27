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
    audio_device: Option<String>,
    firmware_dir: Option<PathBuf>,
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
    if !matches!(
        name,
        "menu"
            | "play"
            | "record"
            | "tap"
            | "sound-pad-1"
            | "sound-pad-2"
            | "sound-pad-3"
            | "sound-pad-4"
    ) {
        return Err("--main-key name must be menu, play, record, tap, or sound-pad-1..4".into());
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

fn validate_main_key_model(key: &str, model: l6fw::Model) -> Result<(), String> {
    let name = key
        .split_once(':')
        .map(|(name, _)| name)
        .unwrap_or_default();
    let supported = match model {
        l6fw::Model::L6 => matches!(
            name,
            "play" | "tap" | "sound-pad-1" | "sound-pad-2" | "sound-pad-3" | "sound-pad-4"
        ),
        l6fw::Model::L6max => matches!(name, "menu" | "play" | "record"),
    };
    if !supported && model == l6fw::Model::L6 && name == "play" {
        return Err("play is not mapped to a verified L6 MAIN GPIO".into());
    }
    if !supported {
        return Err(format!(
            "{name} is not a verified MAIN GPIO key on {model:?}"
        ));
    }
    Ok(())
}

fn panel_uart_byte_counts(trace: &str, uart: u8) -> (usize, usize) {
    let prefix = format!("l6 main LPUART{uart} (panel link) ");
    let accepted_rx = trace.matches(&format!("{prefix}RX accepted ")).count();
    let discarded_rx = trace
        .matches(&format!("{prefix}RX discarded (RE=0) "))
        .count();
    let rom_rx = trace
        .matches(&format!("l6 main LPUART{uart} ROM RX "))
        .count();
    (
        accepted_rx + discarded_rx + rom_rx,
        trace.matches(&format!("{prefix}TX ")).count(),
    )
}

fn parse_options() -> Result<Options, String> {
    let mut options = Options {
        seconds: 2.0,
        audio_device: env::var("L6_AUDIODEV").ok(),
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
            "--audio-device" => {
                options.audio_device = Some(value(&mut args, "--audio-device")?);
            }
            "--firmware-dir" => {
                options.firmware_dir = Some(PathBuf::from(value(&mut args, "--firmware-dir")?))
            }
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
                    "tap" => ("main", "tap"),
                    "sound-pad-1" => ("main", "sound-pad-1"),
                    "sound-pad-2" => ("main", "sound-pad-2"),
                    "sound-pad-3" => ("main", "sound-pad-3"),
                    "sound-pad-4" => ("main", "sound-pad-4"),
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
                    "Usage: qemu-run --qemu PATH [--firmware-dir DIR] [--audio-device 'driver=...,id=...'] [--seconds N] [--button NAME:START:END] [--panel-key ROW:COL:START:END] [--main-key NAME:START:END] [--probe ADDRESS:SIZE]"
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
    model: l6fw::Model,
    panel_keys: &[String],
    main_keys: &[String],
    options: &Options,
) -> Result<Guest, String> {
    let audio_id = options
        .audio_device
        .as_deref()
        .map(|spec| {
            spec.split(',')
                .find_map(|part| part.strip_prefix("id="))
                .filter(|id| {
                    !id.is_empty()
                        && id
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
                })
                .ok_or("--audio-device must contain a valid id= field")
        })
        .transpose()?;
    let machine = if name == "main" {
        audio_id.map_or_else(
            || machine.to_owned(),
            |id| format!("{machine},audiodev={id}"),
        )
    } else {
        machine.to_owned()
    };
    let mut command = Command::new(qemu);
    command
        .arg("-M")
        .arg(&machine)
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
        // The split-process diagnostic has no MAIN image in its standalone
        // panel guest, so carry the model detected from MAIN into that guest.
        .env(
            "L6_MODEL",
            if model == l6fw::Model::L6 {
                "l6"
            } else {
                "l6max"
            },
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(
            fs::File::create(logs.join(format!("{name}.stderr")))
                .map_err(|error| format!("create {name} stderr log: {error}"))?,
        ));
    if name == "main" {
        if let Some(spec) = &options.audio_device {
            command.args(["-audiodev", spec]);
        }
    }
    if name == "panel" && !panel_keys.is_empty() {
        command.env("L6_PANEL_KEYS", panel_keys.join(","));
    }
    if name == "main" && !main_keys.is_empty() {
        let ids = main_keys
            .iter()
            .map(|key| {
                let (name, interval) = key.split_once(':').ok_or("bad main key")?;
                let id = match (model, name) {
                    (l6fw::Model::L6max, "menu") => "0",
                    (l6fw::Model::L6max, "play") => "1",
                    (l6fw::Model::L6max, "record") => "2",
                    (l6fw::Model::L6, "play") => "4",
                    (l6fw::Model::L6, "tap") => "48",
                    (l6fw::Model::L6, "sound-pad-1") => "49",
                    (l6fw::Model::L6, "sound-pad-2") => "50",
                    (l6fw::Model::L6, "sound-pad-3") => "51",
                    (l6fw::Model::L6, "sound-pad-4") => "52",
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
    let firmware = options
        .firmware_dir
        .clone()
        .unwrap_or_else(|| workspace.join("unpacked"))
        .canonicalize()
        .map_err(|error| format!("cannot resolve firmware directory: {error}"))?;
    let main_firmware = fs::read(firmware.join("main_firmware.bin"))
        .map_err(|error| format!("cannot read main_firmware.bin: {error}"))?;
    let model = l6fw::model_from_main(&main_firmware)?;
    for key in &options.main_keys {
        validate_main_key_model(key, model)?;
    }
    let max_panel_columns = if model == l6fw::Model::L6 { 3 } else { 5 };
    for key in &options.panel_keys {
        let values: Vec<u32> = key
            .split(':')
            .filter_map(|value| value.parse().ok())
            .collect();
        if values
            .get(1)
            .is_none_or(|column| *column >= max_panel_columns)
        {
            return Err(format!(
                "panel column for {} supports 0..{} on {}",
                key,
                max_panel_columns - 1,
                if model == l6fw::Model::L6 {
                    "L6"
                } else {
                    "L6max"
                }
            ));
        }
    }
    if model == l6fw::Model::L6 && !options.panel_keys.is_empty() {
        eprintln!("L6 raw panel keys use a 3-column by 8-row matrix");
    }
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
        model,
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
        model,
        &options.panel_keys,
        &[],
        &options,
    )?;
    thread::sleep(Duration::from_secs_f64(options.seconds));

    for guest in [&mut main, &mut panel] {
        if let Some(status) = guest
            .child
            .try_wait()
            .map_err(|error| format!("checking {} QEMU status: {error}", guest.name))?
        {
            return Err(format!(
                "{} QEMU exited before register capture (status {status})",
                guest.name
            ));
        }
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
    if model == l6fw::Model::L6max {
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
    } else {
        fs::write(logs.join("main.display-ready"), "unknown\n").map_err(|e| e.to_string())?;
        println!("LCD framebuffer: unavailable (L6 mapping not confirmed)");
    }
    for (address, size, panel_probe) in &options.probes {
        let guest = if *panel_probe { &mut panel } else { &mut main };
        let name = if *panel_probe { "panel" } else { "main" };
        let filename = format!("{name}.{address:08x}.{size:x}.bin");
        snapshot(guest, &logs, *address, *size, &filename)?;
        println!("RAM probe: {}", logs.join(filename).display());
    }
    let main_trace = fs::read_to_string(logs.join("main.trace")).unwrap_or_default();
    let panel_uart = if model == l6fw::Model::L6 { 7 } else { 1 };
    let (panel_to_main, main_to_panel) = panel_uart_byte_counts(&main_trace, panel_uart);
    println!(
        "LPUART{panel_uart} panel link bytes: panel → main {panel_to_main}; main → panel {main_to_panel}"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{panel_uart_byte_counts, validate_main_key, validate_main_key_model};

    #[test]
    fn counts_panel_link_bytes_for_both_model_uart_routes() {
        let trace = concat!(
            "l6 main LPUART7 (panel link) TX 12\n",
            "l6 main LPUART7 (panel link) RX accepted 34\n",
            "l6 main LPUART7 (panel link) RX discarded (RE=0) 56\n",
            "l6 main LPUART7 ROM RX 78\n",
            "l6 main LPUART1 (panel link) TX 56\n",
            "l6 main LPUART1 (panel link) RX accepted 78\n",
        );
        assert_eq!(panel_uart_byte_counts(trace, 7), (3, 1));
        assert_eq!(panel_uart_byte_counts(trace, 1), (1, 1));
    }

    #[test]
    fn l6_exposes_verified_main_gpio_keys_and_runtime_play() {
        for key in ["menu:0:500", "record:0:500"] {
            validate_main_key(key).unwrap();
            assert!(validate_main_key_model(key, l6fw::Model::L6).is_err());
        }
        assert!(validate_main_key_model("play:0:500", l6fw::Model::L6).is_ok());
        for key in [
            "tap:0:500",
            "sound-pad-1:0:500",
            "sound-pad-2:0:500",
            "sound-pad-3:0:500",
            "sound-pad-4:0:500",
        ] {
            validate_main_key(key).unwrap();
            assert!(validate_main_key_model(key, l6fw::Model::L6).is_ok());
        }
    }

    #[test]
    fn l6max_keeps_all_three_main_gpio_inputs() {
        for key in ["menu:0:500", "play:0:500", "record:0:500"] {
            validate_main_key(key).unwrap();
            assert!(validate_main_key_model(key, l6fw::Model::L6max).is_ok());
        }
        for key in ["tap:0:500", "sound-pad-1:0:500"] {
            validate_main_key(key).unwrap();
            assert!(validate_main_key_model(key, l6fw::Model::L6max).is_err());
        }
    }
}
