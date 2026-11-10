//! Reproducible, revision-guarded firmware experiment. Never edits its input.
mod sd_bundle;
use firmware_patch_compile::{
    FunctionHookSiteMetadata, HookRegistration, compile, compile_l6_patch_loader, compile_sd,
    compile_sd_relocatable, compile_with_continuations, compile_with_continuations_cfg,
    decode_thumb_bl_target, function_hook_sites, hook_registrations, symbol,
};
use firmware_patch_core::{
    FirmwareModel, HookEdit, L6_MAIN_BASE, L6_SLOT_END, PATCHES, PatchEdits, apply_patch_with_map,
    branch, first_payload_for, verify_firmware_for, write_image_length_and_version,
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};
const USAGE: &str = "Usage: l6patch INPUT_UNPACKED_DIR OUTPUT_DIR [--patch NAME]... [--update-version FOUR_DIGITS] [--experimental-patch-bundle | --experimental-patch-loader] [--experimental-qemu-module-arena]\n\
Patches: knob-dialog, midi-control, midi-sync, din-mixer-control. Repeat --patch to combine them. MIDI Control and MIDI Sync can be combined. The Patch loader is experimental and unqualified.";

const SD_MODULE_BASE: u32 = 0x8011_0000;
const SD_MODULE_END: u32 = 0x8011_8000;
const L6_SD_MODULE_BASE: u32 = 0x800c_0000;
const L6_SD_MODULE_END: u32 = 0x800c_8000;

struct Options {
    source: PathBuf,
    output: PathBuf,
    patches: Option<Vec<&'static str>>,
    version: Option<String>,
    experimental_sd_bundle: bool,
    experimental_patch_loader: bool,
    experimental_qemu_module_arena: bool,
}
fn parse_args() -> Result<Options, Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(source), Some(output)) = (args.next(), args.next()) else {
        return Err(USAGE.into());
    };
    let mut requested = Vec::new();
    let mut version = None;
    let mut experimental_sd_bundle = false;
    let mut experimental_patch_loader = false;
    let mut experimental_qemu_module_arena = false;
    while let Some(flag) = args.next() {
        if flag == "--experimental-patch-bundle" {
            experimental_sd_bundle = true;
            continue;
        }
        if flag == "--experimental-patch-loader" {
            experimental_patch_loader = true;
            continue;
        }
        if flag == "--experimental-qemu-module-arena" {
            experimental_qemu_module_arena = true;
            continue;
        }
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
        None
    } else {
        Some(
            PATCHES
                .into_iter()
                .filter(|n| requested.contains(n))
                .collect(),
        )
    };
    if version
        .as_ref()
        .is_some_and(|version| version.len() != 4 || !version.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("update version must be exactly four ASCII digits, e.g. 0112".into());
    }
    if experimental_sd_bundle && experimental_patch_loader {
        return Err("build the loader and SD bundle in separate output directories".into());
    }
    if experimental_qemu_module_arena && !experimental_patch_loader {
        return Err("the QEMU module arena requires --experimental-patch-loader".into());
    }
    Ok(Options {
        source: PathBuf::from(source),
        output: PathBuf::from(output),
        patches,
        version,
        experimental_sd_bundle,
        experimental_patch_loader,
        experimental_qemu_module_arena,
    })
}
#[derive(Debug)]
struct ResolvedHook {
    address: u32,
    symbol: String,
    target: u32,
    length: usize,
    guard_sha256: String,
    continuation_site: Option<String>,
}

struct CompiledPatch {
    name: &'static str,
    base: u32,
    payload: Vec<u8>,
    symbols: String,
    registrations: Vec<HookRegistration>,
    hooks: Vec<ResolvedHook>,
}

