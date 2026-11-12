//! Reproducible, revision-guarded firmware experiment. Never edits its input.
mod sd_bundle;
use firmware_patch_compile::{
    FunctionHookSiteMetadata, HookRegistration, compile, compile_l6_patch_loader, compile_sd,
    compile_sd_relocatable_composed, compile_with_continuations, compile_with_continuations_cfg,
    decode_thumb_bl_target, function_hook_sites, hook_registrations, symbol,
};
use firmware_patch_core::{
    FirmwareModel, HookEdit, L6_MAIN_BASE, L6_SLOT_END, PATCHES, PatchEdits, apply_patch_with_map,
    branch, first_payload_for, verify_firmware_for, write_image_length_and_version,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
const USAGE: &str = "Usage: l6patch INPUT_UNPACKED_DIR OUTPUT_DIR [--patch NAME]... [--update-version FOUR_DIGITS] [--experimental-patch-bundle | --experimental-patch-loader] [--experimental-qemu-module-arena]\n\
Patch names come from patches/<name>/module.json. Repeat --patch to select multiple patches; dependencies are included automatically. The Patch loader is experimental and unqualified.";

const SD_MODULE_BASE: u32 = 0x8011_0000;
const SD_MODULE_END: u32 = 0x8011_8000;
const L6_SD_MODULE_BASE: u32 = 0x800c_0000;
const L6_SD_MODULE_END: u32 = 0x800c_8000;

struct Options {
    source: PathBuf,
    output: PathBuf,
    patches: Option<Vec<String>>,
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
            "--patch" => {
                requested.push(value);
            }
            "--update-version" => version = Some(value),
            _ => return Err(USAGE.into()),
        }
    }
    let patches = if requested.is_empty() {
        None
    } else {
        Some(resolve_patch_dependencies(&requested)?)
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

#[derive(Default)]
struct PatchManifest {
    requires: Vec<String>,
}

/// Resolve dependencies at build time. SD builds link the closure into one
/// module; firmware builds apply each member to the firmware image.
fn resolve_patch_dependencies(
    requested: &[String],
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let patch_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../patches");
    let mut manifests = BTreeMap::<String, PatchManifest>::new();
    for entry in fs::read_dir(&patch_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path().join("module.json");
        if !path.exists() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
        let name = value["name"]
            .as_str()
            .ok_or("module.json requires a string `name`")?
            .to_owned();
        if !valid_patch_name(&name) {
            return Err(format!("invalid patch name `{name}`").into());
        }
        if entry.file_name().to_string_lossy() != name {
            return Err(format!(
                "patch directory `{}` does not match manifest name `{name}`",
                entry.file_name().to_string_lossy()
            )
            .into());
        }
        let manifest = PatchManifest {
            requires: value["requires"]
                .as_array()
                .ok_or_else(|| format!("patch `{name}` requires `requires` to be an array"))?
                .iter()
                .map(|dependency| {
                    dependency
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| format!("patch `{name}` dependencies must be patch names"))
                })
                .collect::<Result<_, _>>()?,
        };
        if manifests.insert(name.clone(), manifest).is_some() {
            return Err(format!("duplicate patch manifest name `{name}`").into());
        }
    }

    Ok(resolve_dependency_order(requested, &manifests)?)
}

