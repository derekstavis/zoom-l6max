//! Stock GPIO update flow followed by behavioral validation of the installed patch.
use l6max_host::{engine::Options, headless::Headless};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    thread,
    time::{Duration, Instant},
};
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn word(h: &Headless, address: u32) -> Result<u32, Box<dyn std::error::Error>> {
    Ok(u32::from_le_bytes(
        h.engine.management[0]
            .read_memory(0, address, 4)?
            .try_into()
            .unwrap(),
    ))
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let package_path = PathBuf::from(
        args.next()
            .ok_or("Usage: patched-update-smoke PACKAGE --volatile [engine options]")?,
    )
    .canonicalize()?;
    let mut options = Options::parse_args(args)?;
    if options.state_dir.is_some() || options.sd_image.is_some() {
        return Err("requires --volatile and no supplied SD/state".into());
    }
    let package = fs::read(&package_path)?;
    l6max_host::package::check_layout(&package)?;
    let payload = l6max_host::update_bootloader::validate(&package)?;
    let original = fs::read(options.firmware_dir.join("main_firmware.bin"))?;
    if format!("{:x}", Sha256::digest(&original))
        != "980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461"
    {
        return Err("requires original main firmware as the starting image".into());
    }
    if &package[0x1a31fc..0x1a3200] <= b"0110" {
        return Err("this startup-prompt diagnostic requires a newer development marker, e.g. --update-version 0111".into());
    }
    let panel = fs::read(options.firmware_dir.join("secondary_firmware.bin"))?;
    assert_eq!(&package[0x1a3200..0x1a6cd8], panel);
    assert_eq!(&package[0x1a71fc..0x1a7200], b"0100");
    fs::create_dir_all(&options.logs)?;
    let fixture = Fixture(
        options
            .logs
            .join(format!("patched-update-fixture-{}", std::process::id())),
    );
    fs::create_dir(&fixture.0)?;
    let card = fixture.0.join("card.img");
    l6max_host::demo_sd::create(&card, &package_path)?;
    let state = fixture.0.join("state");
    options.state_dir = Some(state.clone());
    options.sd_image = Some(card);
    options.update_bootloader = false;
    let guest = Headless::start(&options)?;
    guest.until(40000)?;
    assert_eq!(word(&guest, 0x80462d9c)?, 6);
    assert_eq!(word(&guest, 0x80202610)?, 0x802003b4);
    guest.tap(2, 50, 1000)?;
    guest.tap(3, 50, 1000)?;
    assert_eq!(word(&guest, 0x80202610)?, 0x8020029c);
    assert_ne!(word(&guest, 0x8020029c)? & 0x100, 0);
    guest.engine.inputs.as_ref().unwrap().tap(53, 3500)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while !guest
        .engine
        .inputs
        .as_ref()
        .unwrap()
        .main
        .snapshot()
        .powered_off
    {
        guest.display.sample()?;
        if Instant::now() > deadline {
            return Err("firmware did not cut power".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    drop(guest);
    // Normal shutdown saves settings. Compare storage at the installer boundary,
    // before the restarted application can make its own legitimate writes.
    let before = fs::read(state.join("main-nor.bin"))?;
    assert_eq!(&before[0x1ff000..0x1ff008], b"FW UPDT\0");
    let outcome = l6max_host::update_bootloader::install(&state, options.sd_image.as_deref())?;
    assert_eq!(outcome, l6max_host::update_bootloader::Outcome::Installed);
    let after = fs::read(state.join("main-nor.bin"))?;
    assert!(
        &after[0x50000..0x1f8000] == payload,
        "installed main image differs"
    );
    assert!(
        after[..0x50000] == before[..0x50000],
        "installer changed boot region"
    );
    assert!(
        after[0x1f8000..0x1ff000] == before[0x1f8000..0x1ff000],
        "installer changed calibration/settings"
    );
    options.logs = options.logs.join("installed");
    let installed = Headless::start(&options)?;
    installed.until(40000)?;
    assert_eq!(word(&installed, 0x80462d9c)?, 4);
    drop(installed);
    assert_ne!(
        &fs::read(state.join("main-nor.bin"))?[0x1ff000..0x1ff008],
        b"FW UPDT\0"
    );
    println!(
        "PASS: stock firmware accepted patched package; GPIO shutdown saved marker; installed payload matches; boot/calibration/settings preserved"
    );
    let test = std::env::current_exe()?.with_file_name("knob-dialog-smoke");
    let mut command = Command::new(test);
    command
        .arg("--installed-package")
        .arg(&package_path)
        .arg("--state-dir")
        .arg(&state)
        .arg("--firmware-dir")
        .arg(&options.firmware_dir)
        .arg("--logs")
        .arg(options.logs.join("knobs"));
    if let Some(qemu) = &options.qemu {
        command.arg("--qemu").arg(qemu);
    }
    if !command.status()?.success() {
        return Err("installed knob patch behavior failed".into());
    }
    println!("PASS: repacked firmware patch works after SD installation and another restart");
    Ok(())
}
