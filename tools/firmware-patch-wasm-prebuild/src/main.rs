//! Local-only maintainer tool. Never run in CI, which has no access to the
//! real Zoom firmware (by design — see the repo's firmware-handling policy).
//!
//! Produces the static templates the browser-based patcher ships: for each
//! of the 7 non-empty subsets of `firmware_patch_core::PATCHES`, and for each
//! patch within it, the payload `firmware-patch` would compile at that
//! patch's position — but with every byte that came from the user's firmware
//! (via a patch's `trampolines.s` `.incbin`) zeroed out, plus metadata
//! recording exactly which firmware address each zeroed window came from.
//! The browser never needs those bytes from us: it fills each window from
//! the firmware the user themselves uploads.
//!
//! How a window is found, per (patch, base): compile the payload twice
//! against the same base — once against the real firmware, once against a
//! copy with every byte any of that patch's hooks displace replaced by a
//! unique, per-byte sentinel value. Compilation is otherwise deterministic
//! (verified elsewhere this session: no `build.rs`, no env/time/randomness,
//! pinned toolchain), so the two compiled payloads can only differ at a byte
//! that a `.incbin` actually read — anywhere else, by construction, nothing
//! about the input changed. Diffing them finds those bytes without needing
//! to understand each patch's trampoline assembly; reversing the sentinel
//! value at a differing byte recovers exactly which firmware address it came
//! from. Each unit's own self-check (below) then proves the template plus
//! the real firmware's bytes at the recorded windows reconstructs the real
//! payload exactly, byte for byte — the load-bearing guarantee this whole
//! approach rests on.
use firmware_patch_compile::{compile, symbol};
use firmware_patch_core::{EXPECTED_FIRMWARE_SHA256, FIRST_PAYLOAD, PATCHES};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

/// `knob-dialog`'s three `bl` call-site redirects (address, hook symbol,
/// original target) — not part of `hooks.tsv`, applied separately by
/// `firmware-patch` and by `firmware-patch-core::apply_patch`.
const KNOB_DIALOG_CALL_SITES: [(u32, &str, u32); 3] = [
    (0x8000648e, "notification_initialize_hook", 0x8005f600),
    (0x80006126, "semaphore_take_hook", 0x80091048),
    (0x800062c0, "semaphore_give_hook", 0x80090ad0),
];

struct HookRow {
    address: u32,
    symbol: String,
    length: usize,
    sha256: String,
}

fn read_hook_rows(patch_root: &Path) -> Result<Vec<HookRow>, Box<dyn std::error::Error>> {
    let mut rows = Vec::new();
    for line in fs::read_to_string(patch_root.join("hooks.tsv"))?
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let columns: Vec<_> = line.split('\t').collect();
        rows.push(HookRow {
            address: u32::from_str_radix(columns[0], 16)?,
            symbol: columns[1].to_string(),
            length: columns[2].parse()?,
            sha256: columns[3].to_string(),
        });
    }
    Ok(rows)
}

/// Builds a copy of `firmware` with every byte any hook row displaces
/// replaced by a unique value in `0x01..=0xfe`, distinct from that byte's
/// real value. Returns the sentinel firmware and a reverse lookup from
/// sentinel value to the firmware address it stands in for.
fn build_sentinel_firmware(firmware: &[u8], rows: &[HookRow]) -> (Vec<u8>, HashMap<u8, u32>) {
    let mut sentinel = firmware.to_vec();
    let mut value_to_address = HashMap::new();
    let mut candidate: u16 = 1;
    for row in rows {
        for i in 0..row.length as u32 {
            let address = row.address + i;
            let offset = (address - 0x80000000) as usize;
            let real_byte = firmware[offset];
            while candidate <= 0xfe
                && (value_to_address.contains_key(&(candidate as u8))
                    || candidate as u8 == real_byte)
            {
                candidate += 1;
            }
            assert!(candidate <= 0xfe, "ran out of sentinel byte values");
            let value = candidate as u8;
            value_to_address.insert(value, address);
            sentinel[offset] = value;
            candidate += 1;
        }
    }
    (sentinel, value_to_address)
}

/// One zeroed-out range in a template payload, and where its real bytes come
/// from in the firmware the browser's user uploads.
struct Window {
    payload_offset: usize,
    firmware_address: u32,
    length: usize,
}

