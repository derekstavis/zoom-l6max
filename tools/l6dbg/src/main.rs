//! Scripted, observational diagnostics using the production headless engine.
mod input;
mod midi;
mod popup_trigger;
mod usb;
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
    AudioInputTransport {
        command: AudioInputCommand,
        #[serde(default)]
        position_ms: i32,
    },
    Press {
        #[serde(default)]
        target: Option<u32>,
        #[serde(default)]
        control: Option<String>,
        #[serde(default = "hold")]
        hold_ms: u32,
    },
    Button {
        #[serde(default)]
        target: Option<u32>,
        #[serde(default)]
        control: Option<String>,
        pressed: bool,
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
    Heap {
        name: String,
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
    UsbDescriptors {
        name: String,
        #[serde(default = "usb_timeout_ms")]
        guest_timeout_ms: u32,
    },
    UsbControl {
        name: String,
        setup: [u8; 8],
    },
    UsbMidi {
        name: String,
        #[serde(default)]
        cable: u8,
        #[serde(default)]
        send: Vec<u8>,
        #[serde(default = "listen_ms")]
        listen_ms: u32,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum AudioInputCommand {
    Play,
    Pause,
    Stop,
    Seek,
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
fn usb_timeout_ms() -> u32 {
    60_000
}
fn listen_ms() -> u32 {
    1_000
}

impl Recipe {
    fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
        Self::parse(&text)
    }
    fn parse(text: &str) -> Result<Self, String> {
        let mut recipe: Self = toml::from_str(&text).map_err(|e| e.to_string())?;
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
        let model = match recipe.model.as_str() {
            "l6" => l6fw::Model::L6,
            _ => l6fw::Model::L6max,
        };
        for step in &mut recipe.step {
            if let Step::Press {
                target, control, ..
            }
            | Step::Button {
                target, control, ..
            } = step
            {
                *target = Some(match (*target, control.as_deref()) {
                    (Some(target), None) => target,
                    (None, Some(control)) => l6max_host::controls::control_target(model, control)?,
                    _ => return Err("input steps need exactly one of target or control".into()),
                });
            }
        }
        let mut names = BTreeSet::new();
        let mut held_targets = BTreeSet::new();
        for step in &recipe.step {
            validate_input(step, &recipe.model, &mut held_targets)?;
            if let Step::Snapshot { name }
            | Step::Memory { name, .. }
            | Step::MemoryWrite { name, .. }
            | Step::Heap { name }
            | Step::FirmwareCall { name, .. }
            | Step::Trace { name, .. }
            | Step::UsbDescriptors { name, .. }
            | Step::UsbControl { name, .. }
            | Step::UsbMidi { name, .. } = step
            {
                if !names.insert(name) {
                    return Err("observation names must be unique".into());
                }
            }
            match step {
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
                Step::Heap { name } => {
                    validate_name(name)?;
                    if recipe.model != "l6max" {
                        return Err("heap counters are available only for L6max".into());
                    }
                }
                Step::UsbDescriptors {
                    name,
                    guest_timeout_ms,
                } => {
                    validate_name(name)?;
                    if *guest_timeout_ms == 0 || *guest_timeout_ms > 600_000 {
                        return Err(
                            "usb-descriptors timeout must be 1..600000 guest milliseconds".into(),
                        );
                    }
                }
                Step::UsbControl { name, setup } => {
                    validate_name(name)?;
                    if setup[0] & 0x80 == 0 && (setup[6] != 0 || setup[7] != 0) {
                        return Err(
                            "usb-control supports IN requests and OUT requests without data".into(),
                        );
                    }
                }
                Step::UsbMidi {
                    name,
                    cable,
                    send,
                    listen_ms,
                } => {
                    validate_name(name)?;
                    if *cable > 15 || send.len() > 4096 || *listen_ms > 60_000 {
                        return Err("usb-midi needs cable 0..15, at most 4096 bytes and listen_ms up to 60000".into());
                    }
                }
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
fn validate_input(step: &Step, model: &str, held: &mut BTreeSet<u32>) -> Result<(), String> {
    match step {
        Step::Wait { guest_ms } if *guest_ms == 0 || *guest_ms > 600_000 => {
            Err("wait must be 1..600000 guest milliseconds".into())
        }
        Step::AudioInputTransport {
            command,
            position_ms,
        } if *position_ms < 0
            || (!matches!(command, AudioInputCommand::Seek) && *position_ms != 0) =>
        {
            Err("audio input position must be nonnegative and is only valid for seek".into())
        }
        Step::Press {
            target: Some(target),
            hold_ms,
            ..
        } if *target >= 54 || *hold_ms == 0 || *hold_ms > 60_000 => {
            Err("invalid input target or hold".into())
        }
        Step::Button {
            target: Some(target),
            pressed,
            ..
        } => validate_button(held, *target, *pressed),
        Step::Encoder { channel, steps }
            if *channel == 0
                || *channel > if model == "l6" { 6 } else { 8 }
                || *steps == 0
                || steps.unsigned_abs() > 1000 =>
        {
            Err("invalid encoder channel or steps".into())
        }
        _ => Ok(()),
    }
}
fn validate_button(held: &mut BTreeSet<u32>, target: u32, pressed: bool) -> Result<(), String> {
    if target >= 54 {
        return Err("invalid input target".into());
    }
    let changed = if pressed {
        held.insert(target)
    } else {
        held.remove(&target)
    };
    if !changed {
        return Err("input target is already in that state".into());
    }
    Ok(())
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
/// Enumerate the device once the firmware's controller runs, then read every
/// string it references plus the optional descriptors a host may request.
fn usb_descriptors(
    guest: &Headless,
    options: &Options,
    name: &str,
    guest_timeout_ms: u32,
    deadline: Instant,
) -> Result<(Value, l6max_host::usb::Configuration), String> {
    let usb = guest.engine.usb.as_ref().ok_or("USB backend unavailable")?;
    usb.connect(true).map_err(|e| e.to_string())?;
    let start = guest.guest_ms();
    while !usb.controller_running().map_err(|e| e.to_string())? {
        if Instant::now() >= deadline || guest.guest_ms().wrapping_sub(start) > guest_timeout_ms {
            return Err("USB device controller did not start".into());
        }
        wait(guest, 25, deadline)?;
    }
    let (device, config, configuration) = usb
        .enumerate_with_descriptors()
        .map_err(|e| e.to_string())?;
    fs::write(options.logs.join(format!("{name}-device.bin")), &device)
        .map_err(|e| e.to_string())?;
    fs::write(options.logs.join(format!("{name}-config.bin")), &config)
        .map_err(|e| e.to_string())?;
    let decoded_device = usb::device(&device)?;
    let decoded = usb::configuration(&config)?;
    let mut indices: BTreeSet<u8> = decoded["strings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_u64().map(|v| v as u8))
        .collect();
    indices.extend(
        [14, 15, 16]
            .iter()
            .map(|&offset| device[offset])
            .filter(|&index| index != 0),
    );
    let request = |setup: [u8; 8]| match usb.control(setup) {
        Ok(data) => json!({"bytes":data}),
        Err(error) => json!({"error":error.to_string()}),
    };
    let languages = request([0x80, 6, 0, 3, 0, 0, 0xff, 0]);
    let language = languages["bytes"]
        .as_array()
        .filter(|bytes| bytes.len() >= 4)
        .map(|bytes| {
            [
                bytes[2].as_u64().unwrap() as u8,
                bytes[3].as_u64().unwrap() as u8,
            ]
        })
        .unwrap_or([0x09, 0x04]);
    let strings: serde_json::Map<String, Value> = indices
        .iter()
        .map(|&index| {
            let setup = [0x80, 6, index, 3, language[0], language[1], 0xff, 0];
            let value = match usb.control(setup) {
                Ok(bytes) => usb::string(&bytes)
                    .map(Value::from)
                    .unwrap_or_else(|e| json!({"error":e})),
                Err(error) => json!({"error":error.to_string()}),
            };
            (index.to_string(), value)
        })
        .collect();
    Ok((
        json!({
            "action":"usb-descriptors",
            "name":name,
            "device":decoded_device,
            "configuration":decoded,
            "languages":languages,
            "strings":strings,
            "device_qualifier":request([0x80, 6, 0, 6, 0, 0, 10, 0]),
            "other_speed_configuration":request([0x80, 6, 0, 7, 0, 0, 0xff, 0]),
            "bos":request([0x80, 6, 0, 0x0f, 0, 0, 0xff, 0]),
            "device_file":format!("{name}-device.bin"),
            "configuration_file":format!("{name}-config.bin"),
        }),
        configuration,
    ))
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
    let mut midi = None;
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
                Step::AudioInputTransport {
                    command,
                    position_ms,
                } => {
                    use l6max_host::protocol;
                    let (name, target) = match command {
                        AudioInputCommand::Play => ("play", protocol::AUDIO_INPUT_PLAY),
                        AudioInputCommand::Pause => ("pause", protocol::AUDIO_INPUT_PAUSE),
                        AudioInputCommand::Stop => ("stop", protocol::AUDIO_INPUT_STOP),
                        AudioInputCommand::Seek => ("seek", protocol::AUDIO_INPUT_SEEK),
                    };
                    let sequence = inputs
                        .main
                        .audio_input_transport(target, *position_ms)
                        .map_err(|error| error.to_string())?;
                    loop {
                        if let Some(ack) = inputs
                            .main
                            .snapshot()
                            .last_ack
                            .filter(|ack| ack.sequence == sequence)
                        {
                            if ack.kind != protocol::APPLIED {
                                return Err(format!(
                                    "audio input transport rejected: {}",
                                    ack.value
                                ));
                            }
                            break;
                        }
                        if Instant::now() >= deadline {
                            return Err("audio input transport was not acknowledged".into());
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                    json!({"action":"audio-input-transport","command":name,"position_ms":position_ms,"sequence":sequence})
                }
                Step::Press {
                    target: Some(target),
                    hold_ms,
                    ..
                } => {
                    inputs.tap(*target, *hold_ms).map_err(|e| e.to_string())?;
                    wait(&guest, hold_ms.saturating_add(100), deadline)?;
                    json!({"action":"press","target":target,"hold_ms":hold_ms})
                }
                Step::Button {
                    target: Some(target),
                    pressed,
                    ..
                } => {
                    if *pressed {
                        inputs.down(*target, 50)
                    } else {
                        inputs.up(*target)
                    }
                    .map_err(|e| e.to_string())?;
                    wait(&guest, 200, deadline)?;
                    json!({"action":"button","target":target,"pressed":pressed})
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
                    let display_raw = if display_available {
                        let filename = format!("{name}.oled");
                        fs::write(options.logs.join(&filename), panel.display)
                            .map_err(|e| e.to_string())?;
                        Some(filename)
                    } else {
                        None
                    };
                    let visible = if actual_model == "l6" { 6 } else { 8 };
                    json!({"action":"snapshot","name":name,"indicators":status.indicators,"indicator_levels":status.indicator_levels,"ring_levels":&l6max_host::indicators::ring_levels(status.indicator_levels)[..visible],"rings":&rings(status.indicators)[..visible],"display_available":display_available,"display_ascii":format!("{name}.txt"),"display_raw":display_raw})
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
                Step::Heap { name } => {
                    let heap = l6max_host::heap::read_l6max(&guest.engine.management[0])?;
                    json!({"action":"heap","name":name,"heap":heap,"peak_used":heap.peak_used()})
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
                Step::Press { target: None, .. } | Step::Button { target: None, .. } => {
                    unreachable!("recipe parsing resolves input targets")
                }
                Step::UsbDescriptors {
                    name,
                    guest_timeout_ms,
                } => {
                    let (evidence, configuration) =
                        usb_descriptors(&guest, &options, name, *guest_timeout_ms, deadline)?;
                    midi = configuration.midi;
                    evidence
                }
                Step::UsbMidi {
                    name,
                    cable,
                    send,
                    listen_ms,
                } => {
                    let usb = guest.engine.usb.as_ref().ok_or("USB backend unavailable")?;
                    let endpoints =
                        midi.ok_or("usb-midi needs a preceding usb-descriptors step")?;
                    let packets = l6max_host::usb_midi::Encoder::default().push(*cable, send);
                    if !packets.is_empty() {
                        usb.send(endpoints.output, &packets)
                            .map_err(|e| e.to_string())?;
                    }
                    let start = guest.guest_ms();
                    let mut received = Vec::new();
                    // Mode setters restart the firmware's USB device; the
                    // detach ends the listening window.
                    let mut detached = false;
                    while guest.guest_ms().wrapping_sub(start) < *listen_ms {
                        match usb.try_receive(endpoints.input, 16384) {
                            Ok(bytes) => received.extend(bytes),
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                wait(&guest, 5, deadline)?
                            }
                            Err(e) if e.raw_os_error() == Some(libc::ENODEV) => {
                                detached = true;
                                break;
                            }
                            Err(e) => return Err(e.to_string()),
                        }
                    }
                    let messages: Vec<_> = l6max_host::usb_midi::decode(&received)
                        .map_err(|e| e.to_string())?
                        .into_iter()
                        .map(|(cable, bytes)| json!({"cable":cable,"bytes":bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()}))
                        .collect();
                    json!({"action":"usb-midi","name":name,"cable":cable,"sent":send.len(),"received":messages,"detached":detached})
                }
                Step::UsbControl { name, setup } => {
                    let usb = guest.engine.usb.as_ref().ok_or("USB backend unavailable")?;
                    let (data, error) = match usb.control(*setup) {
                        Ok(data) => (Some(data), None),
                        Err(error) => (None, Some(error.to_string())),
                    };
                    json!({"action":"usb-control","name":name,"setup":setup,"data":data,"error":error})
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
    if recipe == Path::new("--help") || recipe == Path::new("-h") {
        println!(
            "Usage: l6dbg RECIPE.toml --logs NEW_DIR (--volatile | --state-dir DIR) [engine options]\n       l6dbg input|midi|popup-trigger [engine options]"
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
    fn audio_input_transport_validates_commands_and_positions() {
        let header = "version=1\nmodel='l6max'\n[[step]]\naction='audio-input-transport'\n";
        for command in ["play", "pause", "stop", "seek"] {
            assert!(Recipe::parse(&format!("{header}command='{command}'\n")).is_ok());
        }
        assert!(Recipe::parse(&format!("{header}command='seek'\nposition_ms=120\n")).is_ok());
        assert!(Recipe::parse(&format!("{header}command='seek'\nposition_ms=-1\n")).is_err());
        assert!(Recipe::parse(&format!("{header}command='play'\nposition_ms=1\n")).is_err());
        assert!(Recipe::parse(&format!("{header}command='rewind'\n")).is_err());
    }

    #[test]
    fn checked_in_recipes_parse() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut paths = vec![dir.join("example.toml")];
        for entry in std::fs::read_dir(dir.join("recipes")).unwrap() {
            paths.push(entry.unwrap().path());
        }
        for path in paths {
            Recipe::load(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        }
    }

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

    #[test]
    fn held_inputs_require_valid_order_and_targets() {
        let header = "version=1\nmodel='l6max'\n";
        let down = "[[step]]\naction='button'\ntarget=47\npressed=true\n";
        let up = "[[step]]\naction='button'\ntarget=47\npressed=false\n";
        assert!(Recipe::parse(&format!("{header}{down}{up}")).is_ok());
        assert!(Recipe::parse(&format!("{header}{down}")).is_ok());
        assert!(Recipe::parse(&format!("{header}{up}")).is_err());
        assert!(Recipe::parse(&format!("{header}{down}{down}")).is_err());
        assert!(Recipe::parse(&format!("{header}{}", down.replace("47", "54"))).is_err());
    }

    #[test]
    fn inputs_accept_semantic_control_names() {
        let header = "version=1\nmodel='l6max'\n[[step]]\naction='press'\n";
        assert!(Recipe::parse(&format!("{header}control='recorder.confirm'\n")).is_ok());
        assert!(Recipe::parse(&format!("{header}control='no-such-control'\n")).is_err());
        assert!(Recipe::parse(&format!("{header}target=3\ncontrol='recorder.confirm'\n")).is_err());
        assert!(Recipe::parse(header).is_err());
        let held = "version=1\nmodel='l6max'\n[[step]]\naction='button'\ncontrol='recorder.menu'\npressed=true\n[[step]]\naction='button'\ntarget=0\npressed=false\n";
        assert!(Recipe::parse(held).is_ok());
    }

    #[test]
    fn heap_needs_no_hash_but_only_runs_on_l6max() {
        let step = "[[step]]\naction='heap'\nname='idle'\n";
        assert!(Recipe::parse(&format!("version=1\nmodel='l6max'\n{step}")).is_ok());
        assert!(Recipe::parse(&format!("version=1\nmodel='l6'\n{step}")).is_err());
    }

    #[test]
    fn usb_steps_validate_requests() {
        let header = "version=1\nmodel='l6max'\n[[step]]\n";
        assert!(
            Recipe::parse(&format!(
                "{header}action='usb-descriptors'\nname='normal'\n"
            ))
            .is_ok()
        );
        assert!(
            Recipe::parse(&format!(
                "{header}action='usb-descriptors'\nname='normal'\nguest_timeout_ms=0\n"
            ))
            .is_err()
        );
        assert!(
            Recipe::parse(&format!(
                "{header}action='usb-control'\nname='q'\nsetup=[128,6,0,6,0,0,10,0]\n"
            ))
            .is_ok()
        );
        assert!(
            Recipe::parse(&format!(
                "{header}action='usb-control'\nname='q'\nsetup=[0,9,1,0,0,0,1,0]\n"
            ))
            .is_err()
        );
        assert!(
            Recipe::parse(&format!(
                "{header}action='usb-midi'\nname='id'\ncable=2\nsend=[240,126,0,6,1,247]\n"
            ))
            .is_ok()
        );
        assert!(
            Recipe::parse(&format!("{header}action='usb-midi'\nname='id'\ncable=16\n")).is_err()
        );
    }
}
