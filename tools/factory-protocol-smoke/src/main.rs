//! Exercise stock firmware factory SysEx via emulated USB only.
//! Query commands plus entry/exit; uses a fresh disposable state fixture.
use l6max_host::{engine::Options, headless::Headless, usb_midi};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        // Owns only the directory created by this invocation.
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn exchange(
    guest: &Headless,
    out: u8,
    input: u8,
    bytes: &[u8],
) -> Result<Vec<Vec<u8>>, Box<dyn std::error::Error>> {
    let usb = guest.engine.usb.as_ref().unwrap();
    let wire = usb_midi::Encoder::default().push(2, bytes);
    println!("TX cable=2 {bytes:02x?}");
    usb.send(out, &wire)?;
    let deadline = Instant::now() + Duration::from_secs(4);
    let end = guest.guest_ms().saturating_add(400);
    let mut pending = Vec::new();
    let mut replies = Vec::new();
    while guest.guest_ms() < end && Instant::now() < deadline {
        guest.display.sample()?;
        match usb.try_receive(input, 16384) {
            Ok(data) => {
                for (cable, fragment) in usb_midi::decode(&data)? {
                    if cable != 2 {
                        continue;
                    }
                    for b in fragment {
                        if b == 0xf0 {
                            pending.clear();
                        }
                        if b == 0xf0 || !pending.is_empty() {
                            pending.push(b);
                            if b == 0xf7 {
                                println!("RX cable=2 {pending:02x?}");
                                replies.push(std::mem::take(&mut pending));
                            }
                        }
                    }
                }
            }
            Err(e) if e.raw_os_error() == Some(libc::EAGAIN) => {}
            Err(e) => return Err(e.into()),
        }
        thread::sleep(Duration::from_millis(2));
    }
    if !pending.is_empty() {
        return Err(format!("truncated SysEx {pending:02x?}").into());
    }
    Ok(replies)
}
fn expect(replies: Vec<Vec<u8>>, prefix: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    if !replies
        .iter()
        .any(|r| r.starts_with(prefix) && r.last() == Some(&0xf7))
    {
        return Err(format!("missing reply {prefix:02x?}; got {replies:02x?}").into());
    }
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = Options::parse()?;
    if options.state_dir.is_some() || options.sd_image.is_some() {
        return Err("requires --volatile and no SD image".into());
    }
    let hash = format!(
        "{:x}",
        Sha256::digest(fs::read(options.firmware_dir.join("main_firmware.bin"))?)
    );
    if hash != "980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461" {
        return Err("requires original main firmware".into());
    }
    fs::create_dir_all(&options.logs)?;
    let root = options
        .logs
        .join(format!("factory-fixture-{}", std::process::id()));
    fs::create_dir(&root)?;
    let fixture = Fixture(root);
    let card = fixture.0.join("card.img");
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&card)?;
    file.set_len(64 * 1024 * 1024)?;
    fatfs::format_volume(
        &mut file,
        fatfs::FormatVolumeOptions::new().fat_type(fatfs::FatType::Fat32),
    )?;
    {
        let volume = fatfs::FileSystem::new(&mut file, fatfs::FsOptions::new())?;
        volume
            .root_dir()
            .create_file("D445_JIG_000.txt")?
            .write_all(b"emulator fixture; no calibration parameters\n")?;
        volume.unmount()?;
    }
    file.sync_all()?;
    drop(file);
    options.sd_image = Some(card.clone());
    options.usb_enabled = true;
    options.host_midi = false;
    // A valid retained calendar has priority over the factory boot check.
    // Seed only a newly created emulator state directory, never user storage.
    let state = fixture.0.join("state");
    fs::create_dir(&state)?;
    options.state_dir = Some(state);
    {
        let setup = Headless::start(&options)?;
        setup.recorder()?;
        setup.wait(2000)?;
    }
    let guest = Headless::start(&options)?;
    let inputs = guest.engine.inputs.as_ref().unwrap();
    for key in 49..=51 {
        inputs.down(key, 60_000)?;
    }
    guest.until(40_000)?;
    for key in 49..=51 {
        inputs.up(key)?;
    }
    guest.wait(1000)?;
    let mode = guest.engine.management[0].read_memory(0, 0x80462d9c, 4)?;
    println!("Startup mode: {mode:02x?}");
    if mode != 7u32.to_le_bytes() {
        return Err("factory boot combination did not select mode 7".into());
    }
    let pair = guest
        .engine
        .usb
        .as_ref()
        .unwrap()
        .enumerate()?
        .midi
        .ok_or("no MIDI interface")?;
    let send = |bytes: &[u8]| exchange(&guest, pair.output, pair.input, bytes);
    expect(send(&[0xf0, 0x7e, 0x7f, 6, 1, 0xf7])?, &[0xf0, 0x7e])?;
    expect(
        send(&[0xf0, 0x52, 0, 0, 0x67, 0x0b, 0xf7])?,
        &[0xf0, 0x52, 0, 0, 0x67, 0x0c],
    )?;
    for selector in 0..=14 {
        expect(
            send(&[0xf0, 0x52, 0, 0, 0x67, 0x0f, 0, selector, 0xf7])?,
            &[0xf0, 0x52, 0, 0, 0x67, 0x0f, 1, selector, 1],
        )?;
    }
    expect(
        send(&[0xf0, 0x52, 0, 0, 0x67, 0x0f, 4, 0, 0xf7])?,
        &[0xf0, 0x52, 0, 0, 0x67, 0x0f, 5],
    )?;
    expect(
        send(&[0xf0, 0x52, 0, 0, 0x67, 0x0f, 0, 15, 0xf7])?,
        &[0xf0, 0x52, 0, 0, 0x67, 0x0f, 1, 15, 1, 0x7f, 0xf7],
    )?;
    expect(
        send(&[0xf0, 0x52, 0, 0, 0x67, 0x0f, 0, 20, 0xf7])?,
        &[0xf0, 0x52, 0, 0, 0x67, 0x0f, 1, 20, 1, 0x7f, 0xf7],
    )?;
    if !send(&[0xf0, 0x52, 0, 0, 0x67, 0x0f, 1, 0, 0xf7])?.is_empty() {
        return Err("unsupported factory command received a reply".into());
    }
    for selector in 0..=3 {
        expect(
            send(&[0xf0, 0x52, 0, 0, 0x10, selector, 0xf7])?,
            &[0xf0, 0x52, 0, 0, 0x11, selector],
        )?;
    }
    guest.wait(1500)?;
    let qmp = &guest.engine.management[0];
    if qmp.read_memory(0, 0x805dca98, 1)? != [0]
        || qmp.read_memory(0, 0x80462da8, 4)? != [0, 0, 0, 0]
    {
        return Err("keepalive expiry did not clear session and connection".into());
    }
    println!("Keepalive expiry cleared session and connection");
    expect(send(&[0xf0, 0x7e, 0x7f, 6, 1, 0xf7])?, &[0xf0, 0x7e])?;
    expect(
        send(&[0xf0, 0x52, 0, 0, 0x67, 0x0b, 0xf7])?,
        &[0xf0, 0x52, 0, 0, 0x67, 0x0c],
    )?;
    expect(
        send(&[0xf0, 0x52, 0, 0, 0x67, 0x0f, 0x13, 0, 0xf7])?,
        &[0xf0, 0x52, 0, 0, 0x67, 0x0e],
    )?;
    if qmp.read_memory(0, 0x805dca98, 1)? != [0] {
        return Err("inspection exit did not reset session".into());
    }
    drop(guest);
    drop(fixture);
    println!(
        "Factory entry, input queries, calibration presence, version queries, keepalive and exit passed"
    );
    Ok(())
}
