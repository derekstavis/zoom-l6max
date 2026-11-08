//! Applies precompiled, Zoom-byte-free patch templates (see
//! `firmware-patch-wasm-prebuild`, embedded at build time from `templates/`)
//! to a user-uploaded Zoom update package, entirely in memory. Compiles for
//! `wasm32-unknown-unknown` (the browser) and for the host (native tests,
//! which check this crate's output against the native `l6patch` CLI's
//! own output byte-for-byte). No filesystem access, no network, no
//! cross-compilation: every byte this crate needs is either embedded at
//! build time (our own code, verified Zoom-byte-free) or supplied by the
//! caller (the uploaded package).
include!(concat!(env!("OUT_DIR"), "/templates_generated.rs"));

use firmware_patch_core::{
    FirmwareModel, HookEdit, L6_MAIN_BASE, L6_SLOT_END, PATCHES, PatchEdits, apply_patch_with_map,
    branch, first_payload_for, verify_firmware_for, write_image_length_and_version,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use wasm_bindgen::prelude::*;

/// Update-version marker for packages without the Patch loader. Loader-enabled
/// packages use marker 0114 so the device can distinguish that layout from
/// ordinary feature-only updates. Both are distinct from the native CLI's
/// dev-build markers (0112/0113).
const UPDATE_VERSION: &str = "0111";

struct Window {
    payload_offset: usize,
    firmware_address: u32,
    length: usize,
}

struct Unit {
    model: FirmwareModel,
    base: u32,
    template: Vec<u8>,
    windows: Vec<Window>,
    hooks: Vec<HookEdit>,
    hook_symbols: Vec<String>,
}

struct LoaderSite {
    address: u32,
    length: usize,
    guard_sha256: String,
    target_offset: u32,
    link: bool,
}

struct LoaderUnit {
    model: FirmwareModel,
    base: u32,
    module_base: u32,
    reserve_end: u32,
    static_patches: Vec<String>,
    template: Vec<u8>,
    windows: Vec<Window>,
    sites: Vec<LoaderSite>,
}

fn parse_hex(value: &Value) -> Result<u32, String> {
    let text = value.as_str().ok_or("expected a hex string")?;
    u32::from_str_radix(text.trim_start_matches("0x"), 16).map_err(|e| e.to_string())
}

fn parse_unit(payload: &[u8], metadata_json: &str) -> Result<Unit, String> {
    let meta: Value = serde_json::from_str(metadata_json).map_err(|e| e.to_string())?;
    let model_name = meta["model"]
        .as_str()
        .ok_or("template metadata missing model")?;
    let model = match model_name {
        "L6" => FirmwareModel::L6,
        "L6max" => FirmwareModel::L6max,
        other => return Err(format!("unknown firmware model in template: {other}")),
    };
    let base = parse_hex(&meta["base"])?;
    let windows = meta["windows"]
        .as_array()
        .ok_or("metadata missing windows")?
        .iter()
        .map(|w| {
            Ok(Window {
                payload_offset: w["payload_offset"]
                    .as_u64()
                    .ok_or("window missing payload_offset")?
                    as usize,
                firmware_address: parse_hex(&w["firmware_address"])?,
                length: w["length"].as_u64().ok_or("window missing length")? as usize,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let hooks = meta["hooks"]
        .as_array()
        .ok_or("metadata missing hooks")?
        .iter()
        .map(|h| {
            Ok(HookEdit {
                address: parse_hex(&h["address"])?,
                target: parse_hex(&h["target"])?,
                length: h["length"].as_u64().ok_or("hook missing length")? as usize,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let hook_symbols = meta["hooks"]
        .as_array()
        .ok_or("metadata missing hooks")?
        .iter()
        .map(|h| {
            h["symbol"]
                .as_str()
                .map(str::to_owned)
                .ok_or("hook missing symbol".to_owned())
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Unit {
        model,
        base,
        template: payload.to_vec(),
        windows,
        hooks,
        hook_symbols,
    })
}

/// Finds the one embedded unit for `name` at exactly `base` — every
/// non-empty subset of `PATCHES` resolves to one of the 7 embedded units, so
/// this only fails if the embedded templates are stale or corrupt.
fn find_unit(name: &str, model: FirmwareModel, base: u32) -> Result<Unit, String> {
    for (unit_name, payload, metadata) in EMBEDDED_UNITS {
        if *unit_name == name {
            let unit = parse_unit(payload, metadata)?;
            if unit.model == model && unit.base == base {
                return Ok(unit);
            }
        }
    }
    Err(format!(
        "no precompiled template for {name} at base {base:#x} (stale or corrupt embedded templates)"
    ))
}

fn parse_loader(payload: &[u8], metadata_json: &str) -> Result<LoaderUnit, String> {
    let meta: Value = serde_json::from_str(metadata_json).map_err(|e| e.to_string())?;
    let model = match meta["model"]
        .as_str()
        .ok_or("loader metadata missing model")?
    {
        "L6" => FirmwareModel::L6,
        "L6max" => FirmwareModel::L6max,
        other => {
            return Err(format!(
                "unknown firmware model in loader template: {other}"
            ));
        }
    };
    let windows = meta["windows"]
        .as_array()
        .ok_or("loader metadata missing windows")?
        .iter()
        .map(|w| {
            Ok(Window {
                payload_offset: w["payload_offset"]
                    .as_u64()
                    .ok_or("loader window missing payload_offset")?
                    as usize,
                firmware_address: parse_hex(&w["firmware_address"])?,
                length: w["length"].as_u64().ok_or("loader window missing length")? as usize,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let sites = meta["sites"]
        .as_array()
        .ok_or("loader metadata missing sites")?
        .iter()
        .map(|s| {
            Ok(LoaderSite {
                address: parse_hex(&s["address"])?,
                length: s["length"].as_u64().ok_or("loader site missing length")? as usize,
                guard_sha256: s["guard_sha256"]
                    .as_str()
                    .ok_or("loader site missing guard hash")?
                    .to_owned(),
                target_offset: s["target_offset"]
                    .as_u64()
                    .ok_or("loader site missing target offset")?
                    as u32,
                link: s["link"]
                    .as_bool()
                    .ok_or("loader site missing branch kind")?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(LoaderUnit {
        model,
        base: parse_hex(&meta["base"])?,
        module_base: parse_hex(&meta["module_base"])?,
        reserve_end: parse_hex(&meta["reserve_end"])?,
        static_patches: meta["static_patches"]
            .as_array()
            .ok_or("loader metadata missing static patches")?
            .iter()
            .map(|name| {
                name.as_str()
                    .map(str::to_owned)
                    .ok_or("invalid loader patch name".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?,
        template: payload.to_vec(),
        windows,
        sites,
    })
}

fn find_loader(model: FirmwareModel, base: u32, selected: &[&str]) -> Result<LoaderUnit, String> {
    for (loader_model, payload, metadata) in EMBEDDED_LOADERS {
        if *loader_model
            == if model == FirmwareModel::L6 {
                "L6"
            } else {
                "L6max"
            }
        {
            let loader = parse_loader(payload, metadata)?;
            if loader.model == model
                && loader.base == base
                && loader
                    .static_patches
                    .iter()
                    .map(String::as_str)
                    .eq(selected.iter().copied())
            {
                return Ok(loader);
            }
        }
    }
    Err(format!(
        "no Patch loader template for {model:?} at {base:#x}"
    ))
}

/// The patches that apply to `package`, or why none do.
pub fn available_patches(package: &[u8]) -> Result<Vec<&'static str>, String> {
    let model = match l6fw::model_from_package(package)? {
        l6fw::Model::L6 => FirmwareModel::L6,
        l6fw::Model::L6max => FirmwareModel::L6max,
    };
    let (_, segments) = l6fw::unpack_bytes(package)?;
    let firmware = segments
        .get("main_firmware.bin")
        .ok_or("package has no main_firmware.bin segment")?;
    verify_firmware_for(model, firmware)?;
    Ok(match model {
        FirmwareModel::L6 => vec!["midi-sync"],
        FirmwareModel::L6max => PATCHES.to_vec(),
    })
}

/// Applies `selected` (a non-empty subset of `firmware_patch_core::PATCHES`,
/// any order) to `package` — a full Zoom update package, byte for byte what
/// `l6fw::unpack`/`repack` round-trip — and returns the patched package plus
/// a small JSON report. `package` is the only untrusted input: every other
/// byte comes from the embedded, Zoom-byte-free templates.
pub fn apply_patches(package: &[u8], selected: &[&str]) -> Result<(Vec<u8>, Value), String> {
    apply_patches_with_loader(package, selected, false)
}

/// Applies static feature patches and optionally installs the persistent SD
/// loader. When enabled, the loader is written first so feature payloads can
/// follow it in the same image, matching the native builder's layout.
pub fn apply_patches_with_loader(
    package: &[u8],
    selected: &[&str],
    install_patch_loader: bool,
) -> Result<(Vec<u8>, Value), String> {
    let model = match l6fw::model_from_package(package)? {
        l6fw::Model::L6 => FirmwareModel::L6,
        l6fw::Model::L6max => FirmwareModel::L6max,
    };
    for name in selected {
        if !PATCHES.contains(name) {
            return Err(format!("unknown patch: {name}"));
        }
    }
    let ordered: Vec<&str> = PATCHES
        .into_iter()
        .filter(|p| selected.contains(p))
        .collect();
    if ordered.is_empty() && !install_patch_loader {
        return Err("select at least one patch".into());
    }
    if model == FirmwareModel::L6 && ordered.iter().any(|name| *name != "midi-sync") {
        return Err("L6 firmware detected; only the midi-sync patch is supported".into());
    }

    let (mut manifest, mut segments) = l6fw::unpack_bytes(package)?;
    let original_firmware = segments
        .get("main_firmware.bin")
        .ok_or("package has no main_firmware.bin segment")?
        .clone();
    verify_firmware_for(model, &original_firmware)?;
    let original_length = original_firmware.len();
    let padding = segments
        .get("padding_before_secondary.bin")
        .ok_or("package has no padding_before_secondary.bin segment")?
        .clone();

    let mut firmware = original_firmware.clone();
    let (firmware_base, slot_end) = match model {
        FirmwareModel::L6 => (L6_MAIN_BASE, L6_SLOT_END),
        FirmwareModel::L6max => (0x80000000, 0x801a2ff8),
    };
    let mut base = first_payload_for(model, original_length);
    let mut applied_report = Vec::new();
    let mut diagnostic_hooks = Vec::new();
    let mut output_version = UPDATE_VERSION;
    let mut loader_reserve = None;
    if install_patch_loader {
        let loader_base = base;
        let unit = find_loader(model, loader_base, &ordered)?;
        let mut payload = unit.template.clone();
        for window in &unit.windows {
            let offset = window
                .firmware_address
                .checked_sub(firmware_base)
                .ok_or("loader window precedes firmware image")? as usize;
            let end = offset
                .checked_add(window.length)
                .ok_or("loader window overflow")?;
            let payload_end = window
                .payload_offset
                .checked_add(window.length)
                .ok_or("loader payload window overflow")?;
            if end > original_firmware.len() || payload_end > payload.len() {
                return Err("loader template window exceeds the firmware or payload".into());
            }
            payload[window.payload_offset..payload_end]
                .copy_from_slice(&original_firmware[offset..end]);
        }
        for site in &unit.sites {
            let offset = site
                .address
                .checked_sub(firmware_base)
                .ok_or("loader edit precedes firmware image")? as usize;
            let end = offset
                .checked_add(site.length)
                .ok_or("loader edit range overflow")?;
            if end > original_firmware.len() {
                return Err("loader edit exceeds firmware image".into());
            }
            let actual = format!("{:x}", Sha256::digest(&original_firmware[offset..end]));
            if actual != site.guard_sha256 {
                return Err(format!("Patch loader guard failed at {:#x}", site.address));
            }
        }
        let payload_end = loader_base
            .checked_add(payload.len() as u32)
            .ok_or("loader payload address overflow")?;
        if payload_end > unit.module_base {
            return Err("Patch loader payload overlaps its reserved module region".into());
        }
        let payload_offset = loader_base
            .checked_sub(firmware_base)
            .ok_or("loader payload precedes image")? as usize;
        firmware.resize(payload_offset, 0xff);
        firmware.extend_from_slice(&payload);
        for site in &unit.sites {
            let offset = (site.address - firmware_base) as usize;
            let target = loader_base
                .checked_add(site.target_offset)
                .ok_or("loader branch target overflow")?;
            firmware[offset..offset + 4].copy_from_slice(&branch(site.address, target, site.link));
            for chunk in firmware[offset + 4..offset + site.length].chunks_exact_mut(2) {
                chunk.copy_from_slice(&0xbf00u16.to_le_bytes());
            }
        }
        applied_report.push(serde_json::json!({
            "name": "patch-loader",
            "payload_address": format!("{loader_base:#x}"),
            "payload_length": payload.len(),
            "reserved_end": format!("{:#x}", unit.reserve_end),
        }));
        base = (payload_end + 15) & !15;
        loader_reserve = Some((unit.module_base, unit.reserve_end));
        output_version = "0114";
    }
    for &name in &ordered {
        let unit = find_unit(name, model, base)?;
        diagnostic_hooks.extend(unit.hooks.iter().zip(&unit.hook_symbols).map(|(hook, symbol)| {
            serde_json::json!({"address": hook.address, "target": hook.target, "symbol": symbol, "displaced_length": hook.length})
        }));
        let mut payload = unit.template.clone();
        for window in &unit.windows {
            let start = (window.firmware_address - firmware_base) as usize;
            payload[window.payload_offset..window.payload_offset + window.length]
                .copy_from_slice(&original_firmware[start..start + window.length]);
        }
        let edits = PatchEdits::new(unit.hooks);
        let applied = apply_patch_with_map(
            &mut firmware,
            original_length,
            base,
            &payload,
            &edits,
            &padding,
            firmware_base,
            slot_end,
        )?;
        applied_report.push(serde_json::json!({
            "name": name,
            "payload_address": format!("{base:#x}"),
            "payload_length": applied.payload_len,
        }));
        base = applied.next_base;
        if let Some((module_base, _)) = loader_reserve
            && base > module_base
        {
            return Err("firmware feature payloads overlap the Patch loader module reserve".into());
        }
    }
    if let Some((_, reserve_end)) = loader_reserve {
        let reserve_offset = reserve_end
            .checked_sub(firmware_base)
            .ok_or("loader reserve precedes firmware image")? as usize;
        let consumed = reserve_offset
            .checked_sub(original_length)
            .ok_or("loader reserve overlaps original firmware")?;
        if consumed > padding.len() || padding[..consumed].iter().any(|&byte| byte != 0xff) {
            return Err("Patch loader reserve overlaps non-erased package data".into());
        }
        firmware.resize(reserve_offset, 0xff);
    }

    let new_length = firmware.len() as u32;
    let consumed = firmware.len() - original_length;
    segments.insert("main_firmware.bin".into(), firmware.clone());
    segments.insert(
        "padding_before_secondary.bin".into(),
        padding[consumed..].to_vec(),
    );
    let mut trailer = segments["main_trailer.bin"].clone();
    let mut footer = segments["package_footer.bin"].clone();
    write_image_length_and_version(&mut trailer, &mut footer, new_length, output_version);
    segments.insert("main_trailer.bin".into(), trailer);
    segments.insert("package_footer.bin".into(), footer);
    for (segment_name, start, end) in l6fw::layout_for(
        match model {
            FirmwareModel::L6 => l6fw::Model::L6,
            FirmwareModel::L6max => l6fw::Model::L6max,
        },
        new_length as usize,
    ) {
        if let Some(index) = manifest["segments"]
            .as_array()
            .and_then(|array| array.iter().position(|entry| entry["file"] == segment_name))
        {
            manifest["segments"][index]["offset"] = serde_json::json!(start);
            manifest["segments"][index]["size"] = serde_json::json!(end - start);
        }
    }
    let packed = l6fw::repack_bytes(&manifest, &segments, true)?;
    if model == FirmwareModel::L6max {
        l6fw::update::validate(&packed).map_err(|e| e.to_string())?;
    } else if l6fw::model_from_package(&packed)? != l6fw::Model::L6 {
        return Err("patched package model changed unexpectedly".into());
    }

    let report = serde_json::json!({
        "patched_sha256": format!("{:x}", Sha256::digest(&firmware)),
        "model": if model == FirmwareModel::L6 { "L6" } else { "L6max" },
        "image_length": firmware.len(),
        "patches": applied_report,
        "hooks": diagnostic_hooks,
        "update_sha256": format!("{:x}", Sha256::digest(&packed)),
        "version": output_version,
        "usage": "Patched in your browser from your own upload. Verified in emulation only; physical bootloader acceptance is unverified.",
    });
    Ok((packed, report))
}

/// Identifies the supported model from an uploaded firmware update package.
#[wasm_bindgen(js_name = detectFirmwareModel)]
pub fn detect_firmware_model(package: &[u8]) -> Result<String, JsError> {
    let model = l6fw::model_from_package(package).map_err(|message| JsError::new(&message))?;
    Ok(match model {
        l6fw::Model::L6 => "L6".into(),
        l6fw::Model::L6max => "L6max".into(),
    })
}

/// Everything `apply_patches_wasm` hands back to JS: the patched package
/// bytes (as a `Uint8Array`) and the report, as JSON text for the caller to
/// `JSON.parse`.
#[wasm_bindgen]
pub struct PatchResult {
    package: Vec<u8>,
    report_json: String,
}

#[wasm_bindgen]
impl PatchResult {
    #[wasm_bindgen(getter)]
    pub fn package(&self) -> Vec<u8> {
        self.package.clone()
    }
    #[wasm_bindgen(getter, js_name = reportJson)]
    pub fn report_json(&self) -> String {
        self.report_json.clone()
    }
}

/// JS entry point: `selected` is any non-empty subset of `"knob-dialog"`,
/// `"midi-sync"`, `"din-mixer-control"`, in any order.
#[wasm_bindgen(js_name = applyPatches)]
pub fn apply_patches_wasm(
    package: &[u8],
    selected: Vec<String>,
    install_patch_loader: bool,
) -> Result<PatchResult, JsError> {
    let selected_refs: Vec<&str> = selected.iter().map(String::as_str).collect();
    let (package, report) =
        apply_patches_with_loader(package, &selected_refs, install_patch_loader)
            .map_err(|message| JsError::new(&message))?;
    Ok(PatchResult {
        package,
        report_json: report.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::{find_unit, parse_unit};
    use firmware_patch_core::FirmwareModel;

    #[test]
    fn embedded_template_metadata_requires_an_explicit_model() {
        let metadata = r#"{
            "base": "0x80000000",
            "windows": [],
            "hooks": []
        }"#;
        assert_eq!(
            parse_unit(&[], metadata).err().unwrap(),
            "template metadata missing model"
        );
    }

    #[test]
    fn embedded_template_metadata_rejects_unknown_models() {
        let metadata = r#"{
            "model": "L6mini",
            "base": "0x80000000",
            "windows": [],
            "hooks": []
        }"#;
        assert_eq!(
            parse_unit(&[], metadata).err().unwrap(),
            "unknown firmware model in template: L6mini"
        );
    }

    #[test]
    fn knob_template_has_well_formed_hook_metadata() {
        let unit = find_unit("knob-dialog", FirmwareModel::L6max, 0x80108000).unwrap();
        assert_eq!(unit.hooks.len(), unit.hook_symbols.len());
        assert!(unit.hooks.iter().all(|hook| hook.length >= 4));
    }

    #[test]
    fn metadata_requires_hook_symbols() {
        let metadata = r#"{"model":"L6", "base":"0x80000000", "windows":[], "hooks":[{"address":"0x80000000", "target":"0x80001000", "length":4}]}"#;
        assert!(parse_unit(&[], metadata).err().unwrap().contains("symbol"));
    }
}
