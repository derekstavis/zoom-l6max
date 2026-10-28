//! Declarative device-state specs applied to a simulator state directory.
//!
//! ```toml
//! version = 1
//! model = "l6max"
//!
//! [settings]
//! base = "defaults"            # or "installed": edit the active bank
//! [settings.values]
//! setup_required = 0
//! "mixer.current.channel[0].mute" = 1
//!
//! [rtc]
//! datetime = "2026-02-02T01:01:00"
//!
//! [panel]
//! programmed = true            # L6max: skip first-boot panel programming
//! ```
//!
//! Files are written while holding the same exclusive `flock` that QEMU
//! takes, so a spec is never applied to a running device.

use crate::{
    catalog::{self, Match},
    device::Device,
    nor::{self, MainNor},
    rtc::{DateTime, PanelRtc},
    settings::{FieldValue, Settings, parse_value},
};
use l6fw::Model;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::Path,
};

pub use crate::device::{MAIN_NOR_FILE, RESOURCE_NOR_FILE};
pub const PANEL_RTC_FILE: &str = "panel-rtc.bin";
pub const PANEL_FLASH_FILE: &str = "panel-flash.bin";
pub const PANEL_OPTIONS_FILE: &str = "panel-options.bin";
const PANEL_FLASH_SIZE: usize = 0x10000;
/// Panel option bytes after the main firmware's ROM programming sequence in
/// emulated L6max state; QEMU seeds `ff ff ff ff` before programming.
const PROGRAMMED_PANEL_OPTIONS: [u8; 4] = [0xff, 0xff, 0xff, 0xfe];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateSpec {
    pub version: u32,
    pub model: String,
    pub settings: Option<SettingsSpec>,
    pub rtc: Option<RtcSpec>,
    pub panel: Option<PanelSpec>,
}

#[derive(Debug, Default, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Base {
    #[default]
    Defaults,
    Installed,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsSpec {
    #[serde(default)]
    pub base: Base,
    #[serde(default)]
    pub values: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RtcSpec {
    pub datetime: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PanelSpec {
    pub programmed: bool,
}

impl StateSpec {
    pub fn parse(text: &str) -> Result<Self, String> {
        let spec: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        if spec.version != 1 {
            return Err(format!("unsupported state spec version {}", spec.version));
        }
        spec.model()?;
        if let Some(rtc) = &spec.rtc {
            DateTime::parse(&rtc.datetime)?;
        }
        if let Some(settings) = &spec.settings {
            for (path, value) in &settings.values {
                toml_value(path, value)?;
            }
        }
        Ok(spec)
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text =
            fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn model(&self) -> Result<Model, String> {
        match self.model.as_str() {
            "l6" => Ok(Model::L6),
            "l6max" => Ok(Model::L6max),
            other => Err(format!("unknown model {other:?}; use l6 or l6max")),
        }
    }
}

fn toml_value(path: &str, value: &toml::Value) -> Result<FieldValue, String> {
    match value {
        toml::Value::Integer(n) => Ok(FieldValue::Int(*n)),
        toml::Value::Boolean(b) => Ok(FieldValue::Int(i64::from(*b))),
        toml::Value::String(s) => parse_value(s),
        _ => Err(format!("{path}: use an integer, boolean or string value")),
    }
    .map_err(|e| format!("{path}: {e}"))
}

/// Open a device file and hold QEMU's exclusive lock for the returned handle.
fn open_locked(path: &Path) -> Result<fs::File, String> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(format!(
            "{} is locked; stop the emulator using this state first",
            path.display()
        ));
    }
    Ok(file)
}

/// Read a device file under lock; `None` when it is missing or empty (QEMU
/// treats an empty file as uninitialized).
pub fn read_device_file(path: &Path) -> Result<Option<Vec<u8>>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let mut file = open_locked(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    Ok((!bytes.is_empty()).then_some(bytes))
}

/// Replace a device file's contents under lock and flush them.
pub fn write_device_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = open_locked(path)?;
    let error = |e: std::io::Error| format!("cannot write {}: {e}", path.display());
    file.set_len(bytes.len() as u64).map_err(error)?;
    file.seek(SeekFrom::Start(0)).map_err(error)?;
    file.write_all(bytes).map_err(error)?;
    file.sync_all().map_err(error)
}

