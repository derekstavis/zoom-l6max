//! Native-only: compiles one patch's `payload.rs` to a flat ARM Thumb binary
//! via `rustc --target=thumbv7em-none-eabihf` + `lld` + `llvm-objcopy`. Shared
//! by the `l6patch` CLI and the WASM-template prebuild tool, so both
//! ever compile a payload through exactly this one code path — the whole
//! premise of shipping precompiled templates depends on this being the same
//! compile every time, not two paths that could silently drift apart.
//!
//! Never built for `wasm32-unknown-unknown`: it shells out to `rustc`/`lld`/
//! `llvm-objcopy`, which don't exist in a browser.
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn run(command: &mut Command) -> Result<String, Box<dyn std::error::Error>> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

const HOMEBREW_LLD: &str = "/opt/homebrew/bin/ld.lld";
const HOMEBREW_NM: &str = "/opt/homebrew/opt/llvm/bin/llvm-nm";
const HOMEBREW_OBJCOPY: &str = "/opt/homebrew/opt/llvm/bin/llvm-objcopy";
const HOMEBREW_READELF: &str = "/opt/homebrew/opt/llvm/bin/llvm-readelf";

/// Load-time fixups for addresses that refer back into a linked patch image.
/// PC-relative calls and references need no fixup when the complete image moves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Relocation {
    Abs32 { offset: u32 },
    ThumbMov32 { low_offset: u32, high_offset: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionHookSiteMetadata {
    pub address: u32,
    pub displaced_len: u16,
    pub continuation: u8,
    pub dynamic_dispatch: bool,
    pub name: String,
    pub guard_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookRegistration {
    Entry {
        site: String,
        handler: String,
    },
    Offset {
        function: String,
        offset: u32,
        displaced_len: u16,
        guard_sha256: String,
        handler: String,
    },
    CallPre {
        function: String,
        address: u32,
        argument_count: usize,
        handler: String,
    },
    CallPost {
        function: String,
        address: u32,
        argument_count: usize,
        handler: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookWindow {
    pub address: u32,
    pub target_symbol: String,
    pub name: String,
    pub length: usize,
    pub continuation: u8,
}

/// The active toolchain's `bin` directory, which holds `rust-lld` and, with the
/// `llvm-tools` component (listed in `rust-toolchain.toml`), `llvm-nm` and
/// `llvm-objcopy`.
fn toolchain_bin() -> Option<PathBuf> {
    let sysroot = run(Command::new("rustc").args(["--print", "sysroot"])).ok()?;
    fs::read_dir(PathBuf::from(sysroot.trim()).join("lib/rustlib"))
        .ok()?
        .flatten()
        .map(|host| host.path().join("bin"))
        .find(|bin| bin.join("rust-lld").is_file())
}

/// An explicit `variable` override, then a Homebrew LLVM install, then the Rust
/// toolchain's own copy.
fn llvm_tool(
    variable: &str,
    homebrew: &str,
    bundled: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(path) = std::env::var_os(variable) {
        return Ok(PathBuf::from(path));
    }
    if Path::new(homebrew).exists() {
        return Ok(PathBuf::from(homebrew));
    }
    toolchain_bin()
        .map(|bin| bin.join(bundled))
        .filter(|path| path.is_file())
        .ok_or_else(|| {
            format!("{bundled} not found; run `rustup component add llvm-tools` or set {variable}")
                .into()
        })
}

/// Compiles one patch's payload at `base` against `main_firmware.bin` under
/// `source`. Continuations and hook veneers are generated from hook attributes
/// and the shared function catalog. Returns the flat binary and symbols.
pub fn compile(
    source: &Path,
    root: &Path,
    output: &Path,
    name: &str,
    base: u32,
) -> Result<(Vec<u8>, String), Box<dyn std::error::Error>> {
    compile_inner(source, root, output, name, base, None, false, false, true)
}

/// Compile a deterministic sentinel-firmware variant for browser template
/// window discovery. Guard validation is performed against the real image
/// before this variant is built.
pub fn compile_sentinel(
    source: &Path,
    root: &Path,
    output: &Path,
    name: &str,
    base: u32,
) -> Result<(Vec<u8>, String), Box<dyn std::error::Error>> {
    compile_inner(source, root, output, name, base, None, false, false, false)
}

/// Compile with continuation metadata collected from additional patch roots.
/// Used by the loader, whose resident dispatcher also needs stock continuations
/// for patches it can dispatch dynamically.
pub fn compile_with_continuations(
    source: &Path,
    root: &Path,
    continuation_roots: &[&Path],
    output: &Path,
    name: &str,
    base: u32,
) -> Result<(Vec<u8>, String), Box<dyn std::error::Error>> {
    let _ = continuation_roots;
    compile_inner(source, root, output, name, base, None, false, true, true)
}

/// Compile a deterministic sentinel-firmware variant of a loader template.
/// The loader still needs every dynamic dispatcher and continuation emitted,
/// but the sentinel firmware intentionally differs at guarded hook windows.
pub fn compile_sentinel_with_continuations(
    source: &Path,
    root: &Path,
    continuation_roots: &[&Path],
    output: &Path,
    name: &str,
    base: u32,
) -> Result<(Vec<u8>, String), Box<dyn std::error::Error>> {
    let _ = continuation_roots;
    compile_inner(source, root, output, name, base, None, false, true, false)
}

/// Compile a payload with its optional SD-specific initialization entry.
pub fn compile_sd(
    source: &Path,
    root: &Path,
    output: &Path,
    name: &str,
    base: u32,
) -> Result<(Vec<u8>, String), Box<dyn std::error::Error>> {
    compile_inner(
        source,
        root,
        output,
        name,
        base,
        Some("sd_dynamic"),
        false,
        false,
        true,
    )
}

/// Compile an SD module and retain the small set of absolute relocations whose
/// targets are within the module. The relocation records are extracted from
/// the linked ELF; the flat image remains the same compact static codegen.
pub fn compile_sd_relocatable(
    source: &Path,
    root: &Path,
    output: &Path,
    name: &str,
    base: u32,
) -> Result<(Vec<u8>, String, Vec<Relocation>), Box<dyn std::error::Error>> {
    let (image, symbols) = compile_inner(
        source,
        root,
        output,
        name,
        base,
        Some("sd_dynamic"),
        true,
        false,
        true,
    )?;
    let elf = output.join(format!("{name}.payload.elf"));
    let relocations = parse_relocations(&elf, base, image.len())?;
    Ok((image, symbols, relocations))
}

/// Compile the Patch loader against the separate L6 firmware bindings.
pub fn compile_l6_patch_loader(
    source: &Path,
    root: &Path,
    output: &Path,
    name: &str,
    base: u32,
    continuation_roots: &[&Path],
) -> Result<(Vec<u8>, String), Box<dyn std::error::Error>> {
    let _ = continuation_roots;
    let (image, symbols) = compile_inner(
        source,
        root,
        output,
        name,
        base,
        Some("sd_l6"),
        false,
        true,
        true,
    )?;
    Ok((image, symbols))
}

/// Sentinel variant for deriving L6 browser loader templates.
pub fn compile_sentinel_l6_patch_loader(
    source: &Path,
    root: &Path,
    output: &Path,
    name: &str,
    base: u32,
    continuation_roots: &[&Path],
) -> Result<(Vec<u8>, String), Box<dyn std::error::Error>> {
    let _ = continuation_roots;
    compile_inner(
        source,
        root,
        output,
        name,
        base,
        Some("sd_l6"),
        false,
        true,
        false,
    )
}

fn compile_inner(
    source: &Path,
    root: &Path,
    output: &Path,
    name: &str,
    base: u32,
    cfg: Option<&str>,
    emit_relocations: bool,
    generate_dynamic_sites: bool,
    validate_guards: bool,
) -> Result<(Vec<u8>, String), Box<dyn std::error::Error>> {
    let generated = output.join(format!("{name}.generated"));
    fs::create_dir_all(&generated)?;
    let generated = generated.canonicalize()?;
    let firmware = fs::read(source.join("main_firmware.bin"))?;
    let elf = output.join(format!("{name}.payload.elf"));
    let blob = output.join(format!("{name}.payload.bin"));
    let hook_macro = patch_hook_macro_library()?;
    fs::write(
        generated.join("continuations.rs"),
        "pub fn address(_: &str) -> usize { 0 }\n",
    )?;
    let _ = compile_payload_pass(
        source,
        root,
        name,
        base,
        cfg,
        emit_relocations,
        &generated,
        &elf,
        &blob,
        &hook_macro,
    )?;

    let first_symbols =
        run(Command::new(llvm_tool("L6_LLVM_NM", HOMEBREW_NM, "llvm-nm")?).arg(&elf))?;
    let sites = function_hook_sites(&elf)?;
    let registrations = hook_registrations(&first_symbols)?;
    let continuation_rs = generate_continuations_from_metadata(
        &sites,
        &registrations,
        &firmware,
        emit_relocations,
        generate_dynamic_sites,
        validate_guards,
    )?;
    fs::write(generated.join("continuations.rs"), continuation_rs)?;
    compile_payload_pass(
        source,
        root,
        name,
        base,
        cfg,
        emit_relocations,
        &generated,
        &elf,
        &blob,
        &hook_macro,
    )
}

#[allow(clippy::too_many_arguments)]
fn compile_payload_pass(
    source: &Path,
    root: &Path,
    name: &str,
    base: u32,
    cfg: Option<&str>,
    emit_relocations: bool,
    generated: &Path,
    elf: &Path,
    blob: &Path,
    hook_macro: &Path,
) -> Result<(Vec<u8>, String), Box<dyn std::error::Error>> {
    let firmware = fs::read(source.join("main_firmware.bin"))?;
    let firmware_is_l6 = firmware.get(4..8) == Some(&0x8000_1415u32.to_le_bytes());
    let mut command = Command::new("rustc");
    command
        .current_dir(source)
        .env("OUT_DIR", &generated)
        .arg(root.join("src/payload.rs"))
        .arg("--extern")
        .arg(format!("patch_hook_macros={}", hook_macro.display()))
        // Use a checkout-independent source path in compiler-generated data.
        .arg(format!(
            "--remap-path-prefix={}=patches/{name}",
            root.join("src").display(),
        ))
        .args([
            "--edition=2024",
            "--target=thumbv7em-none-eabihf",
            "-C",
            "opt-level=z",
            "-C",
            "panic=abort",
            "-C",
            "relocation-model=static",
            "-C",
        ])
        .arg(format!(
            "linker={}",
            llvm_tool("L6_LD_LLD", HOMEBREW_LLD, "rust-lld")?.display()
        ))
        .arg("-C")
        .arg(format!("link-arg=--defsym=PAYLOAD_BASE={base:#x}"))
        .arg("-C")
        .arg(format!(
            "link-arg=-T{}",
            root.join("linker/link.ld").display()
        ))
        .arg("-o")
        .arg(&elf);
    if emit_relocations {
        command
            .arg("--cfg")
            .arg("sd_dynamic_module")
            .arg("-C")
            .arg("link-arg=--emit-relocs");
    }
    if let Some(cfg) = cfg {
        command.args(["--cfg", cfg]);
    }
    if firmware_is_l6 && cfg != Some("sd_l6") {
        command.args(["--cfg", "sd_l6"]);
    }
    run(&mut command)?;
    let symbols = run(Command::new(llvm_tool("L6_LLVM_NM", HOMEBREW_NM, "llvm-nm")?).arg(&elf))?;
    run(Command::new(llvm_tool(
        "L6_LLVM_OBJCOPY",
        HOMEBREW_OBJCOPY,
        "llvm-objcopy",
    )?)
    .args(["--remove-section=.patch_hooks", "-O", "binary"])
    .arg(&elf)
    .arg(&blob))?;
    Ok((fs::read(blob)?, symbols))
}

fn generate_continuations_from_metadata(
    sites: &[FunctionHookSiteMetadata],
    registrations: &[HookRegistration],
    firmware: &[u8],
    dynamic: bool,
    generate_dynamic_sites: bool,
    validate_guards: bool,
) -> Result<String, Box<dyn std::error::Error>> {
    let main_base = match firmware.get(4..8) {
        Some(bytes) if bytes == 0x8000_0415u32.to_le_bytes() => 0x8000_0000u32,
        Some(bytes) if bytes == 0x8000_1415u32.to_le_bytes() => 0x8000_1000u32,
        _ => return Err("unsupported firmware reset vector for continuation generation".into()),
    };
    let mut needed = std::collections::BTreeSet::new();
    let mut call_groups =
        std::collections::BTreeMap::<String, Vec<(bool, u32, String, usize)>>::new();
    for site in sites.iter().filter(|site| site.dynamic_dispatch) {
        if dynamic || generate_dynamic_sites {
            needed.insert(site.name.as_str());
        }
    }
    for registration in registrations {
        match registration {
            HookRegistration::Entry { site, .. } => {
                needed.insert(site.as_str());
            }
            HookRegistration::CallPre {
                function,
                address,
                argument_count,
                handler,
            } => {
                needed.insert(function.as_str());
                call_groups.entry(function.clone()).or_default().push((
                    false,
                    *address,
                    handler.clone(),
                    *argument_count,
                ));
            }
            HookRegistration::CallPost {
                function,
                address,
                argument_count,
                handler,
            } => {
                needed.insert(function.as_str());
                call_groups.entry(function.clone()).or_default().push((
                    true,
                    *address,
                    handler.clone(),
                    *argument_count,
                ));
            }
            HookRegistration::Offset { .. } => {}
        }
    }

    let mut assembly = String::from(".thumb\n");
    let mut address_arms = Vec::new();
    let mut externs = Vec::new();
    let mut wrappers = Vec::new();
    for name in needed {
        let site = sites
            .iter()
            .find(|site| site.name == name)
            .ok_or_else(|| format!("hook site {name} is missing from define_function_hooks!"))?;
        let offset = usize::try_from(
            site.address
                .checked_sub(main_base)
                .ok_or("hook before firmware")?,
        )?;
        let end = offset
            .checked_add(usize::from(site.displaced_len))
            .ok_or("hook length overflow")?;
        let displaced = firmware
            .get(offset..end)
            .ok_or_else(|| format!("continuation bytes outside firmware at {:#x}", site.address))?;
        let actual_guard = format!("{:x}", Sha256::digest(displaced));
        if validate_guards && actual_guard != site.guard_sha256 {
            return Err(format!(
                "function hook guard failed: {name} at {:#x} (base {main_base:#x}, offset {offset:#x}, bytes {:02x?}): expected {}, got {}",
                site.address, displaced, site.guard_sha256, actual_guard
            )
            .into());
        }

        let original = format!("original_{name}");
        externs.push(format!("unsafe extern \"C\" {{ fn {original}(); }}"));
        if dynamic && site.dynamic_dispatch {
            let slot = format!("dynamic_next_{name}_slot");
            assembly.push_str(&format!(
                ".section .text.continuation_{name}, \"ax\"\n.global {original}\n.thumb_func\n{original}:\n    ldr r12, ={slot}\n    ldr r12, [r12]\n    bx r12\n.align 2\n.global {slot}\n{slot}:\n    .word 0\n"
            ));
        } else {
            assembly.push_str(&format!(
                ".section .text.continuation_{name}, \"ax\"\n.global {original}\n.thumb_func\n{original}:\n"
            ));
            let resume = site
                .address
                .checked_add(u32::from(site.displaced_len))
                .ok_or("continuation address overflow")?
                | 1;
            match site.continuation {
                0 => {
                    emit_bytes(&mut assembly, displaced);
                    assembly.push_str(&format!("    ldr pc, ={resume:#x}\n.ltorg\n"));
                }
                1 => emit_cbz_continuation(&mut assembly, site.address, displaced, resume)?,
                value => return Err(format!("unsupported continuation strategy {value}").into()),
            }
        }
        address_arms.push(format!(
            "{} => {original} as *const () as usize,",
            format!("\"{name}\"")
        ));
    }

    // The resident loader patches every catalog entry marked for dynamic
    // dispatch to one of these ABI-transparent tail-call stubs. The handler
    // pointer table lives beside the loader's fixed hook-chain registry.
    // Each module handler is a normal typed Rust hook and owns its continuation
    // slot; the resident stub only selects the current chain head or stock
    // continuation without disturbing any arguments.
    if generate_dynamic_sites && !dynamic {
        let registry_base = if main_base == 0x8000_1000 {
            0x801f_8000u32
        } else {
            0x8021_8000u32
        };
        let handler_table = registry_base + 0x100 + 8 * 136;
        let mut dynamic_index = 0usize;
        for site in sites.iter().filter(|site| site.dynamic_dispatch) {
            let dispatcher = format!("dispatch_{}", site.name);
            let original = format!("original_{}", site.name);
            validate_asm_symbol(&dispatcher)?;
            let slot_address = handler_table
                .checked_add(u32::try_from(
                    dynamic_index
                        .checked_mul(4)
                        .ok_or("handler slot overflow")?,
                )?)
                .ok_or("handler table address overflow")?;
            assembly.push_str(&format!(
                ".section .text.{dispatcher}, \"ax\"\n.global {dispatcher}\n.thumb_func\n{dispatcher}:\n    push {{r0}}\n    ldr r0, ={registry_base:#x}\n    ldr r0, [r0]\n    ldr r12, =0x4c365244\n    cmp r0, r12\n    pop {{r0}}\n    bne 1f\n    ldr r12, ={slot_address:#x}\n    ldr r12, [r12]\n    cmp r12, #0\n    bne 2f\n1:  ldr r12, ={original}\n2:  bx r12\n.ltorg\n"
            ));
            wrappers.push(format!(
                "unsafe extern \"C\" {{ fn {dispatcher}(); }}\n#[used]\nstatic __L6_KEEP_DYNAMIC_DISPATCH_{}: unsafe extern \"C\" fn() = {dispatcher};",
                site.name
            ));
            dynamic_index += 1;
        }
    }

    let mut offset_wrappers = Vec::new();
    for registration in registrations {
        let HookRegistration::Offset {
            function,
            offset: instruction_offset,
            displaced_len,
            guard_sha256,
            handler,
        } = registration
        else {
            continue;
        };
        let site = sites
            .iter()
            .find(|site| site.name == function.as_str())
            .ok_or_else(|| {
                format!("offset hook function {function} is missing from define_function_hooks!")
            })?;
        let address = site
            .address
            .checked_add(*instruction_offset)
            .ok_or("offset hook address overflow")?;
        let start = usize::try_from(
            address
                .checked_sub(main_base)
                .ok_or("offset hook before firmware")?,
        )?;
        let len = usize::from(*displaced_len);
        let displaced = firmware
            .get(
                start
                    ..start
                        .checked_add(len)
                        .ok_or("offset hook length overflow")?,
            )
            .ok_or_else(|| format!("offset hook bytes outside firmware at {address:#x}"))?;
        let actual_guard = format!("{:x}", Sha256::digest(displaced));
        if validate_guards && actual_guard != *guard_sha256 {
            return Err(format!("offset hook guard failed: {function} at {address:#x}").into());
        }
        let veneer = format!("__l6_offset_veneer_{handler}");
        validate_asm_symbol(handler)?;
        validate_asm_symbol(&veneer)?;
        assembly.push_str(&format!(
            ".section .text.generated_hook_{handler}, \"ax\"\n.global {veneer}\n.thumb_func\n{veneer}:\n    push {{r0-r3, r4-r12, lr}}\n    bl {handler}\n    cmp r0, #1\n    beq 1f\n    pop {{r0-r3, r4-r12, lr}}\n"
        ));
        emit_bytes(&mut assembly, displaced);
        let resume = address
            .checked_add(u32::try_from(len)?)
            .ok_or("offset hook continuation address overflow")?
            | 1;
        assembly.push_str(&format!(
            "    ldr pc, ={resume:#x}\n1:\n    pop {{r0-r3, r4-r12, lr}}\n    ldr pc, ={resume:#x}\n.ltorg\n"
        ));
        offset_wrappers.push(format!(
            "unsafe extern \"C\" {{ fn {veneer}(); }}\n#[used]\nstatic __L6_KEEP_OFFSET_{handler}: unsafe extern \"C\" fn() = {veneer};"
        ));
    }

    if !dynamic {
        for (function, handlers) in call_groups {
            let site = sites
                .iter()
                .find(|site| site.name == function)
                .ok_or_else(|| {
                    format!("call hook function {function} is missing from define_function_hooks!")
                })?;
            for (_, call_address, _, _) in &handlers {
                let actual = decode_thumb_bl_target(firmware, main_base, *call_address)?;
                if actual != site.address {
                    return Err(format!(
                        "call hook at {call_address:#x} targets {actual:#x}, expected {} at {:#x}",
                        function, site.address
                    )
                    .into());
                }
            }
            let argument_count = handlers[0].3;
            if handlers.iter().any(|handler| handler.3 != argument_count) {
                return Err(format!(
                    "call hook handlers for {function} disagree on argument count"
                )
                .into());
            }
            let wrapper = format!("__l6_call_dispatch_{function}");
            let original = format!("original_{function}");
            let matched_label = format!(".Lcall_hook_match_{function}");
            validate_asm_symbol(&wrapper)?;
            assembly.push_str(&format!(
                ".section .text.call_dispatch_{function}, \"ax\"\n.global {wrapper}\n.thumb_func\n{wrapper}:\n"
            ));
            let mut call_returns = std::collections::BTreeSet::new();
            for (_, call_address, _, _) in &handlers {
                call_returns.insert(
                    call_address
                        .checked_add(4)
                        .ok_or("call return address overflow")?
                        | 1,
                );
            }
            for caller_return in call_returns {
                assembly.push_str(&format!(
                    "    ldr r12, ={caller_return:#x}\n    cmp lr, r12\n    beq {matched_label}\n"
                ));
            }
            assembly.push_str(&format!(
                "    ldr pc, ={original}\n{matched_label}:\n    push {{r0-r4, lr}}\n    sub sp, #8\n    mov r4, sp\n"
            ));
            for (_, call_address, handler, _) in handlers.iter().filter(|(post, _, _, _)| !post) {
                let caller_return = call_address
                    .checked_add(4)
                    .ok_or("call return address overflow")?
                    | 1;
                emit_call_hook_dispatch(&mut assembly, caller_return, handler, argument_count)?;
            }
            emit_call_arguments(&mut assembly, argument_count)?;
            assembly.push_str(&format!(
                "    bl {original}\n    mov sp, r4\n    str r0, [r4, #0]\n    str r1, [r4, #4]\n"
            ));
            // Call wrappers unwind after the callee. Reverse the direct static
            // calls so their observable order matches filename-ordered dynamic
            // wrapper chains.
            for (_, call_address, handler, _) in
                handlers.iter().rev().filter(|(post, _, _, _)| *post)
            {
                let caller_return = call_address
                    .checked_add(4)
                    .ok_or("call return address overflow")?
                    | 1;
                emit_call_hook_dispatch(&mut assembly, caller_return, handler, argument_count)?;
            }
            assembly.push_str(
                "    mov sp, r4\n    add sp, #8\n    pop {r0-r4, lr}\n    sub r12, sp, #32\n    ldr r0, [r12, #0]\n    ldr r1, [r12, #4]\n    bx lr\n.ltorg\n",
            );
            wrappers.push(format!(
                "unsafe extern \"C\" {{ fn {wrapper}(); }}\n#[used]\nstatic __L6_KEEP_CALL_DISPATCH_{function}: unsafe extern \"C\" fn() = {wrapper};"
            ));
        }
    }

    if !assembly.trim().eq(".thumb") {
        Ok(format!(
            "use core::arch::global_asm;\nglobal_asm!(r####\"{assembly}\"####, options(raw));\n{}\npub fn address(name: &str) -> usize {{ match name {{ {} _ => 0 }} }}\n{}\n{}\n",
            externs.join("\n"),
            address_arms.join(" "),
            offset_wrappers.join("\n"),
            wrappers.join("\n"),
        ))
    } else {
        Ok("pub fn address(_: &str) -> usize { 0 }\n".to_owned())
    }
}

pub fn decode_thumb_bl_target(
    firmware: &[u8],
    firmware_base: u32,
    address: u32,
) -> Result<u32, Box<dyn std::error::Error>> {
    let offset = usize::try_from(
        address
            .checked_sub(firmware_base)
            .ok_or("call site before firmware")?,
    )?;
    let bytes = firmware
        .get(
            offset
                ..offset
                    .checked_add(4)
                    .ok_or("call instruction offset overflow")?,
        )
        .ok_or_else(|| format!("call instruction outside firmware at {address:#x}"))?;
    let first = u16::from_le_bytes(bytes[..2].try_into()?);
    let second = u16::from_le_bytes(bytes[2..].try_into()?);
    if first & 0xf800 != 0xf000 || second & 0xd000 != 0xd000 {
        return Err(format!("expected Thumb BL at {address:#x}").into());
    }
    let sign = ((first >> 10) & 1) as u32;
    let j1 = ((second >> 13) & 1) as u32;
    let j2 = ((second >> 11) & 1) as u32;
    let i1 = (!(j1 ^ sign)) & 1;
    let i2 = (!(j2 ^ sign)) & 1;
    let encoded = (sign << 24)
        | (i1 << 23)
        | (i2 << 22)
        | (((first & 0x03ff) as u32) << 12)
        | (((second & 0x07ff) as u32) << 1);
    let signed_offset = ((encoded << 7) as i32) >> 7;
    Ok(address.wrapping_add(4).wrapping_add(signed_offset as u32) & !1)
}

fn emit_call_hook_dispatch(
    assembly: &mut String,
    caller_return: u32,
    handler: &str,
    argument_count: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    validate_asm_symbol(handler)?;
    assembly.push_str(&format!(
        "    ldr r0, [r4, #28]\n    ldr r12, ={caller_return:#x}\n    cmp r0, r12\n    bne 1f\n"
    ));
    emit_call_arguments(assembly, argument_count)?;
    assembly.push_str(&format!("    bl {handler}\n    mov sp, r4\n1:\n"));
    Ok(())
}

fn emit_call_arguments(
    assembly: &mut String,
    argument_count: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    assembly.push_str(
        "    ldr r0, [r4, #8]\n    ldr r1, [r4, #12]\n    ldr r2, [r4, #16]\n    ldr r3, [r4, #20]\n",
    );
    let stack_arguments = argument_count.saturating_sub(4);
    let outgoing_bytes = stack_arguments
        .checked_mul(4)
        .ok_or("too many stack arguments")?
        .checked_add(7)
        .ok_or("stack argument area overflow")?
        & !7;
    if outgoing_bytes == 0 {
        assembly.push_str("    mov sp, r4\n");
    } else {
        if outgoing_bytes > 508 {
            return Err("call hook has too many stack arguments".into());
        }
        assembly.push_str(&format!("    mov sp, r4\n    sub sp, #{outgoing_bytes}\n"));
        for index in 0..stack_arguments {
            let source_offset = 32usize
                .checked_add(
                    index
                        .checked_mul(4)
                        .ok_or("stack argument offset overflow")?,
                )
                .ok_or("stack argument offset overflow")?;
            let target_offset = index * 4;
            assembly.push_str(&format!(
                "    ldr r12, [r4, #{source_offset}]\n    str r12, [sp, #{target_offset}]\n"
            ));
        }
    }
    Ok(())
}

fn patch_hook_macro_library() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let executable = std::env::current_exe()?;
    let executable_dir = executable
        .parent()
        .ok_or("compiler executable has no parent directory")?;
    let profile_dir = if executable_dir
        .file_name()
        .is_some_and(|name| name == "deps")
    {
        executable_dir
            .parent()
            .ok_or("dependency executable has no profile directory")?
    } else {
        executable_dir
    };
    let expected_extension = std::env::consts::DLL_SUFFIX.trim_start_matches('.');
    let mut pending = vec![profile_dir.join("deps"), profile_dir.join("build")];
    let mut candidates = Vec::new();
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    (name.starts_with("libpatch_hook_macros-")
                        || name.starts_with("patch_hook_macros-"))
                        && path
                            .extension()
                            .is_some_and(|extension| extension == expected_extension)
                })
            {
                candidates.push(path);
            }
        }
    }
    candidates.sort_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
    });
    candidates.pop().ok_or_else(|| {
        format!(
            "patch-hook-macros artifact not found under {}",
            profile_dir.display()
        )
        .into()
    })
}

