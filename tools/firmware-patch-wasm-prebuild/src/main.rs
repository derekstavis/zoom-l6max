//! Local-only maintainer tool. Never run in CI, which has no access to the
//! real Zoom firmware (by design — see the repo's firmware-handling policy).
//!
//! Produces the static templates the browser-based patcher ships: for each
//! non-empty subset of `firmware_patch_core::PATCHES`, and for each
//! patch within it, the payload `l6patch` would compile at that
//! patch's position — but with every displaced byte copied verbatim by a
//! continuation veneer zeroed out, plus metadata recording which firmware
//! address supplies each removed byte window. Instruction-aware continuations
//! synthesize their control-flow opcodes from guarded hook metadata and decode
//! branch/call targets from the profile firmware.
//! The browser never needs those bytes from us: it fills each window from
//! the firmware the user themselves uploads.
//!
//! How a window is found, per (patch, base): compile the payload twice
//! against the same base — once against the real firmware, once against a
//! copy with every displaced byte copied verbatim replaced by a unique,
//! per-byte sentinel value. CBZ and BL encodings used only to decode
//! instruction-aware continuations are left intact; the generated code
//! relocates them rather than copying them. With the pinned toolchain and
//! identical compilation options, differing payload bytes therefore identify
//! copied firmware bytes. Reversing the sentinel value recovers exactly which
//! firmware address it came from. Each unit's own self-check (below) proves the template plus
//! the real firmware's bytes at the recorded windows reconstructs the real
//! payload exactly, byte for byte — the load-bearing guarantee this whole
//! approach rests on.
use firmware_patch_compile::{
    compile, compile_l6_patch_loader, compile_sentinel, compile_sentinel_l6_patch_loader,
    compile_sentinel_with_continuations, compile_with_continuations, function_hook_sites,
    hook_registrations, hook_windows, symbol,
};
use firmware_patch_core::{
    EXPECTED_FIRMWARE_SHA256, FIRST_PAYLOAD, FirmwareModel, L6_EXPECTED_FIRMWARE_SHA256,
    L6_MAIN_BASE, PATCHES, first_payload_for, verify_firmware_for,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone)]
struct HookRow {
    address: u32,
    symbol: String,
    length: usize,
    continuation_kind: String,
    continuation_site: Option<String>,
    continuation_slot_offset: Option<usize>,
    preserved_instructions: Vec<(usize, usize)>,
}

fn compile_hook_rows(
    source: &Path,
    root: &Path,
    output: &Path,
    name: &str,
    firmware: &[u8],
    firmware_base: u32,
) -> Result<Vec<HookRow>, Box<dyn std::error::Error>> {
    let probe_name = format!("probe-{name}");
    let (_, symbols) = compile(source, root, output, &probe_name, 0)?;
    let sites = function_hook_sites(&output.join(format!("{probe_name}.payload.elf")))?;
    let registrations = hook_registrations(&symbols)?;
    hook_windows(&sites, &registrations)?
        .into_iter()
        .map(|window| {
            let offset = usize::try_from(
                window
                    .address
                    .checked_sub(firmware_base)
                    .ok_or("hook window before firmware")?,
            )?;
            let _bytes = firmware
                .get(
                    offset
                        ..offset
                            .checked_add(window.length)
                            .ok_or("hook window length overflow")?,
                )
                .ok_or_else(|| format!("hook window outside firmware at {:#x}", window.address))?;
            let chain = registrations
                .iter()
                .find_map(|registration| match registration {
                    firmware_patch_compile::HookRegistration::Entry { site, handler }
                        if handler == &window.name =>
                    {
                        Some((
                            site.clone(),
                            format!("static_next_{site}_{handler}_slot"),
                        ))
                    }
                    firmware_patch_compile::HookRegistration::CallPre { function, .. }
                    | firmware_patch_compile::HookRegistration::CallPost { function, .. }
                        if window.name == format!("call_dispatch:{function}") =>
                    {
                        Some((
                            function.clone(),
                            format!("static_next_{function}_slot"),
                        ))
                    }
                    _ => None,
                });
            let chain_site = chain.as_ref().map(|(site, _)| site.clone());
            let continuation_slot_offset = chain
                .as_ref()
                .map(|(_, slot)| symbol(&symbols, slot))
                .transpose()?
                .map(|address| usize::try_from(address).unwrap());
            let continuation_kind = match window.continuation {
                0 => "linear".to_owned(),
                1 => "cbz".to_owned(),
                2 => "relocated_bl".to_owned(),
                strategy => {
                    return Err(format!("unsupported continuation strategy {strategy}").into());
                }
            };
            let mut preserved_instructions = Vec::new();
            if continuation_kind == "cbz" {
                preserved_instructions.push((0, 2));
            }
            if continuation_kind == "relocated_bl" {
                let start = usize::try_from(window.address - firmware_base)?;
                let displaced = &firmware[start..start + window.length];
                let mut offset = 0;
                while offset + 4 <= displaced.len() {
                    let first = u16::from_le_bytes(displaced[offset..offset + 2].try_into()?);
                    let second = u16::from_le_bytes(displaced[offset + 2..offset + 4].try_into()?);
                    if first & 0xf800 == 0xf000 && second & 0xd000 == 0xd000 {
                        preserved_instructions.push((offset, offset + 4));
                        offset += 4;
                    } else {
                        offset += 2;
                    }
                }
            }
            Ok(HookRow {
                address: window.address,
                symbol: window.target_symbol,
                length: window.length,
                continuation_kind,
                continuation_site: chain_site,
                continuation_slot_offset,
                preserved_instructions,
            })
        })
        .collect()
}