fn resolve_compiled_hooks(
    firmware: &[u8],
    firmware_base: u32,
    symbols: &str,
    sites: &[FunctionHookSiteMetadata],
    registrations: &[HookRegistration],
) -> Result<Vec<ResolvedHook>, Box<dyn std::error::Error>> {
    let mut hooks = Vec::new();
    let mut call_dispatch_sites = std::collections::BTreeSet::new();
    for registration in registrations {
        if let HookRegistration::CallPre { function, .. }
        | HookRegistration::CallPost { function, .. } = registration
        {
            if !call_dispatch_sites.insert(function.as_str()) {
                continue;
            }
            let site = sites
                .iter()
                .find(|candidate| candidate.name == *function)
                .ok_or_else(|| {
                    format!("call hook function {function} is absent from define_function_hooks!")
                })?;
            let length = usize::from(site.displaced_len);
            let offset = usize::try_from(
                site.address
                    .checked_sub(firmware_base)
                    .ok_or("hook before firmware")?,
            )?;
            let region = firmware
                .get(offset..offset.checked_add(length).ok_or("hook length overflow")?)
                .ok_or_else(|| format!("hook region outside firmware at {:#x}", site.address))?;
            let guard_sha256 = format!("{:x}", Sha256::digest(region));
            if guard_sha256 != site.guard_sha256 {
                return Err(format!("function hook guard failed: {function}").into());
            }
            hooks.push(ResolvedHook {
                address: site.address,
                symbol: format!("call_dispatch:{function}"),
                target: symbol(symbols, &format!("__l6_call_dispatch_{function}"))?,
                length,
                guard_sha256,
                continuation_site: Some(function.clone()),
            });
            continue;
        }
        let (site_name, address, handler, target_symbol) = match registration {
            HookRegistration::Entry { site, handler } => {
                let site = sites
                    .iter()
                    .find(|candidate| candidate.name == *site)
                    .ok_or_else(|| {
                        format!("entry hook {site} is absent from define_function_hooks!")
                    })?;
                (site.name.as_str(), site.address, handler, handler.clone())
            }
            HookRegistration::Offset {
                function,
                offset,
                handler,
                ..
            } => {
                let site = sites
                    .iter()
                    .find(|candidate| candidate.name == *function)
                    .ok_or_else(|| {
                        format!(
                            "offset hook function {function} is absent from define_function_hooks!"
                        )
                    })?;
                let address = site
                    .address
                    .checked_add(*offset)
                    .ok_or("offset hook address overflow")?;
                (
                    site.name.as_str(),
                    address,
                    handler,
                    format!("__l6_offset_veneer_{handler}"),
                )
            }
            HookRegistration::CallPre { .. } | HookRegistration::CallPost { .. } => unreachable!(),
        };
        let site = sites
            .iter()
            .find(|candidate| candidate.name == site_name)
            .ok_or_else(|| {
                format!("hook site {site_name} is absent from define_function_hooks!")
            })?;
        let length = match registration {
            HookRegistration::Offset { displaced_len, .. } => usize::from(*displaced_len),
            _ => usize::from(site.displaced_len),
        };
        let offset = usize::try_from(
            address
                .checked_sub(firmware_base)
                .ok_or("hook before firmware")?,
        )?;
        let region = firmware
            .get(offset..offset.checked_add(length).ok_or("hook length overflow")?)
            .ok_or_else(|| format!("hook region outside firmware at {address:#x}"))?;
        let guard_sha256 = format!("{:x}", Sha256::digest(region));
        match registration {
            HookRegistration::Entry { .. } if guard_sha256 != site.guard_sha256 => {
                return Err(format!("function hook guard failed: {site_name}").into());
            }
            HookRegistration::Offset {
                guard_sha256: expected,
                ..
            } if guard_sha256 != *expected => {
                return Err(
                    format!("offset hook guard failed: {site_name} at {address:#x}").into(),
                );
            }
            _ => {}
        }
        hooks.push(ResolvedHook {
            address,
            symbol: handler.clone(),
            target: symbol(symbols, &target_symbol)?,
            length,
            guard_sha256,
            continuation_site: match registration {
                HookRegistration::Entry { site, .. } => Some(site.clone()),
                HookRegistration::Offset { .. } => None,
                HookRegistration::CallPre { .. } | HookRegistration::CallPost { .. } => {
                    unreachable!()
                }
            },
        });
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
    let mut firmware = fs::read(source.join("main_firmware.bin"))?;
    let stock_firmware = firmware.clone();
    let original_length = firmware.len();
    let model = l6fw::model_from_main(&firmware)?;
    let (patch_model, firmware_base, slot_end) = match model {
        l6fw::Model::L6 => (FirmwareModel::L6, L6_MAIN_BASE, L6_SLOT_END),
        l6fw::Model::L6max => (FirmwareModel::L6max, 0x80000000, 0x801a2ff8),
    };
    if options.experimental_qemu_module_arena && model != l6fw::Model::L6max {
        return Err("the experimental QEMU module arena is L6max-only".into());
    }
    let (module_base, module_end) = match model {
        l6fw::Model::L6 => (L6_SD_MODULE_BASE, L6_SD_MODULE_END),
        l6fw::Model::L6max => (SD_MODULE_BASE, SD_MODULE_END),
    };
    let patches = options.patches.unwrap_or_else(|| match model {
        _ if options.experimental_patch_loader || options.experimental_sd_bundle => vec![],
        l6fw::Model::L6 => vec!["midi-sync"],
        l6fw::Model::L6max => vec!["knob-dialog"],
    });
    if model == l6fw::Model::L6 && patches.iter().any(|name| *name != "midi-sync") {
        return Err("L6 firmware detected; only the midi-sync patch is supported".into());
    }
    // The original knob-only build keeps its published marker; anything else is newer.
    let version = options.version.unwrap_or_else(|| {
        if options.experimental_patch_loader {
            "0114"
        } else if patches == ["knob-dialog"] {
            "0112"
        } else {
            "0113"
        }
        .into()
    });
    if version.len() != 4 || !version.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("update version must be exactly four ASCII digits, e.g. 0112".into());
    }
    verify_firmware_for(patch_model, &firmware)?;
    let digest = format!("{:x}", Sha256::digest(&firmware));
    // Validate every hook region before compiling or modifying the output image.
    let roots = patches
        .iter()
        .map(|name| {
            let patch_root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../patches")
                .join(name);
            let patch_root = if *name == "midi-sync" || *name == "midi-control" {
                patch_root.join(if patch_model == FirmwareModel::L6 {
                    "l6"
                } else {
                    "l6max"
                })
            } else {
                patch_root
            };
            patch_root.canonicalize()
        })
        .collect::<Result<Vec<_>, _>>()?;
    if options.experimental_sd_bundle {
        if patches.is_empty() {
            return Err("dynamic bundle needs at least one --patch".into());
        }
        fs::create_dir_all(&options.output)?;
        let output = options.output.canonicalize()?;
        let patch_dir = output.join("PATCHES");
        fs::create_dir_all(&patch_dir)?;
        let mut reports = Vec::new();
        for (name, root) in patches.iter().zip(&roots) {
            let (module, memory_len, symbols, relocations) =
                compile_sd_relocatable(&source, root, &output, name, 0)?;
            let sites = function_hook_sites(&output.join(format!("{name}.payload.elf")))?;
            let registrations = hook_registrations(&symbols)?;
            if registrations.is_empty() {
                return Err(format!(
                    "dynamic bundles require at least one hook registration; {name} has none"
                )
                .into());
            }
            let firmware_base = match model {
                l6fw::Model::L6 => 0x8000_1000,
                l6fw::Model::L6max => 0x8000_0000,
            };
            let image = fs::read(source.join("main_firmware.bin"))?;
            let mut handlers = Vec::with_capacity(registrations.len());
            let mut pass_through = Vec::with_capacity(registrations.len());
            let mut registered_addresses = std::collections::BTreeSet::new();
            for registration in &registrations {
                let (site_name, handler_symbol) = match registration {
                    HookRegistration::Entry { site, handler } => (site.as_str(), handler.clone()),
                    HookRegistration::CallPre {
                        function,
                        address,
                        handler,
                        ..
                    }
                    | HookRegistration::CallPost {
                        function,
                        address,
                        handler,
                        ..
                    } => {
                        let target = decode_thumb_bl_target(&image, firmware_base, *address)?;
                        let function_site = sites.iter().find(|site| site.name == *function)
                            .ok_or_else(|| format!("call hook function {function} is missing from define_function_hooks!"))?;
                        if target != function_site.address {
                            return Err(format!(
                                "dynamic call hook at {address:#x} targets {target:#x}, expected {} at {:#x}",
                                function, function_site.address
                            ).into());
                        }
                        let phase = match registration {
                            HookRegistration::CallPre { .. } => "pre",
                            HookRegistration::CallPost { .. } => "post",
                            _ => unreachable!(),
                        };
                        (
                            function.as_str(),
                            format!("__l6_dynamic_call_{phase}_{handler}"),
                        )
                    }
                    HookRegistration::Offset { .. } => {
                        return Err("function-offset hooks are static-only".into());
                    }
                };
                let site = sites
                    .iter()
                    .find(|site| site.name == site_name && site.dynamic_dispatch)
                    .ok_or_else(|| {
                        format!("dynamic hook {site_name} is not a dynamic catalog site")
                    })?;
                if !registered_addresses.insert(site.address) {
                    return Err(format!(
                        "dynamic module {name} registers more than one handler for {site_name}; combine same-site behavior in one handler"
                    ).into());
                }
                let next_slot = symbol(&symbols, &format!("dynamic_next_{}_slot", site.name))?;
                handlers.push((site.address, symbol(&symbols, &handler_symbol)?, next_slot));
                pass_through.push(symbol(&symbols, &format!("original_{}", site.name))?);
            }
            if handlers.len() > patch_loader_core::dynamic::MAX_HOOKS_PER_MODULE {
                return Err(format!(
                    "dynamic module {name} has {} hooks; loader limit is {}",
                    handlers.len(),
                    patch_loader_core::dynamic::MAX_HOOKS_PER_MODULE
                )
                .into());
            }
            let file =
                sd_bundle::build_dynamic_v1(&module, memory_len, 0, &handlers, &relocations)?;
            let filename = format!("{}.BIN", name.to_ascii_uppercase());
            fs::write(patch_dir.join(&filename), &file)?;
            reports.push(serde_json::json!({
                "name": name,
                "file": format!("PATCHES/{filename}"),
                "payload_length": module.len(),
                "memory_length": memory_len,
                "relocation_count": relocations.len(),
                "pass_through_entry_offsets": pass_through,
                "hooks": handlers.iter().map(|(address, offset, next_offset)| serde_json::json!({"address":format!("{address:#x}"),"entry_offset":offset,"next_slot_offset":next_offset})).collect::<Vec<_>>(),
                "file_length": file.len(),
            }));
        }
        let report = serde_json::json!({
            "version": 9,
            "loader_abi": patch_loader_core::dynamic::LOADER_ABI,
            "model": if model == l6fw::Model::L6 { "L6" } else { "L6max" },
            "source_sha256": digest,
            "patches": reports,
        });
        fs::write(
            output.join("patch.json"),
            serde_json::to_vec_pretty(&report)?,
        )?;
        println!("Wrote per-patch files under {}", patch_dir.display());
        return Ok(());
    }
    let mut claimed: Vec<(u32, usize)> = Vec::new();
    let mut loader_dispatch_targets = std::collections::BTreeMap::<u32, u32>::new();
    fs::create_dir_all(&options.output)?;
    let output = options.output.canonicalize()?;
    let padding = fs::read(source.join("padding_before_secondary.bin"))?;
    // Payloads are placed back to back, each aligned to 16 bytes.
    let mut end = if options.experimental_sd_bundle {
        module_base
    } else {
        first_payload_for(patch_model, original_length)
    };
    let initial_payload = end;
    let mut applied = Vec::new();
    if options.experimental_patch_loader {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../patches/patch-loader")
            .canonicalize()?;
        let patches_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../patches");
        let midi_profile = if model == l6fw::Model::L6 {
            "l6"
        } else {
            "l6max"
        };
        let midi_root = patches_root.join("midi-sync").join(midi_profile);
        let mut continuation_roots = vec![midi_root.as_path()];
        let knob_root = patches_root.join("knob-dialog");
        if model == l6fw::Model::L6max {
            continuation_roots.push(knob_root.as_path());
        }
        let (payload, symbols) = if model == l6fw::Model::L6 {
            compile_l6_patch_loader(
                &source,
                &root,
                &output,
                "patch-loader",
                end,
                &continuation_roots,
            )?
        } else {
            if options.experimental_qemu_module_arena {
                compile_with_continuations_cfg(
                    &source,
                    &root,
                    &continuation_roots,
                    &output,
                    "patch-loader",
                    end,
                    &["sd_qemu_module_arena"],
                )?
            } else {
                compile_with_continuations(
                    &source,
                    &root,
                    &continuation_roots,
                    &output,
                    "patch-loader",
                    end,
                )?
            }
        };
        let catalog = function_hook_sites(&output.join("patch-loader.payload.elf"))?;
        let registrations = hook_registrations(&symbols)?;
        let hooks = resolve_compiled_hooks(
            &stock_firmware,
            firmware_base,
            &symbols,
            &catalog,
            &registrations,
        )?;
        for hook in &hooks {
            if claimed.iter().any(|&(address, length)| {
                hook.address < address + length as u32
                    && address < hook.address + hook.length as u32
            }) {
                return Err(format!("loader hook overlaps another patch: {}", hook.symbol).into());
            }
            claimed.push((hook.address, hook.length));
        }
        let edits = PatchEdits::new(
            hooks
                .iter()
                .map(|hook| HookEdit {
                    address: hook.address,
                    target: hook.target,
                    length: hook.length,
                })
                .collect(),
        );
        let placed = apply_patch_with_map(
            &mut firmware,
            original_length,
            end,
            &payload,
            &edits,
            &padding,
            firmware_base,
            module_base,
        )?;
        let installed_hooks = hooks
            .iter()
            .map(|hook| {
                serde_json::json!({
                    "address": hook.address,
                    "symbol": hook.symbol,
                    "target": hook.target,
                    "displaced_length": hook.length,
                    "displaced_sha256": hook.guard_sha256,
                })
            })
            .collect::<Vec<_>>();
        let mut dispatch_sites = Vec::new();
        for site in &catalog {
            if !site.dynamic_dispatch {
                continue;
            }
            let address = site.address;
            let length = usize::from(site.displaced_len);
            let offset = usize::try_from(
                address
                    .checked_sub(firmware_base)
                    .ok_or("hook before firmware")?,
            )?;
            let region = stock_firmware
                .get(offset..offset.checked_add(length).ok_or("hook length overflow")?)
                .ok_or_else(|| format!("function hook outside firmware at {address:#x}"))?;
            if format!("{:x}", Sha256::digest(region)) != site.guard_sha256 {
                return Err(format!("function hook guard failed: {}", site.name).into());
            }
            let dispatch = symbol(&symbols, &format!("dispatch_{}", site.name))?;
            loader_dispatch_targets.insert(address, dispatch | 1);
            let fallback = symbol(&symbols, &format!("original_{}", site.name))?;
            firmware[offset..offset + 4].copy_from_slice(&branch(address, dispatch, false));
            for chunk in firmware[offset + 4..offset + length].chunks_exact_mut(2) {
                chunk.copy_from_slice(&0xbf00u16.to_le_bytes());
            }
            dispatch_sites.push(serde_json::json!({"address":address,"target":dispatch,"fallback":fallback,"length":length,"function":site.name}));
        }
        applied.push(serde_json::json!({"name":"patch-loader", "payload_address":format!("{end:#x}"), "payload_length":payload.len(), "hooks":installed_hooks, "original_midi_in":format!("{:#x}", symbol(&symbols, "original_midi_in")?)}));
        applied.push(serde_json::json!({"name":"dynamic-dispatch", "sites":dispatch_sites}));
        end = placed.next_base;
    }
    let mut compiled_patches = Vec::new();
    let mut compile_base = end;
    for (name, root) in patches.iter().zip(&roots) {
        let (payload, symbols) = if options.experimental_sd_bundle {
            compile_sd(&source, root, &output, name, compile_base)?
        } else {
            compile(&source, root, &output, name, compile_base)?
        };
        let sites = function_hook_sites(&output.join(format!("{name}.payload.elf")))?;
        let registrations = hook_registrations(&symbols)?;
        let hooks = resolve_compiled_hooks(
            &stock_firmware,
            firmware_base,
            &symbols,
            &sites,
            &registrations,
        )?;
        compile_base = (compile_base + payload.len() as u32 + 15) & !15;
        compiled_patches.push(CompiledPatch {
            name,
            base: compile_base - ((payload.len() as u32 + 15) & !15),
            payload,
            symbols,
            registrations,
            hooks,
        });
    }

    // Compatible entry/call hooks are one chain. Each patch's continuation
    // slot points at the next patch, with the last one retaining stock code.
    let mut hook_owners = std::collections::BTreeMap::<u32, (usize, usize, Option<String>)>::new();
    for patch_index in 0..compiled_patches.len() {
        let patch_hooks = compiled_patches[patch_index].hooks.len();
        for hook_index in 0..patch_hooks {
            let (address, length, target, symbol_name, continuation_site) = {
                let hook = &compiled_patches[patch_index].hooks[hook_index];
                (
                    hook.address,
                    hook.length,
                    hook.target,
                    hook.symbol.clone(),
                    hook.continuation_site.clone(),
                )
            };
            if let Some((first_patch, first_hook, site)) = hook_owners.get(&address).cloned() {
                let first = &compiled_patches[first_patch].hooks[first_hook];
                if first.length != length
                    || site.is_none()
                    || continuation_site.as_ref() != site.as_ref()
                {
                    return Err(format!(
                        "incompatible hooks overlap at {:#x}: {}",
                        address, symbol_name
                    )
                    .into());
                }
                let slot_name = format!("static_next_{}_slot", site.as_ref().unwrap());
                let slot = symbol(&compiled_patches[first_patch].symbols, &slot_name)?;
                let slot_offset = usize::try_from(
                    slot.checked_sub(compiled_patches[first_patch].base)
                        .ok_or("chain slot precedes payload")?,
                )?;
                let next_target = target | 1;
                compiled_patches[first_patch].payload[slot_offset..slot_offset + 4]
                    .copy_from_slice(&next_target.to_le_bytes());
                // The first patch's continuation now leads to this patch; for
                // a longer chain, redirect this patch's own continuation next.
                hook_owners.insert(address, (patch_index, hook_index, site));
            } else {
                hook_owners.insert(address, (patch_index, hook_index, continuation_site));
            }
            if let (Some(site), Some(dispatch)) = (
                compiled_patches[patch_index].hooks[hook_index]
                    .continuation_site
                    .clone(),
                loader_dispatch_targets.get(&address).copied(),
            ) {
                let slot = symbol(
                    &compiled_patches[patch_index].symbols,
                    &format!("static_next_{site}_slot"),
                )?;
                let slot_offset = usize::try_from(
                    slot.checked_sub(compiled_patches[patch_index].base)
                        .ok_or("chain slot precedes payload")?,
                )?;
                compiled_patches[patch_index].payload[slot_offset..slot_offset + 4]
                    .copy_from_slice(&dispatch.to_le_bytes());
            }
        }
    }

    let mut installed_hook_sites = std::collections::BTreeSet::new();
    for patch in compiled_patches {
        let name = patch.name;
        let base = patch.base;
        let payload = patch.payload;
        let registrations = patch.registrations;
        let hooks = patch.hooks;
        let mut unique = std::collections::BTreeSet::new();
        let mut edits_vec = Vec::new();
        for hook in &hooks {
            if !unique.insert(hook.address) {
                return Err(format!(
                    "module {name} registers multiple hooks at {:#x}",
                    hook.address
                )
                .into());
            }
            if installed_hook_sites.contains(&hook.address) {
                continue;
            }
            let prior = claimed.iter().any(|&(address, length)| {
                hook.address < address + length as u32
                    && address < hook.address + hook.length as u32
            });
            if prior {
                return Err(format!("hook overlaps another patch: {}", hook.symbol).into());
            }
            claimed.push((hook.address, hook.length));
            installed_hook_sites.insert(hook.address);
            edits_vec.push(HookEdit {
                address: hook.address,
                target: hook.target,
                length: hook.length,
            });
        }
        let edits = PatchEdits::new(edits_vec);
        let applied_patch = apply_patch_with_map(
            &mut firmware,
            original_length,
            base,
            &payload,
            &edits,
            &padding,
            firmware_base,
            slot_end,
        )?;
        end = applied_patch.next_base;
        if options.experimental_patch_loader && end > module_base {
            return Err("installed patch payloads overlap the SD module region".into());
        }
        let mut applied_hooks = Vec::new();
        for (hook_edit, hook) in edits.hooks.iter().zip(&hooks) {
            applied_hooks.push(serde_json::json!({"address":hook.address,"symbol":hook.symbol,"target":hook_edit.target,"displaced_length":hook.length,"displaced_sha256":hook.guard_sha256}));
        }
        let call_site_hooks = registrations
            .iter()
            .filter_map(|registration| match registration {
                HookRegistration::CallPre { function, address, handler, .. } => Some(serde_json::json!({"phase":"pre","function":function,"address":format!("{address:#x}"),"handler":handler})),
                HookRegistration::CallPost { function, address, handler, .. } => Some(serde_json::json!({"phase":"post","function":function,"address":format!("{address:#x}"),"handler":handler})),
                _ => None,
            })
            .collect::<Vec<_>>();
        applied.push(serde_json::json!({"name":name,"payload_address":format!("{base:#x}"),"payload_length":applied_patch.payload_len,"hooks":applied_hooks,"call_site_hooks":call_site_hooks}));
    }
    if options.experimental_patch_loader {
        let reserved_end = (module_end - firmware_base) as usize;
        let consumed = reserved_end - original_length;
        if consumed > padding.len() || padding[..consumed].iter().any(|&byte| byte != 0xff) {
            return Err("SD module reserve overlaps non-erased package data".into());
        }
        firmware.resize(reserved_end, 0xff);
    }
    let consumed = firmware.len() - original_length;
    if options.experimental_sd_bundle {
        let regions = claimed.clone();
        let bundle = sd_bundle::build(
            &stock_firmware,
            &firmware,
            firmware_base,
            module_base,
            0,
            &regions,
        )?;
        if bundle.len() > 16 * 1024 {
            return Err("experimental SD bundle exceeds the loader's 16 KiB limit".into());
        }
        fs::write(output.join("experimental-patch-bundle.bin"), bundle)?;
    }
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
    write_image_length_and_version(&mut trailer, &mut footer, firmware.len() as u32, &version);
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
    let (update_name, trailer_offset) = match model {
        l6fw::Model::L6 => ("L6.BIN", 0x2851f8usize),
        l6fw::Model::L6max => ("L6max.bin", 0x1a31f8usize),
    };
    let update = output.join(update_name);
    l6fw::repack(&output, &update, true)?;
    let packed = fs::read(&update)?;
    if model == l6fw::Model::L6max {
        l6fw::update::validate(&packed)?;
    } else if l6fw::model_from_package(&packed)? != l6fw::Model::L6 {
        return Err("repacked package model changed unexpectedly".into());
    }
    let version_range = trailer_offset + 4..trailer_offset + 8;
    let report = serde_json::json!({"model":if model == l6fw::Model::L6 {"L6"} else {"L6max"},"source_sha256":digest,"patched_sha256":format!("{:x}",Sha256::digest(&firmware)),"image_length":firmware.len(),"payload_address":format!("{initial_payload:#x}"),"patches":applied,"update_package":update_name,"update_sha256":format!("{:x}",Sha256::digest(&packed)),"version":String::from_utf8_lossy(&packed[version_range]),"usage":"Repacked update for emulator validation; physical bootloader acceptance unverified"});
    fs::write(
        output.join("patch.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    println!(
        "MAIN image grew by {} bytes ({}); patched firmware: {}",
        firmware.len() - original_length,
        patches.join(" + "),
        output.display()
    );
    Ok(())
}

// Thumb-branch encoding is tested in `firmware-patch-core`, which owns `branch()`.

#[cfg(test)]
#[path = "../../../patches/knob-dialog/src/values.rs"]
mod values;

#[cfg(test)]
#[path = "../../../patches/midi-sync/l6max/src/parameters.rs"]
mod parameters;

#[cfg(test)]
#[path = "../../../patches/midi-sync/common/sync_request.rs"]
mod sync_request;