fn validate_asm_symbol(symbol: &str) -> Result<(), Box<dyn std::error::Error>> {
    if symbol.is_empty()
        || !symbol
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        || symbol.as_bytes().first().is_some_and(u8::is_ascii_digit)
    {
        return Err(format!("invalid generated assembly symbol {symbol:?}").into());
    }
    Ok(())
}

fn emit_bytes(assembly: &mut String, bytes: &[u8]) {
    for chunk in bytes.chunks(8) {
        assembly.push_str("    .byte ");
        for (index, byte) in chunk.iter().enumerate() {
            if index != 0 {
                assembly.push_str(", ");
            }
            assembly.push_str(&format!("0x{byte:02x}"));
        }
        assembly.push('\n');
    }
}

fn emit_cbz_continuation(
    assembly: &mut String,
    address: u32,
    displaced: &[u8],
    resume: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    if displaced.len() < 2 || displaced.len() % 2 != 0 {
        return Err("CBZ continuation requires an even displaced instruction span".into());
    }
    let instruction = u16::from_le_bytes(displaced[..2].try_into()?);
    if instruction & 0xf500 != 0xb100 {
        return Err(format!("expected CBZ/CBNZ at {address:#x}").into());
    }
    let branch_offset =
        (((instruction & 0x0200) as u32) >> 3) | (((instruction & 0x00f8) as u32) >> 2);
    let target = address
        .checked_add(4)
        .and_then(|pc| pc.checked_add(branch_offset))
        .ok_or("CBZ target overflow")?;
    if (address..address + displaced.len() as u32).contains(&target) {
        return Err(format!("CBZ target lands inside displaced bytes at {target:#x}").into());
    }
    // CBZ's architectural PC is instruction address + 4. The local label
    // follows the replayed span and the 32-bit literal load that resumes it.
    let local_offset = u32::try_from(displaced.len())?;
    if local_offset > 0x7e {
        return Err("displaced CBZ span is too long for generated local branch".into());
    }
    let relocated = (instruction & !0x02f8)
        | (((local_offset & 0x40) << 3) as u16)
        | (((local_offset & 0x3e) << 2) as u16);
    assembly.push_str(&format!("    .hword 0x{relocated:04x}\n"));
    emit_bytes(assembly, &displaced[2..]);
    assembly.push_str(&format!(
        "    ldr pc, ={resume:#x}\n1: ldr pc, ={:#x}\n.ltorg\n",
        target | 1
    ));
    Ok(())
}

