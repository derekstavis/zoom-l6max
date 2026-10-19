//! Reproducible, revision-guarded firmware experiment. Never edits its input.
use firmware_patch_compile::{compile, symbol};
use firmware_patch_core::{
    CallSiteEdit, FIRST_PAYLOAD, HookEdit, PATCHES, PatchEdits, apply_patch, verify_firmware,
    write_image_length_and_version,
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};
const USAGE: &str = "Usage: firmware-patch INPUT_UNPACKED_DIR OUTPUT_DIR [--patch NAME]... [--update-version FOUR_DIGITS]\n\
Patches: knob-dialog (default), midi-sync, din-mixer-control. Repeat --patch to combine them.";

struct Options {
    source: PathBuf,
    output: PathBuf,
    patches: Vec<&'static str>,
    version: String,
}
fn parse_args() -> Result<Options, Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(source), Some(output)) = (args.next(), args.next()) else {
        return Err(USAGE.into());
    };
    let mut requested = Vec::new();
    let mut version = None;
    while let Some(flag) = args.next() {
        let value = args.next().ok_or(USAGE)?;
        match flag.as_str() {
            "--patch" => requested.push(
                *PATCHES
                    .iter()
                    .find(|name| **name == value)
                    .ok_or_else(|| format!("unknown patch {value}\n{USAGE}"))?,
            ),
            "--update-version" => version = Some(value),
            _ => return Err(USAGE.into()),
        }
    }
    let patches = if requested.is_empty() {
        vec!["knob-dialog"]
    } else {
        PATCHES
            .into_iter()
            .filter(|n| requested.contains(n))
            .collect()
    };
    // The original knob-only build keeps its published marker; anything else is newer.
    let version = version.unwrap_or_else(|| {
        if patches == ["knob-dialog"] {
            "0112"
        } else {
            "0113"
        }
        .into()
    });
    if version.len() != 4 || !version.bytes().all(|b| b.is_ascii_digit()) {
        return Err("update version must be exactly four ASCII digits, e.g. 0112".into());
    }
    Ok(Options {
        source: PathBuf::from(source),
        output: PathBuf::from(output),
        patches,
        version,
    })
}
/// Guard a hook table against the supplied image; returns its parsed rows.
fn read_hooks(
    firmware: &[u8],
    root: &Path,
) -> Result<Vec<(u32, String, usize, String)>, Box<dyn std::error::Error>> {
    let mut hooks = Vec::new();
    for line in fs::read_to_string(root.join("hooks.tsv"))?
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let columns = line.split('\t').collect::<Vec<_>>();
        let address = u32::from_str_radix(columns[0], 16)?;
        let length: usize = columns[2].parse()?;
        let offset = (address - 0x80000000) as usize;
        if format!("{:x}", Sha256::digest(&firmware[offset..offset + length])) != columns[3] {
            return Err(format!("hook guard failed: {}", columns[1]).into());
        }
        hooks.push((
            address,
            columns[1].to_string(),
            length,
            columns[3].to_string(),
        ));
    }
    Ok(hooks)
}
// `compile()` and `symbol()` now live in `firmware-patch-compile`, shared
// with the WASM-template prebuild tool so both compile payloads identically.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = parse_args()?;
    let source = options.source.canonicalize()?;
    if options.output.exists() {
        return Err("output must be a new directory".into());
    }
    let version = options.version.as_str();
    let mut firmware = fs::read(source.join("main_firmware.bin"))?;
    let original_length = firmware.len();
    verify_firmware(&firmware)?;
    let digest = format!("{:x}", Sha256::digest(&firmware));
    // Validate every hook region before compiling or modifying the output image.
    let roots = options
        .patches
        .iter()
        .map(|name| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../patches")
                .join(name)
                .canonicalize()
        })
        .collect::<Result<Vec<_>, _>>()?;
    let tables = roots
        .iter()
        .map(|root| read_hooks(&firmware, root))
        .collect::<Result<Vec<_>, _>>()?;
    let mut claimed = Vec::new();
    for (address, name, length, _) in tables.iter().flatten() {
        if claimed
            .iter()
            .any(|&(a, l): &(u32, usize)| *address < a + l as u32 && a < *address + *length as u32)
        {
            return Err(format!("hook overlaps another patch: {name}").into());
        }
        claimed.push((*address, *length));
    }
    fs::create_dir_all(&options.output)?;
    let output = options.output.canonicalize()?;
    let padding = fs::read(source.join("padding_before_secondary.bin"))?;
    // Payloads are placed back to back, each aligned to 16 bytes.
    let mut end = FIRST_PAYLOAD;
    let mut applied = Vec::new();
    let mut knob_report = serde_json::Map::new();
    for (index, (name, root)) in options.patches.iter().zip(&roots).enumerate() {
        let base = end;
        let (payload, symbols) = compile(&source, root, &output, name, base)?;
        let hooks = &tables[index];
        let mut edits = PatchEdits::new(
            hooks
                .iter()
                .map(|(address, hook, length, _)| {
                    Ok(HookEdit {
                        address: *address,
                        target: symbol(&symbols, hook)?,
                        length: *length,
                    })
                })
                .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?,
        );
        if *name == "knob-dialog" {
            edits.knob_dialog_entry = Some(symbol(&symbols, "knob_changed")?);
            edits.knob_dialog_call_sites = [
                (0x8000648eu32, "notification_initialize_hook", 0x8005f600u32),
                (0x80006126, "semaphore_take_hook", 0x80091048),
                (0x800062c0, "semaphore_give_hook", 0x80090ad0),
            ]
            .into_iter()
            .map(|(address, call, original)| {
                Ok(CallSiteEdit {
                    address,
                    original,
                    target: symbol(&symbols, call)?,
                })
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        }
        let applied_patch = apply_patch(
            &mut firmware,
            original_length,
            base,
            &payload,
            &edits,
            &padding,
        )?;
        end = applied_patch.next_base;
        let mut applied_hooks = Vec::new();
        for (hook_edit, (address, hook, length, hash)) in edits.hooks.iter().zip(hooks) {
            applied_hooks.push(serde_json::json!({"address":address,"symbol":hook,"target":hook_edit.target,"displaced_length":length,"displaced_sha256":hash}));
        }
        if *name == "knob-dialog" {
            for (call_edit, (_, call, _)) in edits.knob_dialog_call_sites.iter().zip([
                (0x8000648eu32, "notification_initialize_hook", 0x8005f600u32),
                (0x80006126, "semaphore_take_hook", 0x80091048),
                (0x800062c0, "semaphore_give_hook", 0x80090ad0),
            ]) {
                applied_hooks.push(
                    serde_json::json!({"address":call_edit.address,"symbol":call,"target":call_edit.target}),
                );
            }
            knob_report.insert(
                "knob_changed".into(),
                symbol(&symbols, "knob_changed")?.into(),
            );
            knob_report.insert(
                "notification_tick".into(),
                symbol(&symbols, "notification_tick_hook")?.into(),
            );
        }
        applied.push(serde_json::json!({"name":name,"payload_address":format!("{base:#x}"),"payload_length":applied_patch.payload_len,"hooks":applied_hooks}));
    }
    let consumed = firmware.len() - original_length;
    // Preserve the fixed package slots, consuming only erased main padding.
    for entry in fs::read_dir(&source)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            fs::copy(entry.path(), output.join(entry.file_name()))?;
        }
    }
    fs::write(output.join("main_firmware.bin"), &firmware)?;
    fs::write(
        output.join("padding_before_secondary.bin"),
        &padding[consumed..],
    )?;
    let mut trailer = fs::read(output.join("main_trailer.bin"))?;
    let mut footer = fs::read(output.join("package_footer.bin"))?;
    write_image_length_and_version(&mut trailer, &mut footer, firmware.len() as u32, version);
    fs::write(output.join("main_trailer.bin"), trailer)?;
    fs::write(output.join("package_footer.bin"), footer)?;
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("manifest.json"))?)?;
    manifest["segments"][2]["size"] = serde_json::json!(firmware.len());
    manifest["segments"][3]["offset"] = serde_json::json!(0x200 + firmware.len());
    manifest["segments"][3]["size"] = serde_json::json!(padding.len() - consumed);
    fs::write(
        output.join("manifest.json"),
        serde_json::to_string_pretty(&manifest)?,
    )?;
    let update = output.join("L6max.bin");
    l6fw::repack(&output, &update, true)?;
    let packed = fs::read(&update)?;
    l6fw::update::validate(&packed)?;
    let mut report = serde_json::json!({"source_sha256":digest,"patched_sha256":format!("{:x}",Sha256::digest(&firmware)),"image_length":firmware.len(),"payload_address":format!("{FIRST_PAYLOAD:#x}"),"patches":applied,"update_package":"L6max.bin","update_sha256":format!("{:x}",Sha256::digest(&packed)),"version":String::from_utf8_lossy(&packed[0x1a31fc..0x1a3200]),"usage":"Repacked update for emulator validation; physical bootloader acceptance unverified"});
    if options.patches.contains(&"knob-dialog") {
        let fields = report.as_object_mut().unwrap();
        fields.extend(knob_report);
        fields.insert("hooks".into(), applied[0]["hooks"].clone());
        fields.insert("synchronization".into(), "existing counting semaphore with heap sidecar for same-task nesting; kernel receives untagged handle".into());
        fields.insert("knob_presentation".into(), "coalesced copied value serviced by notification callback; mixer hook never waits for rendering".into());
        fields.insert(
            "state_storage".into(),
            "original firmware heap, allocated per popup and released on restoration".into(),
        );
    }
    fs::write(
        output.join("patch.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    println!(
        "{} bytes of Rust payload ({}); patched firmware: {}",
        firmware.len() - 0x108000,
        options.patches.join(" + "),
        output.display()
    );
    Ok(())
}

// Thumb-branch encoding is tested in `firmware-patch-core`, which owns `branch()`.

#[cfg(test)]
#[path = "../../../patches/knob-dialog/src/values.rs"]
mod values;

#[cfg(test)]
#[path = "../../../patches/midi-sync/src/parameters.rs"]
mod parameters;
