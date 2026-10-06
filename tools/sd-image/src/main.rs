//! Create an empty FAT32 or populated demo card without replacing existing media.
use std::{env, fs::OpenOptions};
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() == 3 && args[1] == "--directory" {
        sd_image::from_directory(
            std::path::Path::new(&args[0]),
            std::path::Path::new(&args[2]),
        )?;
        println!("Created card from folder: {}", args[0].to_string_lossy());
        return Ok(());
    }
    if args.len() == 3 && args[1] == "--demo" {
        sd_image::create(
            std::path::Path::new(&args[0]),
            std::path::Path::new(&args[2]),
        )?;
        println!("Created 128 MiB demo card: {}", args[0].to_string_lossy());
        return Ok(());
    }
    if args.is_empty() || args.len() > 2 {
        return Err(
            "Usage: sd-image PATH [MiB, default 64] | sd-image PATH --demo FIRMWARE | sd-image PATH --directory FOLDER".into(),
        );
    }
    let mib = if let Some(s) = args.get(1) {
        s.to_str().ok_or("size must be UTF-8")?.parse::<u64>()?
    } else {
        64
    };
    if mib < 64 || !mib.is_power_of_two() || mib > 1048576 {
        return Err("size must be a power of two between 64 and 1048576 MiB".into());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&args[0])?;
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
        args[0].to_string_lossy()
    );
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("sd-image: {e}");
        std::process::exit(1);
    }
}