fn parse_relocations(
    elf: &Path,
    base: u32,
    image_len: usize,
) -> Result<Vec<Relocation>, Box<dyn std::error::Error>> {
    let output = run(Command::new(llvm_tool(
        "L6_LLVM_READELF",
        HOMEBREW_READELF,
        "llvm-readelf",
    )?)
    .args(["-r"])
    .arg(elf))?;
    let end = base
        .checked_add(u32::try_from(image_len).map_err(|_| "payload too large")?)
        .ok_or("payload address overflow")?;
    let mut in_text = false;
    let mut records = Vec::new();
    let mut movw = Vec::<(u32, u32, String)>::new();
    let mut movt = Vec::<(u32, u32, String)>::new();

    for line in output.lines() {
        if line.starts_with("Relocation section ") {
            in_text = line.contains(".rel.text'") || line.contains(".rela.text'");
            continue;
        }
        if !in_text {
            continue;
        }
        let columns = line.split_whitespace().collect::<Vec<_>>();
        if columns.len() < 5 || !columns[2].starts_with("R_ARM_") {
            continue;
        }
        let offset = u32::from_str_radix(columns[0], 16)?;
        let kind = columns[2];
        let symbol_value = u32::from_str_radix(columns[3], 16)?;
        if !(base..end).contains(&symbol_value) {
            continue;
        }
        let relative = offset
            .checked_sub(base)
            .ok_or("relocation before payload")?;
        let symbol = columns[4..].join(" ");
        match kind {
            "R_ARM_ABS32" => {
                if (relative as usize)
                    .checked_add(4)
                    .is_none_or(|x| x > image_len)
                {
                    return Err("ABS32 relocation outside payload".into());
                }
                records.push(Relocation::Abs32 { offset: relative });
            }
            "R_ARM_THM_MOVW_ABS_NC" => movw.push((relative, symbol_value, symbol)),
            "R_ARM_THM_MOVT_ABS" => movt.push((relative, symbol_value, symbol)),
            // These relocations encode PC-relative displacements, which do not
            // change when the whole module is moved by one load delta.
            "R_ARM_REL32" | "R_ARM_THM_CALL" | "R_ARM_THM_JUMP24" | "R_ARM_THM_JUMP19"
            | "R_ARM_THM_PC8" | "R_ARM_THM_PC12" | "R_ARM_THM_PC22" => {}
            _ => return Err(format!("unsupported internal relocation {kind}").into()),
        }
    }

    for (low_offset, low_value, low_symbol) in movw {
        let Some((index, (high_offset, _, _))) = movt
            .iter()
            .enumerate()
            .filter(|(_, (_, value, symbol))| *value == low_value && *symbol == low_symbol)
            .min_by_key(|(_, (offset, _, _))| offset.abs_diff(low_offset))
        else {
            return Err(format!("unpaired MOVW relocation for {low_symbol}").into());
        };
        if high_offset.abs_diff(low_offset) > 16 {
            return Err(format!("distant MOVW/MOVT relocations for {low_symbol}").into());
        }
        if (*high_offset as usize)
            .checked_add(4)
            .is_none_or(|x| x > image_len)
        {
            return Err("MOVT relocation outside payload".into());
        }
        records.push(Relocation::ThumbMov32 {
            low_offset,
            high_offset: *high_offset,
        });
        movt.remove(index);
    }
    if !movt.is_empty() {
        return Err("unpaired MOVT relocation".into());
    }
    records.sort_by_key(|record| match record {
        Relocation::Abs32 { offset } => *offset,
        Relocation::ThumbMov32 { low_offset, .. } => *low_offset,
    });
    Ok(records)
}

