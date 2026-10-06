//! Persistent demo media defaults for the visual application.
use l6max_host::engine::Options;
use std::io;
/// Apply visual UI defaults without changing disposable runs or explicit media.
pub fn configure(options: &mut Options) -> io::Result<()> {
    configure_with_package(options, None)
}

pub fn configure_with_package(
    options: &mut Options,
    package: Option<&std::path::Path>,
) -> io::Result<()> {
    if options.qemu.is_none() || options.ui_input_smoke || options.no_sd {
        return Ok(());
    }
    let Some(state) = &options.state_dir else {
        return Ok(());
    };
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
        let card = state.join("sd-card.img");
        if !card.exists() {
            l6max_host::demo_sd::create(
                &card,
                package.unwrap_or(&l6max_host::paths::repository_root().join("L6max.bin")),
            )?;
            eprintln!("Created demo SD card: {}", card.display());
        }
        options.sd_image = Some(card);
    }
    // Only the standard persistent visual UI enables this automatically.
    options.update_bootloader = true;
    Ok(())
}
