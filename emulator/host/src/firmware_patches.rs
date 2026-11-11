//! Runs the patches selected for a model. A device boots from its saved MAIN NOR
//! rather than the selected package, so a change of selection also installs the
//! resulting application there, as a firmware update would.
use crate::engine::Options;
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// The SHA-256 of the patched MAIN image installed on a device. A device without
/// this file runs whatever it has installed.
const INSTALLED: &str = "installed-patches.sha256";

fn stock_package(options: &Options, stock: &Path, model: l6fw::Model) -> Result<Vec<u8>, String> {
    let name = match model {
        l6fw::Model::L6 => "L6.bin",
        l6fw::Model::L6max => "L6max.bin",
    };
    let imported = stock.join(name);
    let package = if imported.is_file() {
        &imported
    } else {
        options
            .firmware_package
            .as_ref()
            .ok_or("patching firmware requires an update package, not only an unpacked directory")?
    };
    let bytes = fs::read(package).map_err(|e| format!("{}: {e}", package.display()))?;
    if l6fw::model_from_package(&bytes)? != model {
        return Err(format!(
            "{} belongs to a different model than this device",
            package.display()
        ));
    }
    Ok(bytes)
}

/// The patches that apply to the selected firmware.
pub fn available(options: &Options, model: l6fw::Model) -> Result<Vec<&'static str>, String> {
    let package = stock_package(options, &options.firmware_dir, model)?;
    firmware_patch_wasm::available_patches(&package)
}

/// Check that a selection can apply, before stopping a running guest for it.
pub fn check(
    options: &Options,
    model: l6fw::Model,
    selected: &[String],
    sd_selected: &[String],
) -> Result<(), String> {
    crate::sd_patches::check(model, sd_selected)?;
    if let Some(patch) = selected.iter().find(|patch| sd_selected.contains(patch)) {
        return Err(format!(
            "{patch} is selected both in the firmware and on the SD card"
        ));
    }
    if !sd_selected.is_empty() && options.no_sd {
        return Err("SD card patches need an SD card".into());
    }
    if selected.is_empty() && sd_selected.is_empty() {
        return Ok(());
    }
    let package = stock_package(options, &options.firmware_dir, model)?;
    let names: Vec<&str> = selected.iter().map(String::as_str).collect();
    firmware_patch_wasm::apply_patches_with_loader(&package, &names, !sd_selected.is_empty())
        .map(|_| ())
}

fn read_segments(directory: &Path) -> Result<[Vec<u8>; 3], String> {
    let read =
        |name: &str| fs::read(directory.join(name)).map_err(|e| format!("cannot read {name}: {e}"));
    Ok([
        read("main_firmware.bin")?,
        read("main_trailer.bin")?,
        read("package_footer.bin")?,
    ])
}

fn install(state: &Path, directory: &Path) -> Result<(), String> {
    let [main, trailer, footer] = read_segments(directory)?;
    crate::update_bootloader::install_application(state, &main, &trailer, &footer)
        .map_err(|e| format!("cannot install patched firmware: {e}"))
}

/// Bring a device to the selected patches. Returns the directory to boot from.
/// Patches selected for the SD card install the Patch loader in the firmware
/// and their modules on `card`. Runs while no guest owns the device's storage.
pub fn prepare(
    options: &Options,
    stock: &Path,
    model: l6fw::Model,
    card: Option<&Path>,
) -> Result<PathBuf, String> {
    let selected = options.selected_patches(model);
    let sd_selected = options.selected_sd_patches(model);
    match card {
        Some(card) => {
            crate::sd_patches::install(card, model, &sd_selected)?;
        }
        None if !sd_selected.is_empty() => {
            return Err(format!(
                "SD card patches ({}) need an SD card",
                sd_selected.join(", ")
            ));
        }
        None => {}
    }
    let state = options.state_dir.as_deref();
    let directory = state.unwrap_or(&options.logs).join("patched-firmware");
    if directory.exists() {
        fs::remove_dir_all(&directory).map_err(|e| e.to_string())?;
    }
    let installed = state.map(|state| state.join(INSTALLED));
    if selected.is_empty() && sd_selected.is_empty() {
        if let (Some(state), Some(installed)) = (state, installed.filter(|path| path.is_file())) {
            install(state, stock)?;
            fs::remove_file(installed).map_err(|e| e.to_string())?;
        }
        return Ok(stock.to_path_buf());
    }
    let package = stock_package(options, stock, model)?;
    let names: Vec<&str> = selected.iter().map(String::as_str).collect();
    let loader = !sd_selected.is_empty();
    let (patched, _) = firmware_patch_wasm::apply_patches_with_loader(&package, &names, loader)
        .map_err(|e| {
            let mut all = selected.clone();
            if loader {
                all.push("the SD Patch loader".into());
            }
            format!("cannot apply {}: {e}", all.join(", "))
        })?;
    let parent = directory.parent().unwrap();
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let file = parent.join("patched-firmware.partial");
    fs::write(&file, patched).map_err(|e| e.to_string())?;
    let unpacked = l6fw::unpack(&file, &directory);
    let _ = fs::remove_file(&file);
    unpacked?;
    if let (Some(state), Some(installed)) = (state, installed) {
        let main = fs::read(directory.join("main_firmware.bin")).map_err(|e| e.to_string())?;
        let hash = format!("{:x}", Sha256::digest(&main));
        match fs::read_to_string(&installed) {
            Ok(current) if current == hash => {}
            Ok(_) => install(state, &directory)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => install(state, &directory)?,
            Err(error) => return Err(error.to_string()),
        }
        fs::write(installed, hash).map_err(|e| e.to_string())?;
    }
    Ok(directory)
}

/// An update replaced the application, so what was installed no longer applies.
pub fn forget_installed(state: &Path) -> Result<(), String> {
    match fs::remove_file(state.join(INSTALLED)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error.to_string()),
        _ => Ok(()),
    }
}