/// Builds a copy with displaced bytes replaced by unique sentinels. Keep the
/// CBZ and BL instructions used for continuation relocation intact so the
/// compiler can decode their original control flow; their derived branch
/// targets are fixed by the profile's exact hook guard. Returns the sentinel
/// firmware and a reverse lookup from sentinel value to firmware address.
fn build_sentinel_firmware(
    firmware: &[u8],
    rows: &[HookRow],
    firmware_base: u32,
) -> (Vec<u8>, HashMap<u8, u32>) {
    let mut sentinel = firmware.to_vec();
    let mut value_to_address = HashMap::new();
    let mut candidate: u16 = 1;
    for row in rows {
        let mut preserved = row.preserved_instructions.clone();
        if let Some(offset) = row.continuation_kind.strip_prefix("call@") {
            let offset: usize = offset
                .parse()
                .expect("validated call offset in hook metadata");
            preserved.push((offset, offset + 4));
        }
        for i in 0..row.length as u32 {
            let byte_index = i as usize;
            if preserved
                .iter()
                .any(|(start, end)| (*start..*end).contains(&byte_index))
            {
                continue;
            }
            let address = row.address + i;
            let offset = (address - firmware_base) as usize;
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

fn loader_site(
    firmware: &[u8],
    firmware_base: u32,
    loader_base: u32,
    address: u32,
    length: usize,
    target_name: &str,
    link: bool,
    symbols: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let start = usize::try_from(
        address
            .checked_sub(firmware_base)
            .ok_or("site before image")?,
    )?;
    let end = start.checked_add(length).ok_or("loader site overflow")?;
    let guard = firmware
        .get(start..end)
        .ok_or("loader site outside firmware")?;
    let target = symbol(symbols, target_name)?;
    let target_offset = target
        .checked_sub(loader_base)
        .ok_or("loader symbol before payload")?;
    Ok(serde_json::json!({
        "address": format!("{address:#x}"),
        "length": length,
        "guard_sha256": format!("{:x}", Sha256::digest(guard)),
        "target_offset": target_offset,
        "link": link,
        "dynamic_dispatch": false,
    }))
}

/// Build a browser template for the standalone persistent loader. Firmware
/// bytes copied into generated continuations or `.incbin` trampolines are
/// sentinel-substituted, then removed from the shipped template and restored
/// only from the user's verified upload.
fn build_loader_template(
    model: FirmwareModel,
    source: &Path,
    firmware: &[u8],
    scratch: &Path,
    loader_base: u32,
    static_patches: &[&str],
    _patch_rows: &HashMap<&'static str, Vec<HookRow>>,
) -> Result<(Vec<u8>, Vec<Window>, serde_json::Value), Box<dyn std::error::Error>> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let patches = repo.join("patches");
    let loader_root = patches.join("patch-loader").canonicalize()?;
    let midi_root = patches
        .join("midi-sync")
        .join(if model == FirmwareModel::L6 {
            "l6"
        } else {
            "l6max"
        })
        .canonicalize()?;
    let firmware_base = if model == FirmwareModel::L6 {
        L6_MAIN_BASE
    } else {
        0x80000000
    };
    let (module_base, reserve_end) = if model == FirmwareModel::L6 {
        (0x800c0000u32, 0x800c8000u32)
    } else {
        (0x80110000u32, 0x80118000u32)
    };

    let sentinel_source = scratch.join(if model == FirmwareModel::L6 {
        "loader-l6-sentinel"
    } else {
        "loader-l6max-sentinel"
    });
    fs::create_dir_all(&sentinel_source)?;
    let midi_profile = [midi_root.as_path()];
    let continuation_roots: Vec<&Path> = midi_profile.to_vec();
    let output = scratch.join(if model == FirmwareModel::L6 {
        "loader-compile-out-l6"
    } else {
        "loader-compile-out-l6max"
    });
    fs::create_dir_all(&output)?;
    let name = "patch-loader";
    let compile_one =
        |input: &Path, sentinel: bool| -> Result<(Vec<u8>, String), Box<dyn std::error::Error>> {
            if model == FirmwareModel::L6 {
                if sentinel {
                    compile_sentinel_l6_patch_loader(
                        input,
                        &loader_root,
                        &output,
                        name,
                        loader_base,
                        &continuation_roots,
                    )
                } else {
                    compile_l6_patch_loader(
                        input,
                        &loader_root,
                        &output,
                        name,
                        loader_base,
                        &continuation_roots,
                    )
                }
            } else if sentinel {
                compile_sentinel_with_continuations(
                    input,
                    &loader_root,
                    &continuation_roots,
                    &output,
                    name,
                    loader_base,
                )
            } else {
                compile_with_continuations(
                    input,
                    &loader_root,
                    &continuation_roots,
                    &output,
                    name,
                    loader_base,
                )
            }
        };
    let (real, symbols) = compile_one(source, false)?;
    let loader_elf = output.join("patch-loader.payload.elf");
    let loader_hook_sites = function_hook_sites(&loader_elf)?;
    let loader_registrations = hook_registrations(&symbols)?;
    let loader_windows = hook_windows(&loader_hook_sites, &loader_registrations)?;

    // The loader payload contains stock bytes for its own hook windows and
    // for every cataloged dynamic dispatcher fallback. Derive the sentinel
    // mask from those same metadata sources so templates cannot drift from
    // the hook attributes or catalog.
    let mut rows = Vec::new();
    for site in loader_hook_sites
        .iter()
        .filter(|site| site.dynamic_dispatch)
    {
        rows.push(HookRow {
            address: site.address,
            symbol: format!("original_{}", site.name),
            length: usize::from(site.displaced_len),
            continuation_kind: match site.continuation {
                1 => "cbz",
                2 => "relocated_bl",
                _ => "linear",
            }
            .to_owned(),
            continuation_site: None,
            continuation_slot_offset: None,
            preserved_instructions: preserved_instructions(
                firmware,
                firmware_base,
                site.address,
                usize::from(site.displaced_len),
                site.continuation,
            )?,
        });
    }
    for window in &loader_windows {
        if !rows
            .iter()
            .any(|row| row.address == window.address && row.length == window.length)
        {
            rows.push(HookRow {
                address: window.address,
                symbol: window.target_symbol.clone(),
                length: window.length,
                continuation_kind: if window.continuation == 1 {
                    "cbz".to_owned()
                } else {
                    "linear".to_owned()
                },
                continuation_site: None,
                continuation_slot_offset: None,
                preserved_instructions: Vec::new(),
            });
        }
    }
    let (sentinel, value_to_address) = build_sentinel_firmware(firmware, &rows, firmware_base);
    fs::write(sentinel_source.join("main_firmware.bin"), &sentinel)?;
    let (masked, _) = compile_one(&sentinel_source, true)?;
    let windows = find_windows(&real, &masked, &value_to_address)?;
    let mut template = real.clone();
    for window in &windows {
        template[window.payload_offset..window.payload_offset + window.length].fill(0);
    }
    let mut reconstructed = template.clone();
    for window in &windows {
        let offset = usize::try_from(window.firmware_address - firmware_base)?;
        reconstructed[window.payload_offset..window.payload_offset + window.length]
            .copy_from_slice(&firmware[offset..offset + window.length]);
    }
    if reconstructed != real {
        return Err("Patch loader template windows do not reconstruct the linked payload".into());
    }
    if loader_base + template.len() as u32 > module_base {
        return Err("Patch loader payload overlaps its reserved module region".into());
    }

    let mut sites = Vec::new();
    for window in &loader_windows {
        sites.push(loader_site(
            firmware,
            firmware_base,
            loader_base,
            window.address,
            window.length,
            &window.target_symbol,
            false,
            &symbols,
        )?);
    }
    for site in loader_hook_sites
        .iter()
        .filter(|site| site.dynamic_dispatch)
    {
        let dispatcher = format!("dispatch_{}", site.name);
        let mut dispatch = loader_site(
            firmware,
            firmware_base,
            loader_base,
            site.address,
            usize::from(site.displaced_len),
            &dispatcher,
            false,
            &symbols,
        )?;
        dispatch["dynamic_dispatch"] = serde_json::json!(true);
        sites.push(dispatch);
    }
    let metadata = serde_json::json!({
        "model": if model == FirmwareModel::L6 { "L6" } else { "L6max" },
        "base": format!("{loader_base:#x}"),
        "static_patches": static_patches,
        "reserve_end": format!("{reserve_end:#x}"),
        "module_base": format!("{module_base:#x}"),
        "payload_length": template.len(),
        "windows": windows.iter().map(|window| serde_json::json!({"payload_offset":window.payload_offset,"firmware_address":format!("{:#x}",window.firmware_address),"length":window.length})).collect::<Vec<_>>(),
        "sites": sites,
    });
    Ok((template, windows, metadata))
}

fn preserved_instructions(
    firmware: &[u8],
    firmware_base: u32,
    address: u32,
    length: usize,
    strategy: u8,
) -> Result<Vec<(usize, usize)>, Box<dyn std::error::Error>> {
    if strategy == 1 {
        return Ok(vec![(0, 2)]);
    }
    if strategy != 2 {
        return Ok(Vec::new());
    }
    let start = usize::try_from(
        address
            .checked_sub(firmware_base)
            .ok_or("site before image")?,
    )?;
    let displaced = firmware
        .get(start..start.checked_add(length).ok_or("site length overflow")?)
        .ok_or("site outside firmware")?;
    let mut preserved = Vec::new();
    let mut offset = 0;
    while offset + 4 <= displaced.len() {
        let first = u16::from_le_bytes(displaced[offset..offset + 2].try_into()?);
        let second = u16::from_le_bytes(displaced[offset + 2..offset + 4].try_into()?);
        if first & 0xf800 == 0xf000 && second & 0xd000 == 0xd000 {
            preserved.push((offset, offset + 4));
            offset += 4;
        } else {
            offset += 2;
        }
    }
    Ok(preserved)
}

/// One precompiled build unit: a patch's payload at one specific base
/// address, with verbatim displaced bytes removed, plus everything `firmware-patch-core::apply_patch`
/// needs once the browser fills in `windows` from the user's own upload.
struct Unit {
    name: &'static str,
    base: u32,
    template: Vec<u8>,
    windows: Vec<Window>,
    hooks: Vec<(u32, u32, usize)>, // (address, target, length)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let source = PathBuf::from(args.next().ok_or(
        "Usage: firmware-patch-wasm-prebuild L6MAX_UNPACKED_FIRMWARE_DIR L6_UNPACKED_FIRMWARE_DIR",
    )?)
    .canonicalize()?;
    let l6_source = PathBuf::from(args.next().ok_or(
        "Usage: firmware-patch-wasm-prebuild L6MAX_UNPACKED_FIRMWARE_DIR L6_UNPACKED_FIRMWARE_DIR",
    )?)
    .canonicalize()?;
    if args.next().is_some() {
        return Err("expected exactly two unpacked firmware directories".into());
    }
    let firmware = fs::read(source.join("main_firmware.bin"))?;
    let digest = format!("{:x}", Sha256::digest(&firmware));
    if digest != EXPECTED_FIRMWARE_SHA256 {
        return Err(format!(
            "unsupported main firmware SHA-256: expected {EXPECTED_FIRMWARE_SHA256}, got {digest}"
        )
        .into());
    }
    verify_firmware_for(FirmwareModel::L6max, &firmware)?;
    let l6_firmware = fs::read(l6_source.join("main_firmware.bin"))?;
    verify_firmware_for(FirmwareModel::L6, &l6_firmware)?;

    let patches_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../patches");
    let scratch = std::env::temp_dir().join(format!(
        "firmware-patch-wasm-prebuild-{}",
        std::process::id()
    ));
    fs::create_dir_all(&scratch)?;
    let output_scratch = scratch.join("compile-out");
    fs::create_dir_all(&output_scratch)?;

    // Per patch: its hook windows, derived from emitted attribute metadata, and a
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
        let patch_root = patches_dir.join(name);
        let patch_root = if name == "midi-sync" || name == "midi-control" {
            patch_root.join("l6max")
        } else {
            patch_root
        };
        let root = patch_root.canonicalize()?;
        let rows = compile_hook_rows(
            &source,
            &root,
            &output_scratch,
            name,
            &firmware,
            0x8000_0000,
        )?;
        let (sentinel, value_to_address) = build_sentinel_firmware(&firmware, &rows, 0x80000000);
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
    let patch_rows = per_patch
        .iter()
        .map(|(name, info)| (*name, info.rows.clone()))
        .collect::<HashMap<_, _>>();

    let mut units: HashMap<(&'static str, u32), Unit> = HashMap::new();
    for mask in 1u16..(1u16 << PATCHES.len()) {
        let subset: Vec<&'static str> = PATCHES
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1u16 << i) != 0)
            .map(|(_, name)| *name)
            .collect();
        let mut base = FIRST_PAYLOAD;
        for name in subset {
            if !units.contains_key(&(name, base)) {
                let info = &per_patch[name];
                let (real_payload, symbols) =
                    compile(&source, &info.root, &output_scratch, name, base)?;
                let (sentinel_payload, _) = compile_sentinel(
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
                units.insert(
                    (name, base),
                    Unit {
                        name,
                        base,
                        template,
                        windows,
                        hooks,
                    },
                );
            }
            base = {
                let len = units[&(name, base)].template.len() as u32;
                (base + len + 15) & !15
            };
        }
    }
    // Additional position-dependent feature templates for loader-enabled
    // images. The native builder places the loader first, then appends static
    // feature payloads; preserve that order in the browser output as well.
    for mask in 1u16..(1u16 << PATCHES.len()) {
        let subset: Vec<&'static str> = PATCHES
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1u16 << index) != 0)
            .map(|(_, name)| *name)
            .collect();
        let (loader_template, _, _) = build_loader_template(
            FirmwareModel::L6max,
            &source,
            &firmware,
            &scratch,
            FIRST_PAYLOAD,
            &subset,
            &patch_rows,
        )?;
        let mut base = (FIRST_PAYLOAD + loader_template.len() as u32 + 15) & !15;
        for name in subset {
            if !units.contains_key(&(name, base)) {
                let info = &per_patch[name];
                let (real_payload, symbols) =
                    compile(&source, &info.root, &output_scratch, name, base)?;
                let (sentinel_payload, _) = compile_sentinel(
                    &info.sentinel_source,
                    &info.root,
                    &output_scratch,
                    name,
                    base,
                )?;
                let windows =
                    find_windows(&real_payload, &sentinel_payload, &info.value_to_address)?;
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
                        "loader-enabled {name} at {base:#x}: template windows do not reconstruct payload"
                    ).into());
                }
                let hooks = info
                    .rows
                    .iter()
                    .map(|row| Ok((row.address, symbol(&symbols, &row.symbol)?, row.length)))
                    .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
                units.insert(
                    (name, base),
                    Unit {
                        name,
                        base,
                        template,
                        windows,
                        hooks,
                    },
                );
            }
            let len = units[&(name, base)].template.len() as u32;
            base = (base + len + 15) & !15;
        }
    }
    assert!(
        units.len() >= PATCHES.len(),
        "expected standard build units"
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
            .zip(&per_patch[unit.name].rows)
            .map(|((address, target, length), row)| {
                serde_json::json!({
                    "address": format!("{address:#x}"),
                    "target": format!("{target:#x}"),
                    "length": length,
                    "symbol": row.symbol,
                    "continuation_site": row.continuation_site,
                    "continuation_slot_offset": row.continuation_slot_offset,
                    "continuation_site": row.continuation_site,
                    "continuation_slot_offset": row.continuation_slot_offset,
                })
            })
            .collect();
        let metadata = serde_json::json!({
            "name": unit.name,
            "model": "L6max",
            "base": format!("{:#x}", unit.base),
            "payload_file": format!("{stem}.bin"),
            "payload_length": unit.template.len(),
            "windows": windows_json,
            "hooks": hooks_json,
            "window_byte_total": unit.windows.iter().map(|w| w.length).sum::<usize>(),
        });
        fs::write(
            templates_dir.join(format!("{stem}.json")),
            serde_json::to_string_pretty(&metadata)?,
        )?;
        manifest_units.push(serde_json::json!({"name": unit.name, "model": "L6max", "base": format!("{:#x}", unit.base), "metadata_file": format!("{stem}.json")}));
        println!(
            "{stem}: {} bytes, {} window(s), {} window byte(s)",
            unit.template.len(),
            unit.windows.len(),
            unit.windows.iter().map(|w| w.length).sum::<usize>()
        );
    }

    // Build the supported L6 profiles at the addresses selected by the native
    // CLI, then zero only firmware-derived continuation windows in browser templates.
    let l6_root = patches_dir.join("midi-sync/l6").canonicalize()?;
    let l6_rows = compile_hook_rows(
        &l6_source,
        &l6_root,
        &output_scratch,
        "midi-sync-l6",
        &l6_firmware,
        L6_MAIN_BASE,
    )?;
    let l6_base = first_payload_for(FirmwareModel::L6, l6_firmware.len());
    let (l6_sentinel, l6_value_to_address) =
        build_sentinel_firmware(&l6_firmware, &l6_rows, L6_MAIN_BASE);
    let l6_sentinel_source = scratch.join("l6-midi-sync-sentinel");
    fs::create_dir_all(&l6_sentinel_source)?;
    fs::write(l6_sentinel_source.join("main_firmware.bin"), &l6_sentinel)?;
    let (l6_payload, l6_symbols) = compile(
        &l6_source,
        &l6_root,
        &output_scratch,
        "l6-midi-sync",
        l6_base,
    )?;
    let (l6_sentinel_payload, _) = compile_sentinel(
        &l6_sentinel_source,
        &l6_root,
        &output_scratch,
        "l6-midi-sync",
        l6_base,
    )?;
    let l6_windows = find_windows(&l6_payload, &l6_sentinel_payload, &l6_value_to_address)?;
    let mut l6_template = l6_payload.clone();
    for window in &l6_windows {
        l6_template[window.payload_offset..window.payload_offset + window.length].fill(0);
    }
    let mut l6_reconstructed = l6_template.clone();
    for window in &l6_windows {
        let start = (window.firmware_address - L6_MAIN_BASE) as usize;
        l6_reconstructed[window.payload_offset..window.payload_offset + window.length]
            .copy_from_slice(&l6_firmware[start..start + window.length]);
    }
    if l6_reconstructed != l6_payload {
        return Err(
            "L6 midi-sync: template + L6 firmware windows do not reconstruct payload".into(),
        );
    }
    let l6_hooks = l6_rows
        .iter()
        .map(|row| Ok((row.address, symbol(&l6_symbols, &row.symbol)?, row.length)))
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    let l6_stem = format!("midi-sync-l6-{l6_base:08x}");
    fs::write(templates_dir.join(format!("{l6_stem}.bin")), &l6_template)?;
    let l6_windows_json: Vec<_> = l6_windows
        .iter()
        .map(|window| {
            serde_json::json!({
                "payload_offset": window.payload_offset,
                "firmware_address": format!("{:#x}", window.firmware_address),
                "length": window.length,
            })
        })
        .collect();
    let l6_hooks_json: Vec<_> = l6_hooks
        .iter()
        .zip(&l6_rows)
        .map(|((address, target, length), row)| {
            serde_json::json!({
                "address": format!("{address:#x}"),
                "target": format!("{target:#x}"),
                "length": length,
                "symbol": row.symbol,
                "continuation_site": row.continuation_site,
                "continuation_slot_offset": row.continuation_slot_offset,
                "continuation_site": row.continuation_site,
                "continuation_slot_offset": row.continuation_slot_offset,
            })
        })
        .collect();
    let l6_metadata = serde_json::json!({
        "name": "midi-sync",
        "model": "L6",
        "base": format!("{l6_base:#x}"),
        "payload_file": format!("{l6_stem}.bin"),
        "payload_length": l6_template.len(),
        "windows": l6_windows_json,
        "hooks": l6_hooks_json,
        "window_byte_total": l6_windows.iter().map(|w| w.length).sum::<usize>(),
    });
    let l6_metadata_file = format!("{l6_stem}.json");
    fs::write(
        templates_dir.join(&l6_metadata_file),
        serde_json::to_string_pretty(&l6_metadata)?,
    )?;
    manifest_units.push(serde_json::json!({
        "name": "midi-sync",
        "model": "L6",
        "base": format!("{l6_base:#x}"),
        "metadata_file": l6_metadata_file,
    }));
    println!(
        "{}: {} bytes, {} window(s), {} window byte(s)",
        l6_stem,
        l6_template.len(),
        l6_windows.len(),
        l6_windows.iter().map(|w| w.length).sum::<usize>()
    );

    let (l6_loader_template, _, _) = build_loader_template(
        FirmwareModel::L6,
        &l6_source,
        &l6_firmware,
        &scratch,
        l6_base,
        &["midi-sync"],
        &patch_rows,
    )?;
    let l6_static_base = (l6_base + l6_loader_template.len() as u32 + 15) & !15;
    let (loader_l6_payload, loader_l6_symbols) = compile(
        &l6_source,
        &l6_root,
        &output_scratch,
        "midi-sync",
        l6_static_base,
    )?;
    let (loader_l6_sentinel_payload, _) = compile_sentinel(
        &l6_sentinel_source,
        &l6_root,
        &output_scratch,
        "midi-sync",
        l6_static_base,
    )?;
    let loader_l6_windows = find_windows(
        &loader_l6_payload,
        &loader_l6_sentinel_payload,
        &l6_value_to_address,
    )?;
    let mut loader_l6_template = loader_l6_payload.clone();
    for window in &loader_l6_windows {
        loader_l6_template[window.payload_offset..window.payload_offset + window.length].fill(0);
    }
    let mut loader_l6_reconstructed = loader_l6_template.clone();
    for window in &loader_l6_windows {
        let start = (window.firmware_address - L6_MAIN_BASE) as usize;
        loader_l6_reconstructed[window.payload_offset..window.payload_offset + window.length]
            .copy_from_slice(&l6_firmware[start..start + window.length]);
    }
    if loader_l6_reconstructed != loader_l6_payload {
        return Err("loader-enabled L6 midi-sync template does not reconstruct payload".into());
    }
    let loader_l6_hooks = l6_rows
        .iter()
        .map(|row| {
            Ok((
                row.address,
                symbol(&loader_l6_symbols, &row.symbol)?,
                row.length,
            ))
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    let loader_l6_stem = format!("midi-sync-l6-loader-{l6_static_base:08x}");
    fs::write(
        templates_dir.join(format!("{loader_l6_stem}.bin")),
        &loader_l6_template,
    )?;
    let loader_l6_metadata = serde_json::json!({
        "name": "midi-sync", "model": "L6", "base": format!("{l6_static_base:#x}"),
        "payload_file": format!("{loader_l6_stem}.bin"), "payload_length": loader_l6_template.len(),
        "windows": loader_l6_windows.iter().map(|window| serde_json::json!({
            "payload_offset":window.payload_offset,
            "firmware_address":format!("{:#x}",window.firmware_address),
            "length":window.length,
        })).collect::<Vec<_>>(),
        "hooks": loader_l6_hooks.iter().zip(&l6_rows).map(|((address,target,length),row)| serde_json::json!({
            "address":format!("{address:#x}"),"target":format!("{target:#x}"),"length":length,"symbol":row.symbol,
        })).collect::<Vec<_>>(),
        "window_byte_total": loader_l6_windows.iter().map(|window| window.length).sum::<usize>(),
    });
    let loader_l6_metadata_file = format!("{loader_l6_stem}.json");
    fs::write(
        templates_dir.join(&loader_l6_metadata_file),
        serde_json::to_string_pretty(&loader_l6_metadata)?,
    )?;
    manifest_units.push(serde_json::json!({
        "name":"midi-sync", "model":"L6", "base":format!("{l6_static_base:#x}"),
        "metadata_file":loader_l6_metadata_file,
    }));

    let mut loader_manifest = Vec::new();
    let mut loader_profiles = Vec::new();
    for mask in 0u8..(1 << PATCHES.len()) {
        let selected: Vec<&'static str> = PATCHES
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, name)| *name)
            .collect();
        loader_profiles.push((
            FirmwareModel::L6max,
            source.as_path(),
            firmware.as_slice(),
            mask,
            selected,
            FIRST_PAYLOAD,
        ));
    }
    for mask in 0u8..2 {
        let selected = if mask == 0 { vec![] } else { vec!["midi-sync"] };
        loader_profiles.push((
            FirmwareModel::L6,
            l6_source.as_path(),
            l6_firmware.as_slice(),
            mask,
            selected,
            l6_base,
        ));
    }
    for (model, loader_source, loader_firmware, mask, selected, base) in loader_profiles {
        let (loader_template, loader_windows, mut metadata) = build_loader_template(
            model,
            loader_source,
            loader_firmware,
            &scratch,
            base,
            &selected,
            &patch_rows,
        )?;
        let model_name = if model == FirmwareModel::L6 {
            "L6"
        } else {
            "L6max"
        };
        let stem = format!("patch-loader-{model_name}-{mask:03}-{base:08x}");
        let payload_file = format!("patch-loader-{model_name}-{base:08x}.bin");
        let payload_path = templates_dir.join(&payload_file);
        if payload_path.exists() {
            if fs::read(&payload_path)? != loader_template {
                return Err(format!("loader payload changed across {model_name} profiles").into());
            }
        } else {
            fs::write(&payload_path, &loader_template)?;
        }
        metadata["name"] = serde_json::json!("patch-loader");
        metadata["payload_file"] = serde_json::json!(payload_file);
        fs::write(
            templates_dir.join(format!("{stem}.json")),
            serde_json::to_string_pretty(&metadata)?,
        )?;
        loader_manifest.push(serde_json::json!({
            "model": model_name,
            "base": format!("{base:#x}"),
            "metadata_file": format!("{stem}.json"),
        }));
        println!(
            "{stem}: {} bytes, {} firmware window(s), reserves through {}",
            loader_template.len(),
            loader_windows.len(),
            metadata["reserve_end"].as_str().unwrap_or("?")
        );
    }

    let loader_template_count = loader_manifest.len();
    fs::write(
        templates_dir.join("manifest.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "source_firmware_sha256": digest,
            "l6_source_firmware_sha256": L6_EXPECTED_FIRMWARE_SHA256,
            "units": manifest_units,
            "loaders": loader_manifest,
        }))?,
    )?;
    fs::remove_dir_all(&scratch)?;
    println!(
        "Wrote {} feature units and {} loader templates (Zoom-byte-free) to {}",
        unit_list.len() + 2,
        loader_template_count,
        templates_dir.display()
    );
    Ok(())
}
