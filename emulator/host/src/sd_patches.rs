//! Patches the Patch loader reads from the SD card. The modules are the ones the
//! web patcher hosts; the `patch-loader-core` tests check them against the loader.
use std::path::Path;

/// The card folder the loader scans.
const FOLDER: &str = "PATCHES";

struct Module {
    patch: &'static str,
    model: l6fw::Model,
    bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/sd_modules.rs"));

fn card_file_name(module: &Module) -> String {
    let package = firmware_patch_core::patch_package(module.patch)
        .expect("embedded SD module must have a package entry");
    format!("{:02}-{}", package.order, package.file_name)
}

/// The patches `model` can load from its SD card, in catalog order.
pub fn available(model: l6fw::Model) -> Vec<&'static str> {
    let model_name = crate::config::model_name(model);
    firmware_patch_core::patch_catalog()
        .iter()
        .filter(|package| {
            package.kind == "patch"
                && package
                    .emulator_sd_models
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(model_name))
        })
        .filter(|package| module(model, &package.name).is_some())
        .map(|package| package.name.as_str())
        .collect()
}

fn module(model: l6fw::Model, patch: &str) -> Option<&'static Module> {
    MODULES
        .iter()
        .find(|module| module.model == model && module.patch == patch)
}

/// Check that each patch in `selected` loads from `model`'s card.
pub fn check(model: l6fw::Model, selected: &[String]) -> Result<(), String> {
    for patch in selected {
        if module(model, patch).is_none() {
            return Err(format!(
                "{patch} cannot load from the {} SD card; it can only be installed in the firmware",
                crate::config::model_name(model)
            ));
        }
    }
    Ok(())
}

/// Bring the card's `PATCHES` folder to `selected`. Only files that are
/// byte-for-byte one of these modules are removed, so a card's own patch files
/// stay. Returns whether the card changed.
pub fn install(card: &Path, model: l6fw::Model, selected: &[String]) -> Result<bool, String> {
    check(model, selected)?;
    let error = |e: std::io::Error| format!("SD card {}: {e}", card.display());
    let mut volume = sd_image::open(card).map_err(error)?;
    let wanted: Vec<&Module> = selected
        .iter()
        .filter_map(|patch| module(model, patch))
        .collect();
    let mut changed = false;
    if volume.exists(FOLDER) {
        for entry in volume.entries(FOLDER).map_err(error)? {
            if entry.directory {
                continue;
            }
            let path = format!("{FOLDER}/{}", entry.name);
            let keep = wanted
                .iter()
                .any(|module| card_file_name(module).eq_ignore_ascii_case(&entry.name));
            if keep
                || !MODULES
                    .iter()
                    .any(|module| module.bytes.len() as u64 == entry.length)
            {
                continue;
            }
            let bytes = volume.read(&path).map_err(error)?;
            if MODULES.iter().any(|module| module.bytes == bytes) {
                volume.remove(&path).map_err(error)?;
                changed = true;
            }
        }
    } else if !wanted.is_empty() {
        volume.create_dir(FOLDER).map_err(error)?;
        changed = true;
    }
    for module in wanted {
        let path = format!("{FOLDER}/{}", card_file_name(module));
        if volume.read(&path).is_ok_and(|bytes| bytes == module.bytes) {
            continue;
        }
        volume.write(&path, module.bytes).map_err(error)?;
        changed = true;
    }
    if changed {
        volume.flush().map_err(error)?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(name: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("l6-sd-patches-{name}-{}.img", std::process::id()));
        let _ = std::fs::remove_file(&path);
        sd_image::create_empty(&path, "TEST").unwrap();
        path
    }

    fn files(card: &Path) -> Vec<String> {
        let mut volume = sd_image::open(card).unwrap();
        if !volume.exists(FOLDER) {
            return Vec::new();
        }
        let mut names: Vec<String> = volume
            .entries(FOLDER)
            .unwrap()
            .into_iter()
            .map(|entry| entry.name.to_ascii_uppercase())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn offers_the_hosted_modules_per_model() {
        assert_eq!(available(l6fw::Model::L6max), ["knob-dialog", "midi-sync"]);
        assert_eq!(available(l6fw::Model::L6), ["midi-sync"]);
        assert!(check(l6fw::Model::L6, &["knob-dialog".into()]).is_err());
        assert!(check(l6fw::Model::L6max, &["din-mixer-control".into()]).is_err());
    }

    #[test]
    fn writes_selected_modules_and_removes_only_its_own() {
        let path = card("install");
        let both = ["knob-dialog".to_owned(), "midi-sync".to_owned()];
        assert!(install(&path, l6fw::Model::L6max, &both).unwrap());
        assert_eq!(
            files(&path),
            ["10-KNOB-DIALOG-1.0.0.BIN", "30-MIDI-SYNC-1.0.0.BIN"]
        );
        let mut volume = sd_image::open(&path).unwrap();
        assert_eq!(
            volume.read("PATCHES/30-midi-sync-1.0.0.bin").unwrap(),
            module(l6fw::Model::L6max, "midi-sync").unwrap().bytes
        );
        volume.write("PATCHES/02-MINE.BIN", b"user patch").unwrap();
        volume.flush().unwrap();
        drop(volume);
        assert!(!install(&path, l6fw::Model::L6max, &both).unwrap());
        assert!(install(&path, l6fw::Model::L6max, &["midi-sync".into()]).unwrap());
        assert_eq!(files(&path), ["02-MINE.BIN", "30-MIDI-SYNC-1.0.0.BIN"]);
        assert!(install(&path, l6fw::Model::L6max, &[]).unwrap());
        assert_eq!(files(&path), ["02-MINE.BIN"]);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn an_unselected_card_stays_untouched() {
        let path = card("untouched");
        assert!(!install(&path, l6fw::Model::L6, &[]).unwrap());
        assert!(files(&path).is_empty());
        std::fs::remove_file(path).unwrap();
    }
}
