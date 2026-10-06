//! Enumerate the firmware's USB device via ChipIdea setup packets and dTD DMA.
use l6max_host::{
    display::PublishedDisplay,
    engine::{Engine, Options},
};
use std::{
    fs, thread,
    time::{Duration, Instant},
};
fn capture(
    display: &PublishedDisplay,
    engine: &Engine,
    logs: &std::path::Path,
    name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut latest = None;
    let end = engine.inputs.as_ref().unwrap().panel.snapshot().guest_ms + 1000;
    let deadline = Instant::now() + Duration::from_secs(15);
    while engine.inputs.as_ref().unwrap().panel.snapshot().guest_ms < end {
        if let Some(frame) = display.sample()? {
            latest = Some(frame);
        }
        match engine.usb.as_ref().unwrap().try_receive(4, 16384) {
            Ok(bytes) => {
                if !bytes.is_empty() {
                    println!("MIDI: {bytes:02x?}");
                }
            }
            Err(e)
                if e.raw_os_error() == Some(libc::EAGAIN)
                    || e.raw_os_error() == Some(libc::ENODEV) => {}
            Err(e) => return Err(e.into()),
        }
        if Instant::now() > deadline {
            return Err("capture guest deadline".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    if let Some(raw) = latest {
        image::GrayImage::from_fn(128, 64, |x, y| {
            image::Luma([
                if raw[((63 - y as usize) / 8) * 128 + x as usize] & (1 << ((63 - y as usize) % 8))
                    != 0
                {
                    255
                } else {
                    0
                },
            ])
        })
        .save(logs.join(name))?;
    }
    Ok(())
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = Options::parse()?;
    options.usb_enabled = true;
    options.host_midi = false; // This diagnostic owns the USB host tokens.
    options.trace_sd = options.sd_image.is_some();
    let engine = Engine::start(&options)?;
    let inputs = engine.inputs.as_ref().ok_or("QEMU required")?;
    let display = PublishedDisplay::new(
        engine.display_memory.as_ref().unwrap().map(),
        inputs.main.clone(),
    )?;
    let deadline = Instant::now() + Duration::from_secs(100);
    while inputs.panel.snapshot().guest_ms < 40000 {
        display.sample()?;
        if Instant::now() > deadline {
            return Err("USB boot deadline".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    inputs.tap(3, 50)?;
    while inputs.panel.snapshot().guest_ms < 42000 {
        display.sample()?;
        thread::sleep(Duration::from_millis(5));
    }
    inputs.tap(3, 50)?;
    while inputs.panel.snapshot().guest_ms < 47000 {
        display.sample()?;
        thread::sleep(Duration::from_millis(5));
    }
    let usb = engine.usb.as_ref().unwrap();
    if options.sd_image.is_some() {
        inputs.tap(0, 500)?;
        capture(&display, &engine, &options.logs, "pre-usb-menu-0.png")?;
        for i in 1..=5 {
            inputs.tap(2, 50)?;
            capture(
                &display,
                &engine,
                &options.logs,
                &format!("pre-usb-menu-{i}.png"),
            )?;
        }
        inputs.tap(3, 50)?;
        capture(&display, &engine, &options.logs, "usb-file-transfer.png")?;
        let deadline = Instant::now() + Duration::from_secs(30);
        let configuration = loop {
            match usb.enumerate() {
                Ok(c) => break c,
                Err(e) if e.raw_os_error() == Some(libc::ENODEV) && Instant::now() < deadline => {
                    capture(
                        &display,
                        &engine,
                        &options.logs,
                        "usb-file-transfer-wait.png",
                    )?;
                }
                Err(e) => {
                    eprintln!("{}", engine.management[0].registers(0)?);
                    return Err(e.into());
                }
            }
        };
        println!("Storage configuration: {configuration:?}");
        let endpoints = configuration
            .storage
            .ok_or("firmware did not expose mass storage")?;
        let mut disk = l6max_host::usb_storage::MassStorage::new(usb, endpoints);
        let inquiry = disk.inquiry()?;
        println!("SCSI INQUIRY: {}", String::from_utf8_lossy(&inquiry[8..36]));
        disk.ready()?;
        let (blocks, size) = disk.capacity()?;
        println!("SCSI capacity: {blocks} blocks of {size} bytes");
        if size != 512 {
            return Err("unsupported sector size".into());
        }
        let boot = disk.read_sector(0)?;
        let image = fs::read(options.sd_image.as_ref().unwrap())?;
        if boot != image[..512] {
            return Err("USB sector did not match attached SD image".into());
        }
        let original = disk.read_sector((blocks - 1) as u32)?;
        disk.write_sector((blocks - 1) as u32, &original)?;
        if disk.read_sector((blocks - 1) as u32)? != original {
            return Err("USB sector write/read mismatch".into());
        }
        println!("Firmware USB storage sector read/write passed");
        return Ok(());
    }
    usb.connect(true)?;
    for _ in 0..100 {
        display.sample()?;
        thread::sleep(Duration::from_millis(5));
    }
    let descriptor = usb.control([0x80, 6, 0, 1, 0, 0, 18, 0])?;
    if descriptor.len() != 18 || descriptor[0..2] != [18, 1] {
        return Err(format!("invalid device descriptor {descriptor:02x?}").into());
    }
    fs::write(options.logs.join("usb-device.bin"), &descriptor)?;
    println!(
        "USB VID={:04x} PID={:04x}",
        u16::from_le_bytes([descriptor[8], descriptor[9]]),
        u16::from_le_bytes([descriptor[10], descriptor[11]])
    );
    let config = usb.control([0x80, 6, 0, 2, 0, 0, 0xff, 0x0f])?;
    fs::write(options.logs.join("usb-config.bin"), &config)?;
    if config.len() < 9
        || config[0..2] != [9, 2]
        || u16::from_le_bytes([config[2], config[3]]) as usize != config.len()
    {
        return Err("invalid configuration descriptor".into());
    }
    println!(
        "USB: {} configuration bytes, {} interfaces",
        config.len(),
        config[4]
    );
    usb.control([0, 9, config[5], 0, 0, 0, 0, 0])?;
    println!("USB SET_CONFIGURATION accepted by firmware");
    inputs.encoder(0, -4)?;
    let midi = usb.receive(4, 16384)?;
    println!("USB MIDI IN: {midi:02x?}");
    fs::write(options.logs.join("usb-midi.bin"), &midi)?;
    usb.send(3, &[0x1b, 0xb0, 0x51, 32])?;
    capture(&display, &engine, &options.logs, "usb-midi-home.png")?;
    if options.sd_image.is_some() {
        inputs.tap(0, 500)?;
        capture(&display, &engine, &options.logs, "usb-menu-0.png")?;
        for i in 1..=8 {
            inputs.tap(2, 50)?;
            capture(
                &display,
                &engine,
                &options.logs,
                &format!("usb-menu-{i}.png"),
            )?;
        }
    }
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("usb-smoke: {e}");
        std::process::exit(1);
    }
}