/// Looks up a symbol's address in `compile()`'s `llvm-nm` output, clearing the
/// Thumb bit (bit 0, set on every Thumb function symbol).
pub fn symbol(symbols: &str, name: &str) -> Result<u32, Box<dyn std::error::Error>> {
    let line = symbols
        .lines()
        .find(|line| line.split_whitespace().last() == Some(name))
        .ok_or_else(|| format!("missing patch entry: {name}"))?;
    Ok(u32::from_str_radix(line.split_whitespace().next().unwrap(), 16)? & !1)
}

/// Read catalog records emitted in the ELF's non-runtime `.patch_hooks`
/// section. The section is removed from the flat payload binary.
pub fn function_hook_sites(
    elf: &Path,
) -> Result<Vec<FunctionHookSiteMetadata>, Box<dyn std::error::Error>> {
    let readelf = llvm_tool("L6_LLVM_READELF", HOMEBREW_READELF, "llvm-readelf")?;
    let sections = run(Command::new(readelf).args(["-S"]).arg(elf))?;
    if !sections.contains(".patch_hooks") {
        return Ok(Vec::new());
    }

    let objcopy = llvm_tool("L6_LLVM_OBJCOPY", HOMEBREW_OBJCOPY, "llvm-objcopy")?;
    let metadata_path = elf.with_extension("patch-hooks.tmp");
    let section_arg = format!("--dump-section=.patch_hooks={}", metadata_path.display());
    run(Command::new(objcopy).arg(section_arg).arg(elf))?;
    let bytes = fs::read(&metadata_path)?;
    fs::remove_file(metadata_path)?;

    const RECORD_LEN: usize = 124;
    const MAGIC: u32 = 0x484f_4f4b;
    let mut records = Vec::new();
    let mut offset = 0;
    while offset + RECORD_LEN <= bytes.len() {
        if u32::from_le_bytes(bytes[offset..offset + 4].try_into()?) != MAGIC {
            offset += 4;
            continue;
        }
        let address = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into()?);
        let displaced_len = u16::from_le_bytes(bytes[offset + 8..offset + 10].try_into()?);
        let continuation = bytes[offset + 10];
        let dynamic_dispatch = bytes[offset + 11];
        let name_bytes = &bytes[offset + 12..offset + 60];
        let guard_bytes = &bytes[offset + 60..offset + 124];
        let name_end = name_bytes.iter().position(|byte| *byte == 0).unwrap_or(48);
        let name = std::str::from_utf8(&name_bytes[..name_end])?.to_owned();
        let guard_sha256 = std::str::from_utf8(guard_bytes)?.to_owned();
        if address & 1 != 0
            || displaced_len < 2
            || !matches!(continuation, 0 | 1)
            || !matches!(dynamic_dispatch, 0 | 1)
            || name.is_empty()
            || guard_sha256.len() != 64
            || !guard_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(
                format!("invalid function hook metadata at section offset {offset:#x}").into(),
            );
        }
        records.push(FunctionHookSiteMetadata {
            address,
            displaced_len,
            continuation,
            dynamic_dispatch: dynamic_dispatch != 0,
            name,
            guard_sha256,
        });
        offset += RECORD_LEN;
    }
    Ok(records)
}

