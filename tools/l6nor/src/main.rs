use l6nor::{
    Device, FieldValue, PanelRtc, Settings, StateSpec,
    catalog::{self, Match},
    rtc::DateTime,
    settings::parse_value,
    state::{self, MAIN_NOR_FILE, PANEL_RTC_FILE, RESOURCE_NOR_FILE},
};
use serde_json::json;
use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

const USAGE: &str = "usage: l6nor <command> ...

PATH is a simulator state directory or a main-nor.bin file. L6 settings live
in resource-nor.bin, so L6 settings commands need the state directory.
FIRMWARE is an unpacked firmware directory or an update package.

  info PATH                         model, versions, update marker, active bank, catalog
  dump PATH                         every catalog field of the active settings bank
  get PATH FIELD...                 print catalog fields of the active bank
  set PATH FIELD=VALUE... [--base defaults|installed]
                                    commit edited settings with the firmware's bank protocol
  diff PATH PATH                    annotated difference of two active settings banks
  seed --firmware FIRMWARE STATE    create the NOR images exactly as QEMU's first boot would
  apply --spec SPEC [--firmware FIRMWARE] STATE
                                    apply a declarative state spec (see tests/smoke/states)
  rtc STATE [--set YYYY-MM-DDTHH:MM:SS]
                                    show or program the panel RTC calendar

Field paths: battery_type, midi_cc_number[3], mixer.current.efx_type,
mixer.scene-0.channel[2].mute. Values: decimal, 0x hex, or hex:BYTES.";

fn open(path: &Path) -> Result<Device, String> {
    Ok(Device::open(path)?.0)
}

fn settings_of(device: &Device) -> Result<(Settings, Match), String> {
    let (catalog, matched) = device.catalog()?;
    let settings = device
        .settings(&catalog)?
        .ok_or("no valid settings bank; the firmware would load defaults")?;
    Ok((settings, matched))
}

/// Write a device back to where `open` read it from.
fn persist(device: &Device, path: &Path) -> Result<(), String> {
    if path.is_dir() {
        device.save(path)
    } else {
        state::write_device_file(path, device.main.bytes())
    }
}

/// Resolve FIRMWARE to an unpacked directory, unpacking a package into a
/// temporary directory that the returned guard removes.
struct Firmware {
    dir: PathBuf,
    temporary: bool,
}

