//! Real application confirmation/power handoff, modeled SD installer, real panel ROM exchange.
//! Owns disposable media/state only; never installs firmware on physical hardware.
use l6max_host::{engine::Options, headless::Headless};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn sum(image: &[u8]) -> u32 {
    image[0x200..0x1a8200]
        .iter()
        .fold(0u32, |sum, &b| sum.wrapping_add(b as u32))
}
fn word(guest: &Headless, address: u32) -> Result<u32, Box<dyn std::error::Error>> {
    Ok(u32::from_le_bytes(
        guest.engine.management[0]
            .read_memory(0, address, 4)?
            .try_into()
            .unwrap(),
    ))
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"));
    let options = Options::parse()?;
    if options.state_dir.is_some() || options.sd_image.is_some() {
        return Err("requires --volatile and no supplied state or SD image".into());
    }
    let original = fs::read(base.join("../../L6max.bin"))?;
    if format!("{:x}", Sha256::digest(&original))
        != "4e20f92c9c5131ed082c0feec2b2d7d0445fb576296f11f6dace56a4fc0c9b47"
    {
        return Err("requires original package".into());
    }
    if format!(
        "{:x}",
        Sha256::digest(fs::read(options.firmware_dir.join("main_firmware.bin"))?)
    ) != "980d92272c1f84bed200b4d2f83575202f6a00082c928d99b9e8b53af8d44461"
    {
        return Err("requires original main image".into());
    }
    let expected = u32::from_be_bytes(original[0x1fc..0x200].try_into().unwrap());
    assert_eq!(sum(&original), expected);
    println!("Original MAIN byte sum verified: {expected:08x}");
    fs::create_dir_all(&options.logs)?;
    let root = options
        .logs
        .join(format!("update-fixture-{}", std::process::id()));
    fs::create_dir(&root)?;
    let fixture = Fixture(root);
    for valid in [false, true] {
        let label = if valid { "valid" } else { "bad-checksum" };
        let directory = fixture.0.join(label);
        fs::create_dir(&directory)?;
        let mut package = original.clone();
        for offset in [0x1a31fc, 0x1a81fc] {
            package[offset..offset + 4].copy_from_slice(b"0111");
        }
        package[0x1a71fc..0x1a7200].copy_from_slice(b"0101");
        // Reserved vector slots: distinguish installed images without replacing executable code.
        package[0x21c..0x220].copy_from_slice(&0x13579bdfu32.to_le_bytes());
        package[0x1a321c..0x1a3220].copy_from_slice(&0x2468ace0u32.to_le_bytes());
        if valid {
            let checksum = sum(&package).to_be_bytes();
            package[0x1fc..0x200].copy_from_slice(&checksum);
        }
        let card = directory.join("card.img");
        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&card)?;
        // Firmware expects 32 sectors/cluster for this capacity, unlike fatfs defaults.
        file.set_len(128 * 1024 * 1024)?;
        fatfs::format_volume(
            &mut file,
            fatfs::FormatVolumeOptions::new()
                .fat_type(fatfs::FatType::Fat16)
                .bytes_per_cluster(16384),
        )?;
        let volume = fatfs::FileSystem::new(&mut file, fatfs::FsOptions::new())?;
        volume
            .root_dir()
            .create_file("L6max.BIN")?
            .write_all(&package)?;
        volume.unmount()?;
        file.sync_all()?;
        drop(file);
        let mut local = Options::parse()?;
        local.sd_image = Some(card);
        local.state_dir = Some(directory.join("state"));
        local.update_bootloader = false;
        local.logs = options.logs.join(format!("update-{label}"));
        let guest = Headless::start(&local)?;
        guest.until(40000)?;
        let mode = word(&guest, 0x80462d9c)?;
        let flags = guest.engine.management[0].read_memory(0, 0x8020a5ca, 1)?[0];
        println!(
            "{label}: mode={mode} SD flags={flags:02x} window={:08x}",
            word(&guest, 0x80202610)?
        );
        assert_eq!(flags & 9, 0, "SD readiness/geometry gate");
        assert_eq!(mode, if valid { 6 } else { 1 });
        let nor_path = directory.join("state/main-nor.bin");
        if valid {
            assert_eq!(word(&guest, 0x80202610)?, 0x802003b4);
            guest.tap(2, 50, 1000)?;
            guest.tap(3, 50, 1000)?;
            assert_eq!(word(&guest, 0x80202610)?, 0x8020029c);
            assert_ne!(
                word(&guest, 0x8020029c)? & 0x100,
                0,
                "preparation entry did not finish"
            );
            let before = fs::read(&nor_path)?;
            assert_ne!(&before[0x1ff000..0x1ff008], b"FW UPDT\0");
            guest.engine.inputs.as_ref().unwrap().tap(53, 3500)?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
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
                if std::time::Instant::now() > deadline {
                    return Err("firmware did not cut power".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            drop(guest);
            let pending = fs::read(&nor_path)?;
            assert_eq!(&pending[0x1ff000..0x1ff008], b"FW UPDT\0");
            local.update_bootloader = true;
            local.logs = options.logs.join("update-installed");
            let installed = Headless::start(&local)?;
            installed.until(40000)?;
            assert_eq!(
                fs::read_to_string(local.logs.join("update-bootloader.log"))?,
                "Installed\n"
            );
            assert_eq!(
                word(&installed, 0x80462d9c)?,
                4,
                "application success handoff"
            );
            assert_eq!(
                word(&installed, 0x8000001c)?,
                0x13579bdf,
                "installed main image mapped"
            );
            let programmed = fs::read_to_string(local.logs.join("engine.trace"))?;
            assert!(programmed.contains("l6 panel ROM 16 KiB image match: yes"));
            assert_eq!(
                programmed
                    .lines()
                    .filter(|line| line.starts_with("l6 panel ROM write 080"))
                    .count(),
                64
            );
            drop(installed);
            let after = fs::read(&nor_path)?;
            assert_eq!(
                &after[..0x50000],
                &pending[..0x50000],
                "bootloader region changed"
            );
            assert_eq!(
                &after[0x50000..0x1f8000],
                &package[0x200..],
                "installed MAIN payload differs"
            );
            assert_eq!(
                &after[0x1f8000..0x1ff000],
                &pending[0x1f8000..0x1ff000],
                "calibration/settings changed"
            );
            assert_ne!(
                &after[0x1ff000..0x1ff008],
                b"FW UPDT\0",
                "handoff not consumed"
            );
            let panel = fs::read(directory.join("state/panel-flash.bin"))?;
            assert_eq!(&panel[..0x3ffc], &package[0x1a3200..0x1a71fc]);
            assert_eq!(&panel[0x3ffc..0x4000], b"0101");
            println!(
                "PASS: main 0111 installed; firmware programmed/verified panel 0101; storage preserved; success handoff consumed"
            );
            local.sd_image = None;
            local.logs = options.logs.join("update-restart");
            let restarted = Headless::start(&local)?;
            restarted.until(40000)?;
            assert_eq!(word(&restarted, 0x8000001c)?, 0x13579bdf);
            drop(restarted);
            assert_eq!(
                fs::read_to_string(local.logs.join("update-bootloader.log"))?,
                "NoPending\n"
            );
            assert_eq!(fs::read(directory.join("state/panel-flash.bin"))?, panel);
            let trace = fs::read_to_string(local.logs.join("engine.trace"))?;
            assert!(
                !trace.contains("l6 panel ROM erase") && !trace.contains("l6 panel ROM mass erase"),
                "unexpected panel reinstall"
            );
            println!("PASS: installed images survive another launch without SD");
        } else {
            drop(guest);
            let nor = fs::read(&nor_path)?;
            assert_ne!(&nor[0x1ff000..0x1ff008], b"FW UPDT\0");
            println!("PASS: corrupt package does not trigger update");
        }
    }
    Ok(())
}