/// Decode hook-attribute registration symbols from `llvm-nm` output.
pub fn hook_registrations(
    symbols: &str,
) -> Result<Vec<HookRegistration>, Box<dyn std::error::Error>> {
    let mut registrations = Vec::new();
    for symbol in symbols
        .lines()
        .filter_map(|line| line.split_whitespace().last())
    {
        let Some(metadata) = symbol.strip_prefix("__l6_patch_hook_") else {
            continue;
        };
        let fields = metadata.split("__").collect::<Vec<_>>();
        let invalid = || format!("invalid hook metadata symbol {symbol}");
        match fields.as_slice() {
            ["entry", site, handler] if !site.is_empty() && !handler.is_empty() => {
                registrations.push(HookRegistration::Entry {
                    site: (*site).to_owned(),
                    handler: (*handler).to_owned(),
                });
            }
            [
                "offset",
                function,
                offset,
                displaced_len,
                guard_sha256,
                handler,
            ] if !function.is_empty() && !handler.is_empty() => {
                let displaced_len = displaced_len.parse::<u16>().map_err(|_| invalid())?;
                if displaced_len < 4
                    || displaced_len & 1 != 0
                    || guard_sha256.len() != 64
                    || !guard_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    return Err(invalid().into());
                }
                registrations.push(HookRegistration::Offset {
                    function: (*function).to_owned(),
                    offset: u32::from_str_radix(offset, 16).map_err(|_| invalid())?,
                    displaced_len,
                    guard_sha256: (*guard_sha256).to_owned(),
                    handler: (*handler).to_owned(),
                });
            }
            ["pre", function, address, argument_count, handler]
            | ["post", function, address, argument_count, handler]
                if !function.is_empty() && !handler.is_empty() =>
            {
                let address = u32::from_str_radix(address, 16).map_err(|_| invalid())?;
                let argument_count = argument_count.parse::<usize>().map_err(|_| invalid())?;
                let registration = if fields[0] == "pre" {
                    HookRegistration::CallPre {
                        function: (*function).to_owned(),
                        address,
                        argument_count,
                        handler: (*handler).to_owned(),
                    }
                } else {
                    HookRegistration::CallPost {
                        function: (*function).to_owned(),
                        address,
                        argument_count,
                        handler: (*handler).to_owned(),
                    }
                };
                registrations.push(registration);
            }
            _ => return Err(invalid().into()),
        }
    }
    Ok(registrations)
}