/// Diffs two equal-length, otherwise-identical compiles and resolves every
/// differing byte back to a firmware address via `value_to_address`. Errors
/// if a differing byte isn't a known sentinel value — that would mean the
/// two compiles differed for some reason other than the firmware bytes we
/// deliberately changed, invalidating this whole approach.
fn find_windows(
    real_payload: &[u8],
    sentinel_payload: &[u8],
    value_to_address: &HashMap<u8, u32>,
) -> Result<Vec<Window>, String> {
    if real_payload.len() != sentinel_payload.len() {
        return Err("real and sentinel compiles produced different lengths".into());
    }
    let mut windows: Vec<Window> = Vec::new();
    for (offset, (&real_byte, &sentinel_byte)) in
        real_payload.iter().zip(sentinel_payload).enumerate()
    {
        if real_byte == sentinel_byte {
            continue;
        }
        let address = *value_to_address.get(&sentinel_byte).ok_or_else(|| {
            format!(
                "payload byte at offset {offset:#x} changed (sentinel build gave {sentinel_byte:#04x}) \
                 but that isn't a sentinel value we assigned — compilation isn't as deterministic as assumed"
            )
        })?;
        if let Some(last) = windows.last_mut()
            && last.payload_offset + last.length == offset
            && last.firmware_address + last.length as u32 == address
        {
            last.length += 1;
            continue;
        }
        windows.push(Window {
            payload_offset: offset,
            firmware_address: address,
            length: 1,
        });
    }
    Ok(windows)
}

