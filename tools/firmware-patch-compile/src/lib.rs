//! Native-only: compiles one patch's `payload.rs` to a flat ARM Thumb binary
//! via `rustc --target=thumbv7em-none-eabihf` + `lld` + `llvm-objcopy`. Shared
//! by the `l6patch` CLI and the WASM-template prebuild tool, so both
//! ever compile a payload through exactly this one code path — the whole
//! premise of shipping precompiled templates depends on this being the same
//! compile every time, not two paths that could silently drift apart.
//!
//! Never built for `wasm32-unknown-unknown`: it shells out to `rustc`/`lld`/
//! `llvm-objcopy`, which don't exist in a browser.
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

/// Compiles one patch's payload at `base`, against the firmware found as
/// `main_firmware.bin` under `source` (via `trampolines.s`'s `.incbin`, read
/// relative to `source`). Returns the flat binary and its `llvm-nm` symbol
/// table (address + name per line).
pub fn compile(
    source: &Path,
    root: &Path,
    output: &Path,
    name: &str,
    base: u32,
) -> Result<(Vec<u8>, String), Box<dyn std::error::Error>> {
    let elf = output.join(format!("{name}.payload.elf"));
    let blob = output.join(format!("{name}.payload.bin"));
    run(Command::new("rustc")
        .current_dir(source)
        .arg(root.join("src/payload.rs"))
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
        .arg(&elf))?;
    let symbols = run(Command::new(llvm_tool("L6_LLVM_NM", HOMEBREW_NM, "llvm-nm")?).arg(&elf))?;
    run(Command::new(llvm_tool(
        "L6_LLVM_OBJCOPY",
        HOMEBREW_OBJCOPY,
        "llvm-objcopy",
    )?)
    .args(["-O", "binary"])
    .arg(&elf)
    .arg(&blob))?;
    Ok((fs::read(blob)?, symbols))
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
