//! Local-only, like the other `*-smoke` tools: needs the real firmware, which
//! CI doesn't have. Confirms `firmware-patch-wasm`'s in-browser patcher
//! produces byte-identical output to the native `firmware-patch` CLI, for
//! every one of the 7 non-empty patch combinations. Re-run this after
//! regenerating templates with `firmware-patch-wasm-prebuild` (or after any
//! change to `firmware-patch-core`'s splicing logic) before trusting the
//! browser patcher again.
use firmware_patch_core::PATCHES;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, process::Command};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let firmware_dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("Usage: wasm-patch-smoke REAL_UNPACKED_FIRMWARE_DIR")?,
    )
    .canonicalize()?;

    let scratch = std::env::temp_dir().join(format!("wasm-patch-smoke-{}", std::process::id()));
    fs::create_dir_all(&scratch)?;

    let stock_package_path = scratch.join("stock.bin");
    l6fw::repack(&firmware_dir, &stock_package_path, false)?;
    let stock_package = fs::read(&stock_package_path)?;

    for mask in 1u8..8 {
        let selected: Vec<&str> = PATCHES
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .map(|(_, name)| *name)
            .collect();

        let native_out = scratch.join(format!("native-{mask:03b}"));
        let mut command = Command::new("cargo");
        command
            .args(["run", "--quiet", "-p", "firmware-patch", "--"])
            .arg(&firmware_dir)
            .arg(&native_out)
            .args(["--update-version", "0111"]);
        for patch in &selected {
            command.args(["--patch", patch]);
        }
        let status = command.status()?;
        if !status.success() {
            return Err(format!("native build failed for {selected:?}").into());
        }
        let native_package = fs::read(native_out.join("L6max.bin"))?;

        let (wasm_package, report) = firmware_patch_wasm::apply_patches(&stock_package, &selected)
            .map_err(|message| format!("wasm patcher failed for {selected:?}: {message}"))?;

        if wasm_package != native_package {
            return Err(format!(
                "MISMATCH for {selected:?}: wasm output ({} bytes, SHA-256 {:x}) differs from native CLI output ({} bytes, SHA-256 {:x})",
                wasm_package.len(),
                Sha256::digest(&wasm_package),
                native_package.len(),
                Sha256::digest(&native_package),
            )
            .into());
        }
        println!(
            "PASS {selected:?}: {} bytes, SHA-256 {:x} -- {}",
            wasm_package.len(),
            Sha256::digest(&wasm_package),
            report["usage"].as_str().unwrap_or(""),
        );
    }

    fs::remove_dir_all(&scratch)?;
    println!("All 7 combinations match the native CLI byte-for-byte.");
    Ok(())
}
