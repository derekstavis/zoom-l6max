//! Scripted, observational diagnostics using the production headless engine.
mod input;
mod midi;
mod popup_trigger;
mod uart_startup;
use l6max_diagnostics::gdb::Gdb;
use l6max_host::{engine::Options, headless::Headless, indicators::rings, panel::PanelData};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    env, fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Recipe {
    version: u32,
    model: String,
    main_sha256: Option<String>,
    #[serde(default = "wall_limit")]
    wall_timeout_s: u64,
    step: Vec<Step>,
}
fn wall_limit() -> u64 {
    600
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
enum Step {
    Wait {
        guest_ms: u32,
    },
    Press {
        target: u32,
        #[serde(default = "hold")]
        hold_ms: u32,
    },
    Encoder {
        channel: usize,
        steps: i32,
    },
    Snapshot {
        name: String,
    },
    Memory {
        name: String,
        address: u32,
        length: usize,
    },
    MemoryWrite {
        name: String,
        address: u32,
        bytes: Vec<u8>,
    },
    FirmwareCall {
        name: String,
        address: u32,
        args: Vec<u32>,
        #[serde(default = "call_timeout_ms")]
        timeout_ms: u32,
    },
    Trace {
        name: String,
        contains: String,
        #[serde(default = "trace_limit")]
        limit: usize,
    },
}
fn hold() -> u32 {
    50
}
fn trace_limit() -> usize {
    100
}
fn call_timeout_ms() -> u32 {
    2_000
}

impl Recipe {
    fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
        Self::parse(&text)
    }
    fn parse(text: &str) -> Result<Self, String> {
        let recipe: Self = toml::from_str(&text).map_err(|e| e.to_string())?;
        if recipe.version != 1
            || !["l6", "l6max"].contains(&recipe.model.as_str())
            || recipe.step.is_empty()
            || recipe.step.len() > 2000
            || recipe.wall_timeout_s == 0
            || recipe.wall_timeout_s > 3600
        {
            return Err(
                "recipe requires version 1, a model, 1..2000 steps and a wall timeout of 1..3600 seconds"
                    .into(),
            );
        }
        let mut names = BTreeSet::new();
        for step in &recipe.step {
            if let Step::Snapshot { name }
            | Step::Memory { name, .. }
            | Step::MemoryWrite { name, .. }
            | Step::FirmwareCall { name, .. }
            | Step::Trace { name, .. } = step
            {
                if !names.insert(name) {
                    return Err("observation names must be unique".into());
                }
            }
            match step {
                Step::Wait { guest_ms } if *guest_ms == 0 || *guest_ms > 600_000 => {
                    return Err("wait must be 1..600000 guest milliseconds".into());
                }
                Step::Press { target, hold_ms }
                    if *target >= 54 || *hold_ms == 0 || *hold_ms > 60_000 =>
                {
                    return Err("invalid input target or hold".into());
                }
                Step::Encoder { channel, steps }
                    if *channel == 0
                        || *channel > if recipe.model == "l6" { 6 } else { 8 }
                        || *steps == 0
                        || steps.unsigned_abs() > 1000 =>
                {
                    return Err("invalid encoder channel or steps".into());
                }
                Step::Memory {
                    name,
                    address,
                    length,
                } => {
                    validate_name(name)?;
                    if recipe
                        .main_sha256
                        .as_ref()
                        .is_none_or(|v| v.len() != 64 || !v.bytes().all(|b| b.is_ascii_hexdigit()))
                        || *length == 0
                        || *length > 4096
                        || address.checked_add(*length as u32).is_none()
                    {
                        return Err(
                            "memory requires a MAIN SHA-256 guard and a valid 1..4096-byte range"
                                .into(),
                        );
                    }
                }
                Step::MemoryWrite {
                    name,
                    address,
                    bytes,
                } => {
                    validate_name(name)?;
                    let end = address.checked_add(bytes.len() as u32);
                    if recipe
                        .main_sha256
                        .as_ref()
                        .is_none_or(|v| v.len() != 64 || !v.bytes().all(|b| b.is_ascii_hexdigit()))
                        || bytes.is_empty()
                        || bytes.len() > 256
                        || *address < 0x8000_0000
                        || end.is_none_or(|end| end > 0x8200_0000)
                    {
                        return Err("memory-write requires a MAIN SHA-256 guard and a 1..256-byte range in MAIN RAM (0x80000000..0x82000000)".into());
                    }
                }
                Step::FirmwareCall {
                    name,
                    address,
                    args,
                    timeout_ms,
                } => {
                    validate_name(name)?;
                    if recipe
                        .main_sha256
                        .as_ref()
                        .is_none_or(|v| v.len() != 64 || !v.bytes().all(|b| b.is_ascii_hexdigit()))
                        || *address < 0x8000_0000
                        || *address >= 0x8020_0000
                        || address & 1 != 0
                        || !(1..=5).contains(&args.len())
                        || *timeout_ms == 0
                        || *timeout_ms > 10_000
                    {
                        return Err("firmware-call requires a MAIN SHA-256 guard, an even MAIN address, 1..5 arguments and a timeout of 1..10000 ms".into());
                    }
                }
                Step::Snapshot { name } => validate_name(name)?,
                Step::Trace {
                    name,
                    contains,
                    limit,
                } => {
                    validate_name(name)?;
                    if contains.is_empty() || contains.len() > 256 || *limit == 0 || *limit > 1000 {
                        return Err("trace needs a nonempty pattern and limit of 1..1000".into());
                    }
                }
                _ => {}
            }
        }
        Ok(recipe)
    }
}
fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(
            "observation name must be 1..64 letters, digits, hyphens or underscores".into(),
        );
    }
    Ok(())
}
fn main_image(options: &Options) -> Result<Vec<u8>, String> {
    let main =
        fs::read(options.firmware_dir.join("main_firmware.bin")).map_err(|e| e.to_string())?;
    if let Some(nor) = options
        .state_dir
        .as_ref()
        .map(|p| p.join("main-nor.bin"))
        .filter(|p| p.exists())
    {
        let data = fs::read(nor).map_err(|e| e.to_string())?;
        if data.get(0x50000..0x50000 + main.len()) != Some(main.as_slice()) {
            return Err(
                "installed MAIN differs from the selected firmware; use a matching image".into(),
            );
        }
    }
    Ok(main)
}
fn wait(guest: &Headless, ms: u32, deadline: Instant) -> Result<(), String> {
    let start = guest.guest_ms();
    while guest.guest_ms().wrapping_sub(start) < ms {
        if Instant::now() >= deadline {
            return Err("diagnostic wall timeout".into());
        }
        if guest
            .engine
            .inputs
            .as_ref()
            .unwrap()
            .main
            .snapshot()
            .powered_off
        {
            return Err("guest powered off".into());
        }
        guest.sample_display().map_err(|e| e.to_string())?;
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
fn connect_gdb(path: &Path, deadline: Instant) -> Result<Gdb, String> {
    let mut last_error = None;
    while Instant::now() < deadline {
        match Gdb::connect(path) {
            Ok(debugger) => return Ok(debugger),
            Err(error) => {
                last_error = Some(error);
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
    Err(format!(
        "could not connect to QEMU GDB socket {}: {}",
        path.display(),
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "diagnostic wall timeout".into())
    ))
}
fn run(recipe_path: &Path, options: Options) -> Result<Value, String> {
    let recipe = Recipe::load(recipe_path)?;
    let main = main_image(&options)?;
    let actual_model = match l6fw::model_from_main(&main)? {
        l6fw::Model::L6 => "l6",
        l6fw::Model::L6max => "l6max",
    };
    if actual_model != recipe.model {
        return Err("recipe model differs from firmware".into());
    }
    if let Some(expected) = &recipe.main_sha256 {
        if !format!("{:x}", Sha256::digest(&main)).eq_ignore_ascii_case(expected) {
            return Err("MAIN firmware does not match recipe guard".into());
        }
    }
    let needs_gdb = recipe
        .step
        .iter()
        .any(|step| matches!(step, Step::MemoryWrite { .. } | Step::FirmwareCall { .. }));
    let gdb_socket = if needs_gdb {
        Some(
            options
                .gdb_socket
                .as_deref()
                .ok_or("memory-write requires --gdb-socket")?,
        )
    } else {
        None
    };
    let guest = Headless::start(&options)?;
    let deadline = Instant::now() + Duration::from_secs(recipe.wall_timeout_s);
    let mut debugger = gdb_socket
        .map(|socket| connect_gdb(socket, deadline))
        .transpose()?;
    if debugger.is_some() {
        let management = &guest.engine.management[0];
        if !management
            .query_status()
            .map_err(|error| error.to_string())?
            .running
        {
            management.resume().map_err(|error| error.to_string())?;
        }
    }
    let mut events = Vec::new();
    let outcome = (|| -> Result<(), String> {
        for (index, step) in recipe.step.iter().enumerate() {
            if Instant::now() >= deadline {
                return Err("diagnostic wall timeout".into());
            }
            let inputs = guest.engine.inputs.as_ref().ok_or("missing inputs")?;
            let evidence = match step {
                Step::Wait { guest_ms } => {
                    wait(&guest, *guest_ms, deadline)?;
                    json!({"action":"wait","requested_ms":guest_ms})
                }
                Step::Press { target, hold_ms } => {
                    inputs.tap(*target, *hold_ms).map_err(|e| e.to_string())?;
                    wait(&guest, hold_ms.saturating_add(100), deadline)?;
                    json!({"action":"press","target":target,"hold_ms":hold_ms})
                }
                Step::Encoder { channel, steps } => {
                    inputs
                        .knob(channel - 1, *steps)
                        .map_err(|e| e.to_string())?;
                    wait(&guest, 200, deadline)?;
                    json!({"action":"encoder","channel":channel,"steps":steps})
                }
                Step::Snapshot { name } => {
                    let status = inputs.panel.snapshot();
                    let frame = guest.sample_display().map_err(|e| e.to_string())?;
                    let display_available = frame.is_some();
                    let panel = PanelData {
                        display: frame.unwrap_or([0; 1024]),
                        ..Default::default()
                    };
                    fs::write(
                        options.logs.join(format!("{name}.txt")),
                        panel.display_ascii(),
                    )
                    .map_err(|e| e.to_string())?;
                    let visible = if actual_model == "l6" { 6 } else { 8 };
                    json!({"action":"snapshot","name":name,"indicators":status.indicators,"indicator_levels":status.indicator_levels,"ring_levels":&l6max_host::indicators::ring_levels(status.indicator_levels)[..visible],"rings":&rings(status.indicators)[..visible],"display_available":display_available,"display_ascii":format!("{name}.txt")})
                }
                Step::Memory {
                    name,
                    address,
                    length,
                } => {
                    let data = guest.engine.management[0]
                        .read_memory(0, *address, *length)
                        .map_err(|e| e.to_string())?;
                    json!({"action":"memory","name":name,"address":format!("{address:#010x}"),"bytes":data})
                }
                Step::MemoryWrite {
                    name,
                    address,
                    bytes,
                } => {
                    let management = &guest.engine.management[0];
                    management.stop().map_err(|error| error.to_string())?;
                    let write_result = debugger
                        .as_mut()
                        .ok_or("missing GDB connection")?
                        .write(*address, bytes)
                        .map_err(|error| error.to_string());
                    let readback_result = management
                        .read_memory(0, *address, bytes.len())
                        .map_err(|error| error.to_string());
                    let resume_result = management.resume().map_err(|error| error.to_string());
                    write_result?;
                    let readback = readback_result?;
                    resume_result?;
                    if readback != *bytes {
                        return Err(format!(
                            "MAIN RAM write at {address:#010x} read back as {readback:02x?}, expected {bytes:02x?}"
                        ));
                    }
                    json!({"action":"memory-write","name":name,"address":format!("{address:#010x}"),"bytes":bytes,"readback":readback})
                }
                Step::FirmwareCall {
                    name,
                    address,
                    args,
                    timeout_ms,
                } => {
                    let management = &guest.engine.management[0];
                    management.stop().map_err(|error| error.to_string())?;
                    let call_result = debugger
                        .as_mut()
                        .ok_or("missing GDB connection")?
                        .call_thumb(*address, args, Duration::from_millis(*timeout_ms as u64))
                        .map_err(|error| error.to_string());
                    let resume_result = management.resume().map_err(|error| error.to_string());
                    let stop = call_result?;
                    resume_result?;
                    json!({"action":"firmware-call","name":name,"address":format!("{address:#010x}"),"args":args,"stop":stop})
                }
                Step::Trace {
                    name,
                    contains,
                    limit,
                } => {
                    let path = options.logs.join(if options.trace_sd {
                        "sdhci.trace"
                    } else {
                        "engine.trace"
                    });
                    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
                    let size = file.metadata().map_err(|e| e.to_string())?.len();
                    let offset = size.saturating_sub(2 * 1024 * 1024);
                    file.seek(SeekFrom::Start(offset))
                        .map_err(|e| e.to_string())?;
                    let mut bytes = Vec::new();
                    file.take(2 * 1024 * 1024)
                        .read_to_end(&mut bytes)
                        .map_err(|e| e.to_string())?;
                    let text = String::from_utf8_lossy(&bytes);
                    let lines: Vec<_> = text
                        .lines()
                        .filter(|line| line.contains(contains))
                        .rev()
                        .take(*limit)
                        .map(|line| line.chars().take(4096).collect::<String>())
                        .collect();
                    json!({"action":"trace","name":name,"contains":contains,"tail_truncated":offset > 0,"lines":lines.into_iter().rev().collect::<Vec<_>>()})
                }
            };
            events.push(json!({"step":index+1,"guest_ms":guest.guest_ms(),"observation":evidence}));
        }
        Ok(())
    })();
    let report = json!({"schema":1,"model":actual_model,"main_sha256":format!("{:x}", Sha256::digest(&main)),"recipe":recipe_path,"outcome":if outcome.is_ok() { "complete" } else { "error" },"failed_step":if outcome.is_err() { Some(events.len() + 1) } else { None },"error":outcome.as_ref().err(),"events":events});
    fs::write(
        options.logs.join("diagnostic.json"),
        serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    outcome?;
    Ok(report)
}
fn main() {
    if let Err(error) = execute() {
        eprintln!("l6dbg: {error}");
        std::process::exit(1);
    }
}
fn execute() -> Result<(), String> {
    let mut args = env::args_os().skip(1);
    let recipe = PathBuf::from(
        args.next()
            .ok_or("usage: l6dbg RECIPE.toml --volatile [engine options]")?,
    );
    if recipe == Path::new("input") {
        return input::run(args.collect());
    }
    if recipe == Path::new("midi") {
        return midi::run(args.collect());
    }
    if recipe == Path::new("popup-trigger") {
        return popup_trigger::run(args.collect()).map_err(|error| error.to_string());
    }
    if recipe == Path::new("uart-startup") {
        return uart_startup::run(args.collect()).map_err(|error| error.to_string());
    }
    if recipe == Path::new("--help") || recipe == Path::new("-h") {
        println!(
            "Usage: l6dbg RECIPE.toml --logs NEW_DIR (--volatile | --state-dir DIR) [engine options]\n       l6dbg input|midi|popup-trigger|uart-startup [engine options]"
        );
        return Ok(());
    }
    let rest: Vec<_> = args.collect();
    if !rest.iter().any(|a| a == "--volatile" || a == "--state-dir") {
        return Err("specify --volatile or an explicit disposable --state-dir".into());
    }
    if !rest.iter().any(|a| a == "--logs") {
        return Err("specify --logs with a new artifact directory".into());
    }
    let mut options = Options::parse_args(rest)?;
    if options.auto_setup {
        options.prepare()?;
        options.auto_setup = false;
    }
    let parent = options
        .logs
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    fs::create_dir(&options.logs).map_err(|e| {
        format!(
            "cannot create new artifact directory {}: {e}",
            options.logs.display()
        )
    })?;
    let report = run(&recipe, options.clone())?;
    let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    println!("{text}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Recipe;

    #[test]
    fn address_sensitive_observations_require_a_revision_guard() {
        let base = "version=1\nmodel='l6'\n[[step]]\naction='memory'\nname='test'\naddress=536870912\nlength=4\n";
        assert!(Recipe::parse(base).err().unwrap().contains("SHA-256"));
        let guarded = format!(
            "version=1\nmodel='l6'\nmain_sha256='{}'\n[[step]]\naction='memory'\nname='test'\naddress=536870912\nlength=4\n",
            "a".repeat(64)
        );
        assert!(Recipe::parse(&guarded).is_ok());
        assert!(Recipe::parse(&guarded.replace("length=4", "length=4097")).is_err());
    }

    #[test]
    fn rejects_invalid_controls_and_unsafe_observation_names() {
        assert!(Recipe::parse("version=1\nmodel='l6'\n[[step]]\naction='snapshot'\nname='same'\n[[step]]\naction='snapshot'\nname='same'\n").is_err());
        assert!(
            Recipe::parse(
                "version=1\nmodel='l6'\n[[step]]\naction='encoder'\nchannel=7\nsteps=1\n"
            )
            .is_err()
        );
        assert!(
            Recipe::parse("version=1\nmodel='l6'\n[[step]]\naction='snapshot'\nname='../bad'\n")
                .is_err()
        );
        assert!(
            Recipe::parse(
                "version=1\nmodel='l6'\n[[step]]\naction='trace'\nname='log'\ncontains=''\n"
            )
            .is_err()
        );
    }
}