fn valid_patch_name(name: &str) -> bool {
    !name.is_empty()
        && name.as_bytes()[0].is_ascii_lowercase()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn resolve_dependency_order(
    requested: &[String],
    manifests: &BTreeMap<String, PatchManifest>,
) -> Result<Vec<String>, String> {
    fn visit(
        name: &str,
        manifests: &BTreeMap<String, PatchManifest>,
        active: &mut BTreeSet<String>,
        complete: &mut BTreeSet<String>,
        ordered: &mut Vec<String>,
    ) -> Result<(), String> {
        if complete.contains(name) {
            return Ok(());
        }
        if !active.insert(name.to_owned()) {
            return Err(format!("patch dependency cycle at `{name}`"));
        }
        let manifest = manifests
            .get(name)
            .ok_or_else(|| format!("patch `{name}` has no patches/{name}/module.json"))?;
        for dependency in &manifest.requires {
            visit(dependency, manifests, active, complete, ordered)?;
        }
        active.remove(name);
        complete.insert(name.to_owned());
        ordered.push(name.to_owned());
        Ok(())
    }
    let mut active = BTreeSet::new();
    let mut complete = BTreeSet::new();
    let mut ordered = Vec::new();
    for name in requested {
        visit(name, &manifests, &mut active, &mut complete, &mut ordered)?;
    }
    Ok(ordered)
}
#[derive(Debug)]
struct ResolvedHook {
    address: u32,
    symbol: String,
    target: u32,
    length: usize,
    guard_sha256: String,
    continuation_site: Option<String>,
    continuation_handler: Option<String>,
}

struct CompiledPatch {
    name: String,
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
                continuation_handler: None,
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
            continuation_handler: match registration {
                HookRegistration::Entry { handler, .. } => Some(handler.clone()),
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
        l6fw::Model::L6 => vec!["midi-sync".to_owned()],
        l6fw::Model::L6max => vec!["knob-dialog".to_owned()],
    });
    if !options.experimental_patch_loader
        && !options.experimental_sd_bundle
        && patches.iter().any(|name| !PATCHES.contains(&name.as_str()))
    {
        return Err(
            "the selected patch is not supported for built-in firmware installation".into(),
        );
    }
    if model == l6fw::Model::L6 && patches.iter().any(|name| name != "midi-sync") {
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
            let model_root = patch_root.join(if patch_model == FirmwareModel::L6 {
                "l6"
            } else {
                "l6max"
            });
            let patch_root = if model_root.join("src/payload.rs").is_file() {
                model_root
            } else {
                patch_root
            };
            if !patch_root.join("src/payload.rs").is_file() {
                let model_name = if patch_model == FirmwareModel::L6 {
                    "L6"
                } else {
                    "L6max"
                };
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("patch `{name}` has no {model_name} implementation"),
                ));
            }
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

        // Dependencies run before their dependents, followed by the user's
        // requested patch order. That order defines same-site hook chains.
        let module_roots = patches.iter().zip(&roots).collect::<Vec<_>>();
        let root_refs = module_roots
            .iter()
            .map(|(name, root)| (name.as_str(), root.as_path()))
            .collect::<Vec<_>>();
        let bundle_name = "composed-patches";
        let (module, memory_len, symbols, relocations) =
            compile_sd_relocatable_composed(&source, &root_refs, &output, bundle_name, 0)?;
        let sites = function_hook_sites(&output.join(format!("{bundle_name}.payload.elf")))?;
        let registrations = hook_registrations(&symbols)?;
        if registrations.is_empty() {
            return Err("dynamic bundle has no hook registrations".into());
        }
        let firmware_base = match model {
            l6fw::Model::L6 => 0x8000_1000,
            l6fw::Model::L6max => 0x8000_0000,
        };
        let image = fs::read(source.join("main_firmware.bin"))?;
        let mut handlers = Vec::with_capacity(registrations.len());
        let mut pass_through = Vec::with_capacity(registrations.len());
        for registration in &registrations {
            let (site_name, handler_symbol, continuation_handler) = match registration {
                HookRegistration::Entry { site, handler } => {
                    (site.as_str(), handler.clone(), handler.clone())
                }
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
                    let function_site = sites
                        .iter()
                        .find(|site| site.name == *function)
                        .ok_or_else(|| {
                            format!("call hook function {function} is missing from define_function_hooks!")
                        })?;
                    if target != function_site.address {
                        return Err(format!(
                            "dynamic call hook at {address:#x} targets {target:#x}, expected {} at {:#x}",
                            function, function_site.address
                        )
                        .into());
                    }
                    let phase = match registration {
                        HookRegistration::CallPre { .. } => "pre",
                        HookRegistration::CallPost { .. } => "post",
                        _ => unreachable!(),
                    };
                    (
                        function.as_str(),
                        format!("__l6_dynamic_call_{phase}_{handler}"),
                        handler.clone(),
                    )
                }
                HookRegistration::Offset { .. } => {
                    return Err("function-offset hooks are static-only".into());
                }
            };
            let site = sites
                .iter()
                .find(|site| site.name == site_name && site.dynamic_dispatch)
                .ok_or_else(|| format!("dynamic hook {site_name} is not a dynamic catalog site"))?;
            let next_slot = symbol(
                &symbols,
                &format!("dynamic_next_{}_{}_slot", site.name, continuation_handler),
            )?;
            handlers.push((site.address, symbol(&symbols, &handler_symbol)?, next_slot));
            pass_through.push(symbol(
                &symbols,
                &format!("original_{}_{}", site.name, continuation_handler),
            )?);
        }
        if handlers.len() > patch_loader_core::dynamic::MAX_HOOKS_PER_MODULE {
            return Err(format!(
                "composed module has {} hooks; loader limit is {}",
                handlers.len(),
                patch_loader_core::dynamic::MAX_HOOKS_PER_MODULE
            )
            .into());
        }
        let file = sd_bundle::build_dynamic_v1(&module, memory_len, &handlers, &relocations)?;
        let filename = format!("{}.BIN", bundle_name.to_ascii_uppercase());
        fs::write(patch_dir.join(&filename), &file)?;
        let report_patch = serde_json::json!({
            "name": bundle_name,
            "members": patches,
            "file": format!("PATCHES/{filename}"),
            "payload_length": module.len(),
            "memory_length": memory_len,
            "relocation_count": relocations.len(),
            "exports": exported_symbol_offsets(&symbols),
            "writable_data": {
                "offset": symbol(&symbols, "__module_data_start")?,
                "length": symbol(&symbols, "__module_data_end")?
                    .checked_sub(symbol(&symbols, "__module_data_start")?)
                    .ok_or("writable data range underflow")?,
            },
            "pass_through_entry_offsets": pass_through,
            "hooks": handlers.iter().map(|(address, offset, next_offset)| serde_json::json!({"address":format!("{address:#x}"),"entry_offset":offset,"next_slot_offset":next_offset})).collect::<Vec<_>>(),
            "file_length": file.len(),
        });
        let report = serde_json::json!({
            "version": patch_loader_core::dynamic::FORMAT_VERSION,
            "loader_abi": patch_loader_core::dynamic::LOADER_ABI,
            "model": if model == l6fw::Model::L6 { "L6" } else { "L6max" },
            "source_sha256": digest,
            "patches": [report_patch],
        });
        fs::write(
            output.join("patch.json"),
            serde_json::to_vec_pretty(&report)?,
        )?;
        println!("Wrote one composed module under {}", patch_dir.display());
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
        let (payload, symbols) = if model == l6fw::Model::L6 {
            compile_l6_patch_loader(&source, &root, &output, "patch-loader", end, &[])?
        } else {
            if options.experimental_qemu_module_arena {
                compile_with_continuations_cfg(
                    &source,
                    &root,
                    &[],
                    &output,
                    "patch-loader",
                    end,
                    &["sd_qemu_module_arena"],
                )?
            } else {
                compile_with_continuations(&source, &root, &[], &output, "patch-loader", end)?
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
            name: name.clone(),
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
                let slot_name = first
                    .continuation_handler
                    .as_ref()
                    .map(|handler| {
                        format!("static_next_{}_{}_slot", site.as_ref().unwrap(), handler)
                    })
                    .unwrap_or_else(|| format!("static_next_{}_slot", site.as_ref().unwrap()));
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
            let is_last_in_chain =
                hook_owners
                    .get(&address)
                    .is_some_and(|(owner_patch, owner_hook, _)| {
                        *owner_patch == patch_index && *owner_hook == hook_index
                    });
            if is_last_in_chain
                && let (Some(site), Some(dispatch)) = (
                    compiled_patches[patch_index].hooks[hook_index]
                        .continuation_site
                        .clone(),
                    loader_dispatch_targets.get(&address).copied(),
                )
            {
                let handler = compiled_patches[patch_index].hooks[hook_index]
                    .continuation_handler
                    .as_ref();
                let slot_name = handler.map_or_else(
                    || format!("static_next_{site}_slot"),
                    |handler| format!("static_next_{site}_{handler}_slot"),
                );
                let slot = symbol(&compiled_patches[patch_index].symbols, &slot_name)?;
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
        let mut installed_hooks = Vec::new();
        for hook in &hooks {
            if !unique.insert(hook.address) {
                continue;
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
            installed_hooks.push(hook);
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
        for (hook_edit, hook) in edits.hooks.iter().zip(installed_hooks) {
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

fn exported_symbol_offsets(symbols: &str) -> serde_json::Value {
    let mut exports = serde_json::Map::new();
    for line in symbols.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 4 || fields[2].len() != 1 {
            continue;
        }
        let Ok(address) = u32::from_str_radix(fields[0], 16) else {
            continue;
        };
        let Some(kind) = fields[2].chars().next() else {
            continue;
        };
        if kind.is_ascii_uppercase() {
            exports.insert(fields[3..].join(" "), serde_json::json!(address & !1));
        }
    }
    serde_json::Value::Object(exports)
}

// Thumb-branch encoding is tested in `firmware-patch-core`, which owns `branch()`.

#[cfg(test)]
mod manifest_tests {
    use super::*;

    fn manifest(requires: &[&str]) -> PatchManifest {
        PatchManifest {
            requires: requires.iter().map(|name| (*name).to_owned()).collect(),
        }
    }

    #[test]
    fn resolves_dependencies_before_requested_patches_and_deduplicates() {
        let manifests = BTreeMap::from([
            ("dialog".to_owned(), manifest(&[])),
            ("effect".to_owned(), manifest(&["dialog"])),
            ("midi".to_owned(), manifest(&[])),
        ]);
        let requested = ["midi".to_owned(), "effect".to_owned(), "dialog".to_owned()];

        assert_eq!(
            resolve_dependency_order(&requested, &manifests).unwrap(),
            ["midi", "dialog", "effect"]
        );
    }

    #[test]
    fn rejects_dependency_cycles_and_missing_modules() {
        let cycle = BTreeMap::from([
            ("one".to_owned(), manifest(&["two"])),
            ("two".to_owned(), manifest(&["one"])),
        ]);
        assert!(
            resolve_dependency_order(&["one".to_owned()], &cycle)
                .unwrap_err()
                .contains("cycle")
        );

        let missing = BTreeMap::from([("one".to_owned(), manifest(&["absent"]))]);
        assert!(
            resolve_dependency_order(&["one".to_owned()], &missing)
                .unwrap_err()
                .contains("no patches/absent/module.json")
        );
    }

    #[test]
    fn patch_names_are_safe_rust_module_names_after_dash_normalization() {
        assert!(valid_patch_name("midi-control"));
        assert!(valid_patch_name("patch2"));
        assert!(!valid_patch_name("2patch"));
        assert!(!valid_patch_name("patch_name"));
        assert!(!valid_patch_name("../patch"));
    }

    #[test]
    fn exported_symbol_offsets_keep_global_data_and_clear_thumb_bits() {
        let exports = exported_symbol_offsets(
            "00000011 00000008 T hook_entry\n00000020 00000004 B state_word\n00000030 00000004 b local_state\n",
        );

        assert_eq!(exports["hook_entry"], 0x10);
        assert_eq!(exports["state_word"], 0x20);
        assert!(!exports.as_object().unwrap().contains_key("local_state"));
    }
}

#[cfg(test)]
#[path = "../../../patches/knob-dialog/src/values.rs"]
mod values;

#[cfg(test)]
#[path = "../../../patches/midi-sync/l6max/src/parameters.rs"]
mod parameters;

#[cfg(test)]
#[path = "../../../patches/midi-sync/common/sync_request.rs"]
mod sync_request;