/// Resolve static hook sites from emitted attributes and the shared catalog.
/// Call-site hooks produce a function-entry dispatcher window; offset hooks
/// produce a veneer at the declared instruction offset.
pub fn hook_windows(
    sites: &[FunctionHookSiteMetadata],
    registrations: &[HookRegistration],
) -> Result<Vec<HookWindow>, Box<dyn std::error::Error>> {
    let mut windows = Vec::new();
    let mut call_dispatches = std::collections::BTreeSet::new();
    for registration in registrations {
        match registration {
            HookRegistration::Entry { site, handler } => {
                let site = sites
                    .iter()
                    .find(|candidate| candidate.name == *site)
                    .ok_or_else(|| {
                        format!("entry hook {site} is absent from define_function_hooks!")
                    })?;
                windows.push(HookWindow {
                    address: site.address,
                    target_symbol: handler.clone(),
                    name: handler.clone(),
                    length: usize::from(site.displaced_len),
                    continuation: site.continuation,
                });
            }
            HookRegistration::Offset {
                function,
                offset,
                displaced_len,
                handler,
                ..
            } => {
                let site = sites
                    .iter()
                    .find(|candidate| candidate.name == *function)
                    .ok_or_else(|| {
                        format!("offset hook {function} is absent from define_function_hooks!")
                    })?;
                windows.push(HookWindow {
                    address: site
                        .address
                        .checked_add(*offset)
                        .ok_or("offset hook address overflow")?,
                    target_symbol: format!("__l6_offset_veneer_{handler}"),
                    name: handler.clone(),
                    length: usize::from(*displaced_len),
                    continuation: 0,
                });
            }
            HookRegistration::CallPre { function, .. }
            | HookRegistration::CallPost { function, .. } => {
                if !call_dispatches.insert(function.as_str()) {
                    continue;
                }
                let site = sites
                    .iter()
                    .find(|candidate| candidate.name == *function)
                    .ok_or_else(|| {
                        format!(
                            "call hook function {function} is absent from define_function_hooks!"
                        )
                    })?;
                windows.push(HookWindow {
                    address: site.address,
                    target_symbol: format!("__l6_call_dispatch_{function}"),
                    name: format!("call_dispatch:{function}"),
                    length: usize::from(site.displaced_len),
                    continuation: site.continuation,
                });
            }
        }
    }
    Ok(windows)
}
