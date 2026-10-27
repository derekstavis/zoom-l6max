//! Create an empty FAT32 or populated demo card without replacing existing media.
use std::{ffi::OsString, fs::OpenOptions, path::Path};

pub fn run(args: Vec<OsString>) -> Result<(), Box<dyn std::error::Error>> {
    if args
        .first()
        .is_some_and(|arg| matches!(arg.to_str(), Some("help" | "--help" | "-h")))
    {
        println!(
            "Usage: l6fs card create PATH [--size-mib MIB]\n       l6fs card demo PATH --firmware-package PACKAGE\n       l6fs card import PATH --directory FOLDER"
        );
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "import") {
        if args.len() != 4 || args[2] != "--directory" {
            return Err("Usage: l6fs card import PATH --directory FOLDER".into());
        }
        sd_image::from_directory(Path::new(&args[1]), Path::new(&args[3]))?;
        println!("Created card from folder: {}", args[1].to_string_lossy());
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "demo") {
        if args.len() != 4 || args[2] != "--firmware-package" {
            return Err("Usage: l6fs card demo PATH --firmware-package PACKAGE".into());
        }
        sd_image::create(Path::new(&args[1]), Path::new(&args[3]))?;
        println!("Created 128 MiB demo card: {}", args[1].to_string_lossy());
        return Ok(());
    }
    if args.first().is_none_or(|arg| arg != "create") || args.len() < 2 {
        return Err("Usage: l6fs card create PATH [--size-mib MIB]".into());
    }
    let mib = match args.as_slice() {
        [_, _, flag, value] if flag == "--size-mib" => {
            value.to_str().ok_or("size must be UTF-8")?.parse::<u64>()?
        }
        [_, _] => 64,
        _ => return Err("Usage: l6fs card create PATH [--size-mib MIB]".into()),
    };
    if mib < 64 || !mib.is_power_of_two() || mib > 1048576 {
        return Err("size must be a power of two between 64 and 1048576 MiB".into());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&args[1])?;
    file.set_len(mib * 1024 * 1024)?;
    fatfs::format_volume(
        &mut file,
        fatfs::FormatVolumeOptions::new()
            .fat_type(fatfs::FatType::Fat32)
            .volume_label(*b"L6MAX      "),
    )?;
    file.sync_all()?;
    println!(
        "Created {mib} MiB FAT32 card: {}",
        args[1].to_string_lossy()
    );
    Ok(())
}
