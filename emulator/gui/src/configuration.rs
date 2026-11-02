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

/// The configured card, or one a previous version saved beside the device state,
/// which moves to the configuration.
fn saved_card(
    options: &mut Options,
    state: &Path,
    model: l6fw::Model,
) -> io::Result<Option<PathBuf>> {
    if let Some(card) = options
        .config
        .as_ref()
        .and_then(|config| config.sd_card(model))
    {
        return Ok(Some(card.clone()).filter(|card| card.is_file()));
    }
    let selection = state.join("sd-card-selection.txt");
    if !selection.is_file() {
        return Ok(None);
    }
    let card = PathBuf::from(fs::read_to_string(&selection)?);
    if !card.is_file() {
        return Ok(None);
    }
    if let Some(path) = options.config_path.clone() {
        l6max_host::config::save_sd_card(&path, model, &card)
            .and_then(|()| options.reload_config())
            .map_err(io::Error::other)?;
        fs::remove_file(selection)?;
    }
    Ok(Some(card))
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
    let state = l6max_host::setup::separate_model_state(&state, model)?;
    options.state_dir = Some(state.clone());
    if options.no_sd {
        return Ok(());
    }
    if options.sd_image.is_none() {
        options.sd_image = saved_card(options, &state, model)?;
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

        assert_eq!(
            l6max_host::setup::separate_model_state(&dir, l6fw::Model::L6).unwrap(),
            dir
        );
        assert_eq!(
            l6max_host::setup::separate_model_state(&dir, l6fw::Model::L6max).unwrap(),
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
