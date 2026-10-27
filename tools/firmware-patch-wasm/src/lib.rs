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
    CallSiteEdit, FirmwareModel, HookEdit, L6_MAIN_BASE, L6_SLOT_END, PATCHES, PatchEdits,
    apply_patch_with_map, first_payload_for, verify_firmware_for, write_image_length_and_version,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use wasm_bindgen::prelude::*;

/// Update-version marker for every package this crate produces, distinct
/// from the native CLI's own dev-build markers (0112/0113). See the plan's
/// "Resolved decisions": 1.11 reads as "stock 1.10 plus the web patcher's
/// changes."
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
    knob_dialog_entry: Option<u32>,
    knob_dialog_call_sites: Vec<CallSiteEdit>,
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
    let knob_dialog_entry = match &meta["knob_dialog_entry"] {
        Value::Null => None,
        value => Some(parse_hex(value)?),
    };
    let knob_dialog_call_sites = meta["knob_dialog_call_sites"]
        .as_array()
        .ok_or("metadata missing knob_dialog_call_sites")?
        .iter()
        .map(|c| {
            Ok(CallSiteEdit {
                address: parse_hex(&c["address"])?,
                original: parse_hex(&c["original"])?,
                target: parse_hex(&c["target"])?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Unit {
        model,
        base,
        template: payload.to_vec(),
        windows,
        hooks,
        hook_symbols,
        knob_dialog_entry,
        knob_dialog_call_sites,
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

/// Applies `selected` (a non-empty subset of `firmware_patch_core::PATCHES`,
/// any order) to `package` — a full Zoom update package, byte for byte what
/// `l6fw::unpack`/`repack` round-trip — and returns the patched package plus
/// a small JSON report. `package` is the only untrusted input: every other
/// byte comes from the embedded, Zoom-byte-free templates.
pub fn apply_patches(package: &[u8], selected: &[&str]) -> Result<(Vec<u8>, Value), String> {
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
    if ordered.is_empty() {
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
    for &name in &ordered {
        let unit = find_unit(name, model, base)?;
        if name == "knob-dialog" {
            diagnostic_hooks = unit.hooks.iter().zip(&unit.hook_symbols).map(|(hook, symbol)|
                serde_json::json!({"address": hook.address, "target": hook.target, "symbol": symbol, "displaced_length": hook.length})
            ).collect();
        }
        let mut payload = unit.template.clone();
        for window in &unit.windows {
            let start = (window.firmware_address - firmware_base) as usize;
            payload[window.payload_offset..window.payload_offset + window.length]
                .copy_from_slice(&original_firmware[start..start + window.length]);
        }
        let mut edits = PatchEdits::new(unit.hooks);
        edits.knob_dialog_entry = unit.knob_dialog_entry;
        edits.knob_dialog_call_sites = unit.knob_dialog_call_sites;
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
    write_image_length_and_version(&mut trailer, &mut footer, new_length, UPDATE_VERSION);
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
        "version": UPDATE_VERSION,
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
pub fn apply_patches_wasm(package: &[u8], selected: Vec<String>) -> Result<PatchResult, JsError> {
    let selected_refs: Vec<&str> = selected.iter().map(String::as_str).collect();
    let (package, report) =
        apply_patches(package, &selected_refs).map_err(|message| JsError::new(&message))?;
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
            "hooks": [],
            "knob_dialog_call_sites": []
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
            "hooks": [],
            "knob_dialog_call_sites": []
        }"#;
        assert_eq!(
            parse_unit(&[], metadata).err().unwrap(),
            "unknown firmware model in template: L6mini"
        );
    }

    #[test]
    fn knob_template_exposes_named_hooks_for_fixture_diagnostics() {
        let unit = find_unit("knob-dialog", FirmwareModel::L6max, 0x80108000).unwrap();
        assert_eq!(unit.hooks.len(), unit.hook_symbols.len());
        for name in [
            "notification_show_hook",
            "notification_pop_hook",
            "busy_show_hook",
            "busy_hide_hook",
            "overlays_clear_hook",
        ] {
            assert!(unit.hook_symbols.iter().any(|symbol| symbol == name));
        }
        let metadata = r#"{"model":"L6", "base":"0x80000000", "windows":[], "hooks":[{"address":"0x80000000", "target":"0x80001000", "length":4}], "knob_dialog_call_sites":[]}"#;
        assert!(parse_unit(&[], metadata).err().unwrap().contains("symbol"));
    }
}
