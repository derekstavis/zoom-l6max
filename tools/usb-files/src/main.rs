//! Host FAT file access through the MCU's USB mass-storage implementation.
use l6max_host::{
    engine::Options,
    headless::Headless,
    usb_storage::{MassStorage, UsbDisk},
};
use std::{
    env,
    fs::File,
    io::{self, Write},
    time::{Duration, Instant},
};
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = env::args_os().skip(1).collect::<Vec<_>>();
    let split=args.iter().position(|a|a=="--").ok_or("Usage: usb-files --sd-image CARD.img [engine options] -- list [DIR] | get SD_PATH HOST_PATH | put HOST_PATH SD_PATH")?;
    let mut options = Options::parse_args(args[..split].iter().cloned())?;
    options.usb_enabled = true;
    options.host_midi = false;
    if options.sd_image.is_none() {
        return Err("--sd-image is required".into());
    }
    let command = args[split + 1..]
        .iter()
        .map(|s| s.to_str().ok_or("file command must be UTF-8"))
        .collect::<Result<Vec<_>, _>>()?;
    if !matches!(
        command.as_slice(),
        ["list"] | ["list", _] | ["get", _, _] | ["put", _, _] | ["roundtrip"]
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
    let mut disk = UsbDisk::new(MassStorage::new(usb, endpoints))?;
    let filesystem = fatfs::FileSystem::new(&mut disk, fatfs::FsOptions::new())?;
    {
        let root = filesystem.root_dir();
        let path = |s: &str| s.trim_start_matches('/').to_owned();
        match command.as_slice() {
            ["list"] | ["list", _] => {
                let requested = command.get(1).map(|s| path(s)).unwrap_or_default();
                let directory = if !requested.is_empty() {
                    root.open_dir(&requested)?
                } else {
                    root
                };
                for entry in directory.iter() {
                    let e = entry?;
                    println!(
                        "{} {:>10} {}",
                        if e.is_dir() { "dir " } else { "file" },
                        e.len(),
                        e.file_name()
                    );
                }
            }
            ["get", sd, host] => {
                let mut source = root.open_file(&path(sd))?;
                let mut destination = File::create(host)?;
                let bytes = io::copy(&mut source, &mut destination)?;
                println!("Read {bytes} bytes from {sd}");
            }
            ["put", host, sd] => {
                let mut source = File::open(host)?;
                let mut destination = root.create_file(&path(sd))?;
                destination.truncate()?;
                let bytes = io::copy(&mut source, &mut destination)?;
                destination.flush()?;
                println!("Wrote {bytes} bytes to {sd}");
            }
            ["roundtrip"] => {
                use std::io::Read;
                let name = format!("L6T{:05X}.TXT", std::process::id() & 0xfffff);
                match root.open_file(&name) {
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    _ => return Err("roundtrip filename already exists".into()),
                }
                let expected = (0..1537)
                    .map(|i| ((i * 37 + 11) % 256) as u8)
                    .collect::<Vec<_>>();
                {
                    let mut file = root.create_file(&name)?;
                    file.write_all(&expected)?;
                    file.flush()?;
                }
                let mut actual = Vec::new();
                root.open_file(&name)?.read_to_end(&mut actual)?;
                root.remove(&name)?;
                if expected != actual {
                    return Err("USB FAT file roundtrip mismatch".into());
                }
                println!(
                    "USB FAT create/write/read/delete passed: {} bytes",
                    actual.len()
                );
            }
            _ => unreachable!(),
        }
    }
    filesystem.unmount()?;
    disk.flush()?;
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("usb-files: {e}");
        std::process::exit(1);
    }
}