/// Apply a spec to a state directory, seeding missing NOR images from
/// `firmware_dir`. Returns a JSON report of what was written.
pub fn apply(
    spec: &StateSpec,
    firmware_dir: Option<&Path>,
    state_dir: &Path,
) -> Result<Value, String> {
    let model = spec.model()?;
    fs::create_dir_all(state_dir).map_err(|e| e.to_string())?;
    let mut device = Device::load(state_dir, firmware_dir)?;
    let installed = device.model()?;
    if installed != model {
        return Err(format!(
            "state spec is for {} but the installed MAIN is {}",
            catalog::model_name(model),
            catalog::model_name(installed)
        ));
    }
    let mut report = json!({
        "model": catalog::model_name(model),
        "seeded_main_nor": device.seeded_main,
        "seeded_resource_nor": device.seeded_resource,
        "versions": versions_json(&device.main),
    });
    if let Some(settings_spec) = &spec.settings {
        let (catalog, matched) = device.catalog()?;
        let mut settings = match settings_spec.base {
            Base::Defaults => Settings::defaults(&catalog),
            Base::Installed => device
                .settings(&catalog)?
                .ok_or("base = \"installed\" needs a valid settings bank")?,
        };
        for (path, value) in &settings_spec.values {
            settings.set(path, &toml_value(path, value)?)?;
        }
        let bank = device.store_settings(&settings)?;
        report["settings"] = json!({
            "catalog": catalog.system_version,
            "catalog_match": matched.as_str(),
            "base": format!("{:?}", settings_spec.base).to_lowercase(),
            "values": settings_spec.values.keys().collect::<Vec<_>>(),
            "device": catalog.bank.device.file(),
            "slot": format!("{:#x}", catalog.bank.slots[bank.slot]),
            "marker": bank.marker,
        });
        if matched == Match::Fingerprints {
            report["settings"]["note"] =
                json!("MAIN differs from the catalog image; all code fingerprints match");
        }
    }
    device.save(state_dir)?;
    let nor = &device.main;
    if let Some(rtc_spec) = &spec.rtc {
        let path = state_dir.join(PANEL_RTC_FILE);
        let mut rtc = match read_device_file(&path)? {
            Some(bytes) => PanelRtc::from_bytes(&bytes)?,
            None => PanelRtc::seed(),
        };
        let time = DateTime::parse(&rtc_spec.datetime)?;
        rtc.set_calendar(time);
        write_device_file(&path, rtc.bytes())?;
        report["rtc"] = json!(time.to_string());
    }
    if let Some(panel) = &spec.panel
        && panel.programmed
    {
        if model != Model::L6max {
            return Err("panel pre-programming is established only for L6max".into());
        }
        let mut flash = vec![0xff; PANEL_FLASH_SIZE];
        flash[..nor.panel_region().len()].copy_from_slice(nor.panel_region());
        write_device_file(&state_dir.join(PANEL_FLASH_FILE), &flash)?;
        write_device_file(
            &state_dir.join(PANEL_OPTIONS_FILE),
            &PROGRAMMED_PANEL_OPTIONS,
        )?;
        report["panel"] = json!({"programmed": true, "version": nor.versions().packaged_panel});
    }
    Ok(report)
}

pub fn versions_json(nor: &MainNor) -> Value {
    let v = nor.versions();
    json!({
        "boot": v.boot,
        "system": v.system,
        "packaged_main": v.packaged_main,
        "packaged_panel": v.packaged_panel,
        "update_marker": format!("{:?}", nor.update_marker()).to_lowercase(),
    })
}

/// Full NOR region map shared by inspection commands.
pub fn regions() -> [(&'static str, usize, usize); 6] {
    [
        ("bootloader", 0, nor::MAIN_IMAGE),
        ("main", nor::MAIN_IMAGE, nor::MAIN_TRAILER + 8),
        ("panel-package", nor::PANEL_IMAGE, nor::PANEL_REGION_END),
        ("boot-data", nor::PANEL_REGION_END, 0x1f8000),
        ("calibration", 0x1f8000, 0x1f9000),
        ("update-marker", nor::UPDATE_MARKER, nor::MAIN_NOR_SIZE),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_validate_values_and_models() {
        let spec = StateSpec::parse(
            "version = 1\nmodel = \"l6max\"\n[settings.values]\nsetup_required = 0\nrecorder_file_name = \"hex:3200\"\n[rtc]\ndatetime = \"2026-02-02T01:01:00\"\n",
        )
        .unwrap();
        assert_eq!(spec.model().unwrap(), Model::L6max);
        assert!(StateSpec::parse("version = 1\nmodel = \"l12\"\n").is_err());
        assert!(StateSpec::parse("version = 2\nmodel = \"l6\"\n").is_err());
        assert!(StateSpec::parse("version = 1\nmodel = \"l6\"\nextra = 1\n").is_err());
        assert!(
            StateSpec::parse("version = 1\nmodel = \"l6\"\n[rtc]\ndatetime = \"tomorrow\"\n")
                .is_err()
        );
    }

    #[test]
    fn device_files_are_locked_against_concurrent_writers() {
        let dir = std::env::temp_dir().join(format!("l6nor-lock-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("main-nor.bin");
        let held = open_locked(&path).unwrap();
        assert!(
            write_device_file(&path, b"x")
                .unwrap_err()
                .contains("locked")
        );
        drop(held);
        write_device_file(&path, b"x").unwrap();
        assert_eq!(read_device_file(&path).unwrap().unwrap(), b"x");
        fs::remove_dir_all(dir).unwrap();
    }
}