/// One precompiled build unit: a patch's payload at one specific base
/// address, Zoom-byte-free, plus everything `firmware-patch-core::apply_patch`
/// needs once the browser fills in `windows` from the user's own upload.
struct Unit {
    name: &'static str,
    base: u32,
    template: Vec<u8>,
    windows: Vec<Window>,
    hooks: Vec<(u32, u32, usize)>, // (address, target, length)
    knob_dialog_entry: Option<u32>,
    knob_dialog_call_sites: Vec<(u32, u32, u32)>, // (address, original, target)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("Usage: firmware-patch-wasm-prebuild REAL_UNPACKED_FIRMWARE_DIR")?,
    )
    .canonicalize()?;
    let firmware = fs::read(source.join("main_firmware.bin"))?;
    let digest = format!("{:x}", Sha256::digest(&firmware));
    if digest != EXPECTED_FIRMWARE_SHA256 {
        return Err(format!(
            "unsupported main firmware SHA-256: expected {EXPECTED_FIRMWARE_SHA256}, got {digest}"
        )
        .into());
    }

    let patches_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../patches");
    let scratch = std::env::temp_dir().join(format!(
        "firmware-patch-wasm-prebuild-{}",
        std::process::id()
    ));
    fs::create_dir_all(&scratch)?;

    // Per patch: its hook rows, verified against the real firmware, and a
    // sentinel firmware + reverse lookup built once and reused at every base
    // that patch needs (the sentinel substitution doesn't depend on base).
    struct PerPatch {
        root: PathBuf,
        rows: Vec<HookRow>,
        sentinel_source: PathBuf,
        value_to_address: HashMap<u8, u32>,
    }
    let mut per_patch: HashMap<&'static str, PerPatch> = HashMap::new();
    for name in PATCHES {
        let root = patches_dir.join(name).canonicalize()?;
        let rows = read_hook_rows(&root)?;
        for row in &rows {
            let offset = (row.address - 0x80000000) as usize;
            let actual = format!(
                "{:x}",
                Sha256::digest(&firmware[offset..offset + row.length])
            );
            if actual != row.sha256 {
                return Err(format!(
                    "{name}: hooks.tsv hash mismatch for {} at {:#x}",
                    row.symbol, row.address
                )
                .into());
            }
        }
        let (sentinel, value_to_address) = build_sentinel_firmware(&firmware, &rows);
        let sentinel_source = scratch.join(format!("{name}-sentinel"));
        fs::create_dir_all(&sentinel_source)?;
        fs::write(sentinel_source.join("main_firmware.bin"), &sentinel)?;
        per_patch.insert(
            name,
            PerPatch {
                root,
                rows,
                sentinel_source,
                value_to_address,
            },
        );
    }

    let mut units: HashMap<(&'static str, u32), Unit> = HashMap::new();
    let output_scratch = scratch.join("compile-out");
    fs::create_dir_all(&output_scratch)?;
    for mask in 1u8..8 {
        let subset: Vec<&'static str> = PATCHES
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .map(|(_, name)| *name)
            .collect();
        let mut base = FIRST_PAYLOAD;
        for name in subset {
            if !units.contains_key(&(name, base)) {
                let info = &per_patch[name];
                let (real_payload, symbols) =
                    compile(&source, &info.root, &output_scratch, name, base)?;
                let (sentinel_payload, _) = compile(
                    &info.sentinel_source,
                    &info.root,
                    &output_scratch,
                    name,
                    base,
                )?;
                let windows =
                    find_windows(&real_payload, &sentinel_payload, &info.value_to_address)?;

                // The load-bearing self-check: zero the windows, then refill
                // them from the REAL firmware, and confirm that reproduces
                // the real compile exactly. If this doesn't hold, the
                // template is unsafe to ship (either leaking real bytes, or
                // unable to reconstruct a working payload) and we must stop.
                let mut template = real_payload.clone();
                for window in &windows {
                    template[window.payload_offset..window.payload_offset + window.length].fill(0);
                }
                let mut reconstructed = template.clone();
                for window in &windows {
                    let start = (window.firmware_address - 0x80000000) as usize;
                    reconstructed[window.payload_offset..window.payload_offset + window.length]
                        .copy_from_slice(&firmware[start..start + window.length]);
                }
                if reconstructed != real_payload {
                    return Err(format!(
                        "{name} at {base:#x}: template + real firmware bytes does not reconstruct the real payload"
                    )
                    .into());
                }

                let hooks = info
                    .rows
                    .iter()
                    .map(|row| Ok((row.address, symbol(&symbols, &row.symbol)?, row.length)))
                    .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
                let (knob_dialog_entry, knob_dialog_call_sites) = if name == "knob-dialog" {
                    (
                        Some(symbol(&symbols, "knob_changed")?),
                        KNOB_DIALOG_CALL_SITES
                            .iter()
                            .map(|(address, call, original)| {
                                Ok((*address, *original, symbol(&symbols, call)?))
                            })
                            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?,
                    )
                } else {
                    (None, Vec::new())
                };

                units.insert(
                    (name, base),
                    Unit {
                        name,
                        base,
                        template,
                        windows,
                        hooks,
                        knob_dialog_entry,
                        knob_dialog_call_sites,
                    },
                );
            }
            base = {
                let len = units[&(name, base)].template.len() as u32;
                (base + len + 15) & !15
            };
        }
    }
    assert_eq!(
        units.len(),
        7,
        "expected exactly 7 distinct (patch, base) build units"
    );

    let templates_dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../firmware-patch-wasm/templates");
    fs::create_dir_all(&templates_dir)?;
    for entry in fs::read_dir(&templates_dir)? {
        let entry = entry?;
        if entry
            .path()
            .extension()
            .is_some_and(|ext| ext == "bin" || ext == "json")
        {
            fs::remove_file(entry.path())?;
        }
    }

    let mut unit_list = units.into_values().collect::<Vec<_>>();
    unit_list.sort_by_key(|unit| (unit.name, unit.base));
    let mut manifest_units = Vec::new();
    for unit in &unit_list {
        let stem = format!("{}-{:08x}", unit.name, unit.base);
        fs::write(templates_dir.join(format!("{stem}.bin")), &unit.template)?;
        let windows_json: Vec<_> = unit
            .windows
            .iter()
            .map(|w| {
                serde_json::json!({
                    "payload_offset": w.payload_offset,
                    "firmware_address": format!("{:#x}", w.firmware_address),
                    "length": w.length,
                })
            })
            .collect();
        let hooks_json: Vec<_> = unit
            .hooks
            .iter()
            .map(|(address, target, length)| {
                serde_json::json!({
                    "address": format!("{address:#x}"),
                    "target": format!("{target:#x}"),
                    "length": length,
                })
            })
            .collect();
        let call_sites_json: Vec<_> = unit
            .knob_dialog_call_sites
            .iter()
            .map(|(address, original, target)| {
                serde_json::json!({
                    "address": format!("{address:#x}"),
                    "original": format!("{original:#x}"),
                    "target": format!("{target:#x}"),
                })
            })
            .collect();
        let metadata = serde_json::json!({
            "name": unit.name,
            "base": format!("{:#x}", unit.base),
            "payload_file": format!("{stem}.bin"),
            "payload_length": unit.template.len(),
            "windows": windows_json,
            "hooks": hooks_json,
            "knob_dialog_entry": unit.knob_dialog_entry.map(|a| format!("{a:#x}")),
            "knob_dialog_call_sites": call_sites_json,
            "window_byte_total": unit.windows.iter().map(|w| w.length).sum::<usize>(),
        });
        fs::write(
            templates_dir.join(format!("{stem}.json")),
            serde_json::to_string_pretty(&metadata)?,
        )?;
        manifest_units.push(serde_json::json!({"name": unit.name, "base": format!("{:#x}", unit.base), "metadata_file": format!("{stem}.json")}));
        println!(
            "{stem}: {} bytes, {} window(s), {} window byte(s)",
            unit.template.len(),
            unit.windows.len(),
            unit.windows.iter().map(|w| w.length).sum::<usize>()
        );
    }
    fs::write(
        templates_dir.join("manifest.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "source_firmware_sha256": digest,
            "units": manifest_units,
        }))?,
    )?;
    fs::remove_dir_all(&scratch)?;
    println!(
        "Wrote {} build units ({} Zoom-byte-free templates) to {}",
        unit_list.len(),
        unit_list.len(),
        templates_dir.display()
    );
    Ok(())
}
