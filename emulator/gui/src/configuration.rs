//! Persistent demo media defaults for the visual application.
use l6max_host::engine::Options;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

fn booted_model(firmware_dir: &Path, package: Option<&Path>) -> io::Result<l6fw::Model> {
    let main = firmware_dir.join("main_firmware.bin");
    match fs::read(&main) {
        Ok(bytes) => l6fw::model_from_main(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let package = package.ok_or(error)?;
            let bytes = fs::read(package)?;
            l6fw::model_from_package(&bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
        }
        Err(error) => Err(error),
    }
}

fn package_for_model(model: l6fw::Model, package: Option<&Path>) -> io::Result<PathBuf> {
    if let Some(package) = package {
        if fs::read(package)
            .ok()
            .and_then(|bytes| l6fw::model_from_package(&bytes).ok())
            == Some(model)
        {
            return Ok(package.to_path_buf());
        }
    }
    let filenames = match model {
        l6fw::Model::L6 => ["L6.BIN", "L6.bin"],
        l6fw::Model::L6max => ["L6max.bin", "L6max.BIN"],
    };
    let root = l6max_host::paths::repository_root();
    filenames
        .into_iter()
        .flat_map(|filename| [root.join(filename), root.join("firmware").join(filename)])
        .into_iter()
        .find(|candidate| {
            fs::read(candidate)
                .ok()
                .and_then(|bytes| l6fw::model_from_package(&bytes).ok())
                == Some(model)
        })
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "no supported {} package is available for the booted firmware",
                    filenames[0]
                ),
            )
        })
}

fn default_card_path(state: &Path, model: l6fw::Model) -> PathBuf {
    match model {
        l6fw::Model::L6 => state.join("sd-card-l6.img"),
        l6fw::Model::L6max => state.join("sd-card.img"),
    }
}

fn model_state_path(state: &Path, model: l6fw::Model) -> PathBuf {
    let model_name = match model {
        l6fw::Model::L6 => "l6",
        l6fw::Model::L6max => "l6max",
    };
    state.join("models").join(model_name)
}

/// Keep an existing legacy state with its installed MAIN image. If the
/// selected package targets the other model, use a separate persistent device
/// directory so its NOR, panel flash, RTC, and SD selection cannot cross over.
fn separate_model_state(state: &Path, model: l6fw::Model) -> io::Result<PathBuf> {
    let model_state = model_state_path(state, model);
    if model_state.is_dir() {
        let main_nor = model_state.join("main-nor.bin");
        if main_nor.is_file() {
            let installed = l6fw::model_from_main_nor(&fs::read(main_nor)?)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            if installed != model {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "model-specific state contains firmware for a different device",
                ));
            }
        }
        return Ok(model_state);
    }
    let main_nor = state.join("main-nor.bin");
    match fs::read(&main_nor) {
        Ok(bytes) => {
            let installed = l6fw::model_from_main_nor(&bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            if installed != model {
                return Ok(model_state);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if ["panel-flash.bin", "resource-nor.bin", "panel-rtc.bin"]
                .iter()
                .any(|name| state.join(name).exists())
            {
                return Ok(model_state);
            }
        }
        Err(error) => return Err(error),
    }
    Ok(state.to_path_buf())
}
/// Apply visual UI defaults without changing disposable runs or explicit media.
pub fn configure(options: &mut Options) -> io::Result<()> {
    let package = options.firmware_package.clone();
    configure_with_package(options, package.as_deref())
}

pub fn configure_with_package(
    options: &mut Options,
    package: Option<&std::path::Path>,
) -> io::Result<()> {
    if options.qemu.is_none() {
        return Ok(());
    }
    let model = booted_model(&options.firmware_dir, package)?;
    let Some(state) = options.state_dir.clone() else {
        return Ok(());
    };
    let state = separate_model_state(&state, model)?;
    options.state_dir = Some(state.clone());
    if options.no_sd {
        return Ok(());
    }
    if options.sd_image.is_none() {
        let selection = state.join("sd-card-selection.txt");
        if selection.is_file() {
            let card = std::path::PathBuf::from(std::fs::read_to_string(&selection)?);
            if card.is_file() {
                options.sd_image = Some(card);
            }
        }
    }
    if options.sd_image.is_none() {
        let card = default_card_path(&state, model);
        if !card.exists() {
            let matching_package = package_for_model(model, package)?;
            l6max_host::demo_sd::create(&card, &matching_package)?;
            eprintln!("Created demo SD card: {}", card.display());
        }
        options.sd_image = Some(card);
    }
    // The replacement installer selects the package and flash mapping from the
    // MAIN image actually booted, for both model layouts.
    options.update_bootloader = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn main_image(reset: u32) -> Vec<u8> {
        let mut image = vec![0; 8];
        image[..4].copy_from_slice(&0x2001_0000u32.to_le_bytes());
        image[4..].copy_from_slice(&reset.to_le_bytes());
        image
    }

    #[test]
    fn the_booted_main_image_selects_the_model_over_a_conflicting_package() {
        let dir = std::env::temp_dir().join(format!("l6-boot-model-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("main_firmware.bin"), main_image(0x8000_1415)).unwrap();
        let misleading_package = dir.join("L6max.bin");
        fs::write(
            &misleading_package,
            b"package does not override the booted image",
        )
        .unwrap();
        assert_eq!(
            booted_model(&dir, Some(&misleading_package)).unwrap(),
            l6fw::Model::L6
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn generated_default_cards_are_separate_by_model() {
        let state = Path::new("/state");
        assert_eq!(
            default_card_path(state, l6fw::Model::L6),
            Path::new("/state/sd-card-l6.img")
        );
        assert_eq!(
            default_card_path(state, l6fw::Model::L6max),
            Path::new("/state/sd-card.img")
        );
    }

    #[test]
    fn installed_model_keeps_legacy_state_while_other_model_gets_isolated_storage() {
        let dir = std::env::temp_dir().join(format!("l6-model-state-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut nor = vec![0xff; 0x200000];
        nor[0x50000..0x50004].copy_from_slice(&0x20220000u32.to_le_bytes());
        nor[0x50004..0x50008].copy_from_slice(&0x80001415u32.to_le_bytes());
        fs::write(dir.join("main-nor.bin"), nor).unwrap();

        assert_eq!(separate_model_state(&dir, l6fw::Model::L6).unwrap(), dir);
        assert_eq!(
            separate_model_state(&dir, l6fw::Model::L6max).unwrap(),
            dir.join("models/l6max")
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn persistent_gui_enables_the_model_aware_installer_for_both_models() {
        let dir = std::env::temp_dir().join(format!("l6-update-default-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let state = dir.join("state");
        fs::create_dir_all(&state).unwrap();
        let card = dir.join("card.img");
        fs::write(&card, b"test card").unwrap();

        for (name, reset) in [("l6", 0x8000_1415), ("l6max", 0x8000_0415)] {
            let firmware = dir.join(name);
            fs::create_dir_all(&firmware).unwrap();
            fs::write(firmware.join("main_firmware.bin"), main_image(reset)).unwrap();
            let mut options = Options::parse_args([]).unwrap();
            options.qemu = Some(PathBuf::from("qemu"));
            options.firmware_dir = firmware;
            options.state_dir = Some(state.clone());
            options.sd_image = Some(card.clone());
            options.update_bootloader = false;

            configure_with_package(&mut options, None).unwrap();

            assert!(options.update_bootloader, "{name}");
        }

        fs::remove_dir_all(dir).unwrap();
    }
}