impl Firmware {
    fn open(path: &Path) -> Result<Self, String> {
        if path.is_dir() {
            return Ok(Self {
                dir: path.into(),
                temporary: false,
            });
        }
        let dir = env::temp_dir().join(format!("l6nor-firmware-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        l6fw::unpack(path, &dir)?;
        Ok(Self {
            dir,
            temporary: true,
        })
    }
}

impl Drop for Firmware {
    fn drop(&mut self) {
        if self.temporary {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
}

fn take_option(args: &mut Vec<OsString>, name: &str) -> Result<Option<OsString>, String> {
    match args.iter().position(|a| a == name) {
        None => Ok(None),
        Some(i) if i + 1 < args.len() => {
            let value = args.remove(i + 1);
            args.remove(i);
            Ok(Some(value))
        }
        Some(_) => Err(format!("{name} needs a value")),
    }
}

fn text(arg: &OsString) -> Result<&str, String> {
    arg.to_str()
        .ok_or_else(|| format!("argument is not UTF-8: {arg:?}"))
}

fn print_json(value: &serde_json::Value) {
    println!("{}", serde_json::to_string_pretty(value).unwrap());
}

fn info(path: &Path) -> Result<(), String> {
    let device = open(path)?;
    let nor = &device.main;
    let model = nor.model()?;
    let mut report = json!({
        "model": catalog::model_name(model),
        "versions": state::versions_json(nor),
        "regions": state::regions().iter().map(|(name, start, end)| json!({
            "name": name,
            "range": format!("{start:#08x}-{end:#08x}"),
            "erased": nor.bytes()[*start..*end].iter().all(|&b| b == 0xff),
        })).collect::<Vec<_>>(),
    });
    match device.catalog() {
        Ok((catalog, matched)) => {
            report["settings"] = json!({
                "catalog": catalog.system_version,
                "catalog_match": matched.as_str(),
                "device": catalog.bank.device.file(),
            });
            match device.active_bank(&catalog) {
                Ok(active) => {
                    report["settings"]["active_slot"] =
                        json!(active.map(|a| format!("{:#x}", catalog.bank.slots[a.slot])));
                    report["settings"]["marker"] = json!(active.map(|a| a.marker));
                }
                Err(reason) => report["settings"]["unavailable"] = json!(reason),
            }
            if let Ok(Some(settings)) = device.settings(&catalog) {
                for name in [
                    "setup_required",
                    "battery_type",
                    "auto_power_off",
                    "saved_scenes",
                ] {
                    if let Ok(value) = settings.get(name) {
                        report["settings"][name] = json!(value.to_string());
                    }
                }
            }
        }
        Err(reason) => report["settings"] = json!({ "unavailable": reason }),
    }
    let rtc = path.is_dir().then(|| path.join(PANEL_RTC_FILE));
    if let Some(bytes) = rtc
        .as_deref()
        .map(state::read_device_file)
        .transpose()?
        .flatten()
    {
        report["rtc"] = json!(
            PanelRtc::from_bytes(&bytes)?
                .calendar()
                .map(|t| t.to_string())
        );
    }
    print_json(&report);
    Ok(())
}

fn set(path: &Path, mut args: Vec<OsString>) -> Result<(), String> {
    let base = take_option(&mut args, "--base")?;
    let mut device = open(path)?;
    let (catalog, matched) = device.catalog()?;
    let mut settings = match base.as_ref().map(text).transpose()? {
        Some("defaults") => Settings::defaults(&catalog),
        Some("installed") | None => device.settings(&catalog)?.ok_or(
            "no valid settings bank; pass --base defaults to start from firmware defaults",
        )?,
        Some(other) => return Err(format!("unknown base {other}")),
    };
    if args.is_empty() {
        return Err("set needs FIELD=VALUE arguments".into());
    }
    for arg in &args {
        let (field, value) = text(arg)?
            .split_once('=')
            .ok_or_else(|| format!("expected FIELD=VALUE, got {arg:?}"))?;
        settings.set(field, &parse_value(value)?)?;
    }
    let bank = device.store_settings(&settings)?;
    persist(&device, path)?;
    eprintln!(
        "committed {} bank at {:#x}, marker {} (catalog {}, {} match)",
        catalog.bank.device.file(),
        catalog.bank.slots[bank.slot],
        bank.marker,
        catalog.system_version,
        matched.as_str()
    );
    Ok(())
}

fn diff(a: &Path, b: &Path) -> Result<(), String> {
    let (left, _) = settings_of(&open(a)?)?;
    let (right, _) = settings_of(&open(b)?)?;
    if left.catalog().model != right.catalog().model
        || left.catalog().system_version != right.catalog().system_version
    {
        return Err("banks use different catalogs".into());
    }
    let (l, r) = (left.payload(), right.payload());
    let show = |value: FieldValue| match value {
        FieldValue::Bytes(bytes) if bytes.len() > 16 => format!("<{} bytes>", bytes.len()),
        value => value.to_string(),
    };
    let mut covered = vec![false; l.len()];
    let mut differences = 0;
    for (path, offset, value) in left.entries() {
        let size = left.locate(&path)?.field.size;
        covered[offset..offset + size].fill(true);
        let other = right.get(&path)?;
        if other != value {
            differences += 1;
            println!("{offset:#06x}\t{path}\t{} -> {}", show(value), show(other));
        }
    }
    // Bytes outside every catalog element are reported as raw ranges.
    let mut offset = 0;
    while offset < l.len() {
        if covered[offset] || l[offset] == r[offset] {
            offset += 1;
            continue;
        }
        let start = offset;
        while offset < l.len() && !covered[offset] && l[offset] != r[offset] {
            offset += 1;
        }
        differences += 1;
        let hex = |bytes: &[u8]| show(FieldValue::Bytes(bytes.to_vec()));
        println!(
            "{start:#06x}\tunclassified[{}]\t{} -> {}",
            offset - start,
            hex(&l[start..offset]),
            hex(&r[start..offset])
        );
    }
    eprintln!("{differences} differences");
    Ok(())
}

fn run() -> Result<(), String> {
    let mut args: Vec<OsString> = env::args_os().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        println!("{USAGE}");
        return Ok(());
    }
    let command = args.remove(0);
    match (text(&command)?, args.as_slice()) {
        ("info", [path]) => info(Path::new(path)),
        ("dump", [path]) => {
            let (settings, matched) = settings_of(&open(Path::new(path))?)?;
            eprintln!(
                "catalog {} ({} match)",
                settings.catalog().system_version,
                matched.as_str()
            );
            for (path, offset, value) in settings.entries() {
                println!("{offset:#06x}\t{path}\t{value}");
            }
            Ok(())
        }
        ("get", [path, fields @ ..]) if !fields.is_empty() => {
            let (settings, _) = settings_of(&open(Path::new(path))?)?;
            for field in fields {
                println!("{}", settings.get(text(field)?)?);
            }
            Ok(())
        }
        ("set", [path, rest @ ..]) => set(Path::new(path), rest.to_vec()),
        ("diff", [a, b]) => diff(Path::new(a), Path::new(b)),
        ("seed", _) => {
            let firmware = take_option(&mut args, "--firmware")?.ok_or("seed needs --firmware")?;
            let [state_dir] = args.as_slice() else {
                return Err("seed needs one STATE directory".into());
            };
            let firmware = Firmware::open(Path::new(&firmware))?;
            let state_dir = Path::new(state_dir);
            for file in [MAIN_NOR_FILE, RESOURCE_NOR_FILE] {
                if state_dir.join(file).exists() {
                    return Err(format!("{} already has {file}", state_dir.display()));
                }
            }
            fs::create_dir_all(state_dir).map_err(|e| e.to_string())?;
            let device = Device::load(state_dir, Some(&firmware.dir))?;
            device.save(state_dir)?;
            print_json(&state::versions_json(&device.main));
            Ok(())
        }
        ("apply", _) => {
            let spec = take_option(&mut args, "--spec")?.ok_or("apply needs --spec")?;
            let firmware = take_option(&mut args, "--firmware")?
                .map(|path| Firmware::open(Path::new(&path)))
                .transpose()?;
            let [state_dir] = args.as_slice() else {
                return Err("apply needs one STATE directory".into());
            };
            let spec = StateSpec::load(Path::new(&spec))?;
            let report = state::apply(
                &spec,
                firmware.as_ref().map(|f| f.dir.as_path()),
                Path::new(state_dir),
            )?;
            print_json(&report);
            Ok(())
        }
        ("rtc", _) => {
            let time = take_option(&mut args, "--set")?;
            let [state_dir] = args.as_slice() else {
                return Err("rtc needs one STATE directory".into());
            };
            let path = Path::new(state_dir).join(PANEL_RTC_FILE);
            let mut rtc = match state::read_device_file(&path)? {
                Some(bytes) => PanelRtc::from_bytes(&bytes)?,
                None => PanelRtc::seed(),
            };
            if let Some(time) = time {
                rtc.set_calendar(DateTime::parse(text(&time)?)?);
                state::write_device_file(&path, rtc.bytes())?;
            }
            match rtc.calendar() {
                Some(time) => println!("{time}"),
                None => println!("invalid (the firmware will ask for Date/Time)"),
            }
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
