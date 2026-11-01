//! Host FAT file access through the MCU's USB mass-storage implementation.
use l6max_host::{
    engine::Options,
    headless::Headless,
    usb_storage::{MassStorage, UsbDisk},
};
use std::{
    ffi::OsString,
    fs::File,
    io::Write,
    time::{Duration, Instant},
};
pub fn run(args: Vec<OsString>) -> Result<(), Box<dyn std::error::Error>> {
    if args
        .first()
        .is_some_and(|arg| matches!(arg.to_str(), Some("--help" | "-h")))
    {
        println!(
            "Usage: l6fs usb --sd-image CARD.img [engine options] -- list [DIR] | get SD_PATH HOST_PATH | put HOST_PATH SD_PATH"
        );
        return Ok(());
    }
    let split = args
        .iter()
        .position(|arg| arg == "--")
        .ok_or("Usage: l6fs usb --sd-image CARD.img [engine options] -- list [DIR] | get SD_PATH HOST_PATH | put HOST_PATH SD_PATH")?;
    let options = Options::parse_args(args[..split].iter().cloned())?;
    if options.sd_image.is_none() {
        return Err("--sd-image is required".into());
    }
    let command = args[split + 1..]
        .iter()
        .map(|s| s.to_str().ok_or("file command must be UTF-8"))
        .collect::<Result<Vec<_>, _>>()?;
    if !matches!(
        command.as_slice(),
        ["list"] | ["list", _] | ["get", _, _] | ["put", _, _]
    ) {
        return Err("use list [DIR], get SD_PATH HOST_PATH, or put HOST_PATH SD_PATH".into());
    }
    let headless = Headless::start(&options)?;
    headless.file_transfer()?;
    let usb = headless.engine.usb.as_ref().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let config = loop {
        match usb.enumerate() {
            Ok(c) => break c,
            Err(e) if e.raw_os_error() == Some(libc::ENODEV) && Instant::now() < deadline => {
                headless.wait(1000)?
            }
            Err(e) => return Err(e.into()),
        }
    };
    let endpoints = config
        .storage
        .ok_or("firmware is not in USB File Transfer mode")?;
    let disk = UsbDisk::new(MassStorage::new(usb, endpoints))?;
    let mut volume = sd_image::Volume::open(disk)?;
    match command.as_slice() {
        ["list"] | ["list", _] => {
            for entry in volume.entries(command.get(1).copied().unwrap_or("/"))? {
                println!(
                    "{} {:>10} {}",
                    if entry.directory { "dir " } else { "file" },
                    entry.length,
                    entry.name
                );
            }
        }
        ["get", sd, host] => {
            let bytes = volume.read(sd)?;
            File::create(host)?.write_all(&bytes)?;
            println!("Read {} bytes from {sd}", bytes.len());
        }
        ["put", host, sd] => {
            let bytes = std::fs::read(host)?;
            volume.write(sd, &bytes)?;
            println!("Wrote {} bytes to {sd}", bytes.len());
        }
        _ => unreachable!(),
    }
    volume.flush()?;
    Ok(())
}
