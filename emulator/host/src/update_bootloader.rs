//! Opt-in replacement for the unavailable ZOOM SD installer, not its ROM code.
//! Runs before QEMU owns storage and maps validated packages per detected model.
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::fd::AsRawFd,
    path::Path,
};
const NOR_SIZE: usize = 0x200000;
const MARKER: usize = 0x1ff000;
const PACKAGE_SIZE: usize = 0x1a8200;
const MAX_PACKAGE_SIZE: usize = 0x395200;
const L6_RESOURCE_SIZE: usize = 0x400000;
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    NoPending,
    Installed,
    Failed(String),
    PreviousFailure,
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn le32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
pub use l6fw::update::validate;
/// A bounded FAT partition view; the card is opened read-only. Supports a raw
/// FAT volume or the first FAT partition in an MBR, not GPT or extended chains.
struct Volume {
    file: File,
    start: u64,
    length: u64,
    position: u64,
}
impl Read for Volume {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let count = bytes.len().min((self.length - self.position) as usize);
        self.file
            .seek(SeekFrom::Start(self.start + self.position))?;
        let n = self.file.read(&mut bytes[..count])?;
        self.position += n as u64;
        Ok(n)
    }
}
impl Write for Volume {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "installer SD view is read-only",
        ))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Seek for Volume {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let target = match from {
            SeekFrom::Start(n) => i128::from(n),
            SeekFrom::Current(n) => i128::from(self.position) + i128::from(n),
            SeekFrom::End(n) => i128::from(self.length) + i128::from(n),
        };
        if target < 0 || target > i128::from(self.length) {
            return Err(invalid("FAT seek outside partition"));
        }
        self.position = target as u64;
        Ok(self.position)
    }
}
fn read_package(card: &Path, model: l6fw::Model) -> io::Result<Vec<u8>> {
    let mut file = File::open(card)?;
    let size = file.metadata()?.len();
    let mut sector = [0u8; 512];
    file.read_exact(&mut sector)?;
    if sector[510..] != [0x55, 0xaa] {
        return Err(invalid("invalid SD boot sector"));
    }
    let (start, length) = if sector[82..90] == *b"FAT32   "
        || sector[54..62] == *b"FAT16   "
        || sector[54..62] == *b"FAT12   "
    {
        (0, size)
    } else {
        let partition = (0..4)
            .map(|i| &sector[446 + i * 16..462 + i * 16])
            .find(|p| matches!(p[4], 1 | 4 | 6 | 0xb | 0xc | 0xe))
            .ok_or_else(|| invalid("no supported FAT partition"))?;
        let start = u64::from(le32(partition, 8)) * 512;
        let length = u64::from(le32(partition, 12)) * 512;
        if length == 0 || start + length > size {
            return Err(invalid("invalid MBR partition bounds"));
        }
        (start, length)
    };
    let volume = fatfs::FileSystem::new(
        Volume {
            file,
            start,
            length,
            position: 0,
        },
        fatfs::FsOptions::new(),
    )?;
    let mut package = Vec::new();
    let limit = match model {
        l6fw::Model::L6 => MAX_PACKAGE_SIZE,
        l6fw::Model::L6max => PACKAGE_SIZE,
    };
    volume
        .root_dir()
        .open_file(match model {
            l6fw::Model::L6 => "L6.BIN",
            l6fw::Model::L6max => "L6max.BIN",
        })?
        .take((limit + 1) as u64)
        .read_to_end(&mut package)?;
    Ok(package)
}

fn package_segment<'a>(package: &'a [u8], model: l6fw::Model, name: &str) -> io::Result<&'a [u8]> {
    let main_length = le32(package, package.len() - 8) as usize;
    let (start, end) = l6fw::layout_for(model, main_length)
        .into_iter()
        .find_map(|(segment, start, end)| (segment == name).then_some((start, end)))
        .ok_or_else(|| invalid("required L6 update segment is missing"))?;
    package
        .get(start..end)
        .ok_or_else(|| invalid("L6 update segment is outside the package"))
}

fn install_l6_package(package: &[u8], nor: &mut [u8]) -> io::Result<Vec<u8>> {
    if l6fw::model_from_package(package).map_err(|_| invalid("invalid L6 update package"))?
        != l6fw::Model::L6
    {
        return Err(invalid("update package model does not match installed L6"));
    }
    let main = package_segment(package, l6fw::Model::L6, "main_firmware.bin")?;
    let main_trailer = package_segment(package, l6fw::Model::L6, "main_trailer.bin")?;
    let panel = package_segment(package, l6fw::Model::L6, "secondary_firmware.bin")?;
    let panel_trailer = package_segment(package, l6fw::Model::L6, "secondary_trailer.bin")?;
    let package_footer = package_segment(package, l6fw::Model::L6, "package_footer.bin")?;
    let resource = package_segment(package, l6fw::Model::L6, "resource_region.bin")?;
    if main.len() > 0x1a2ff8
        || main_trailer.len() != 8
        || panel.len() > 0x3ff8
        || panel_trailer.len() != 8
        || package_footer.len() != 0x40
        || resource.len() > L6_RESOURCE_SIZE
    {
        return Err(invalid(
            "L6 update image exceeds its installed flash region",
        ));
    }

    // L6's update package is not a linear image of its 2 MiB MAIN NOR:
    // the package's resource region belongs to a separate 4 MiB serial NOR.
    nor[0x50000..0x1f2ff8].fill(0xff);
    nor[0x50000..0x50000 + main.len()].copy_from_slice(main);
    nor[0x1f2ff8..0x1f3000].copy_from_slice(main_trailer);
    nor[0x1f3000..0x1f6ff8].fill(0xff);
    nor[0x1f3000..0x1f3000 + panel.len()].copy_from_slice(panel);
    nor[0x1f6ff8..0x1f7000].copy_from_slice(panel_trailer);
    nor[0x1f7fc0..0x1f8000].copy_from_slice(package_footer);

    let mut resource_flash = vec![0xff; L6_RESOURCE_SIZE];
    resource_flash[..resource.len()].copy_from_slice(resource);
    Ok(resource_flash)
}
/// Existing NOR is exclusively locked; QEMU's storage lock prevents running
/// this installer against a live guest. No update is attempted without a marker.
pub fn install(state: &Path, card: Option<&Path>, model: l6fw::Model) -> io::Result<Outcome> {
    let path = state.join("main-nor.bin");
    let mut file = match OpenOptions::new().read(true).write(true).open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Outcome::NoPending),
        Err(e) => return Err(e),
    };
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if file.metadata()?.len() != NOR_SIZE as u64 {
        return Err(invalid("installed NOR must be exactly 2 MiB"));
    }
    let mut nor = Vec::new();
    file.read_to_end(&mut nor)?;
    if &nor[MARKER..MARKER + 8] != b"FW UPDT\0" {
        return Ok(Outcome::NoPending);
    }
    if &nor[MARKER + 8..MARKER + 20] == b"FW UPDT ERR\0" {
        return Ok(Outcome::PreviousFailure);
    }
    let mut resource_update = None;
    let result = card
        .ok_or_else(|| invalid("update requested without SD media"))
        .and_then(|card| read_package(card, model))
        .and_then(|package| {
            match model {
                l6fw::Model::L6 => {
                    resource_update = Some(install_l6_package(&package, &mut nor)?);
                }
                l6fw::Model::L6max => {
                    if l6fw::model_from_package(&package)
                        .map_err(|_| invalid("invalid L6max update package"))?
                        != l6fw::Model::L6max
                    {
                        return Err(invalid(
                            "update package model does not match installed L6max",
                        ));
                    }
                    let payload = validate(&package)?;
                    // The L6max MAIN section maps contiguously to this NOR range.
                    nor[0x50000..0x1f8000].copy_from_slice(payload);
                }
            }
            Ok(())
        });
    let outcome = match result {
        Ok(()) => Outcome::Installed,
        Err(e) => {
            nor[MARKER + 8..MARKER + 20].copy_from_slice(b"FW UPDT ERR\0");
            Outcome::Failed(e.to_string())
        }
    };
    if outcome == Outcome::Installed {
        if let Some(resource) = resource_update {
            let path = state.join("resource-nor.bin");
            let mut resource_file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(path)?;
            resource_file.write_all(&resource)?;
            resource_file.sync_all()?;

            // L6 application firmware does not consume the L6max update
            // record. The replacement installer owns this L6-only marker.
            nor[MARKER..MARKER + 8].fill(0xff);
            nor[MARKER + 8..MARKER + 20].fill(0xff);
        }
    }
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&nor)?;
    file.sync_all()?;
    Ok(outcome)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn package() -> Vec<u8> {
        let mut p = vec![0; PACKAGE_SIZE];
        p[..17].copy_from_slice(b"L6max System Data");
        p[0x60] = 2;
        p[0x64] = 0x40;
        p[0x68..0x6c].copy_from_slice(b"BOOT");
        p[0x74..0x78].copy_from_slice(b"MAIN");
        p[0x78..0x7c].copy_from_slice(&0x100u32.to_le_bytes());
        p[0x7c..0x80].copy_from_slice(&0x1a8100u32.to_le_bytes());
        p[0x100..0x10f].copy_from_slice(b"L6max Main Data");
        p[0x200..0x204].copy_from_slice(&0x20002000u32.to_le_bytes());
        p[0x204..0x208].copy_from_slice(&0x80000415u32.to_le_bytes());
        p[0x1a31f8..0x1a31fc].copy_from_slice(&0x1000u32.to_le_bytes());
        p[0x1a3200..0x1a3204].copy_from_slice(&0x20002000u32.to_le_bytes());
        p[0x1a3204..0x1a3208].copy_from_slice(&0x08000009u32.to_le_bytes());
        p[0x1a71f8..0x1a71fc].copy_from_slice(&0x80u32.to_le_bytes());
        p[0x1a71fc..0x1a7200].copy_from_slice(b"0101");
        p[0x1a31fc..0x1a3200].copy_from_slice(b"0111");
        p[0x1a81fc..].copy_from_slice(b"0111");
        p[0x1a81f8..0x1a81fc].copy_from_slice(&0x1000u32.to_le_bytes());
        checksum(&mut p);
        p
    }

    fn l6_package() -> Vec<u8> {
        const SIZE: usize = 0x395200;
        const MAIN_TRAILER: usize = 0x2851f8;
        const PANEL_START: usize = 0x285200;
        const PANEL_TRAILER: usize = 0x2951f8;
        let mut p = vec![0xff; SIZE];
        p[..14].copy_from_slice(b"L6 System Data");
        p[0x100..0x10c].copy_from_slice(b"L6 Main Data");
        for (offset, value) in [
            (0x60, 2u32),
            (0x64, 1),
            (0x6c, 0),
            (0x70, 0),
            (0x78, 0x100),
            (0x7c, (SIZE - 0x100) as u32),
            (0x200, 0x20002000),
            (0x204, 0x80001009),
            (MAIN_TRAILER, 0x100),
            (PANEL_START, 0x20002000),
            (PANEL_START + 4, 0x08000009),
            (PANEL_TRAILER, 0x80),
            (SIZE - 8, 0x100),
        ] {
            p[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        p[0x68..0x6c].copy_from_slice(b"BOOT");
        p[0x74..0x78].copy_from_slice(b"MAIN");
        p[MAIN_TRAILER + 4..MAIN_TRAILER + 8].copy_from_slice(b"0111");
        p[PANEL_TRAILER + 4..PANEL_TRAILER + 8].copy_from_slice(b"0101");
        p[SIZE - 4..].copy_from_slice(b"0111");
        p[0x208..0x210].copy_from_slice(b"L6MAIN!!");
        p[PANEL_START + 8..PANEL_START + 16].copy_from_slice(b"L6PANEL!");
        p[PANEL_TRAILER + 8..PANEL_TRAILER + 16].copy_from_slice(b"L6RESOUR");
        let sum = p[0x200..]
            .iter()
            .fold(0u32, |sum, &byte| sum.wrapping_add(u32::from(byte)));
        p[0x1fc..0x200].copy_from_slice(&sum.to_be_bytes());
        assert_eq!(l6fw::model_from_package(&p).unwrap(), l6fw::Model::L6);
        p
    }
    fn checksum(p: &mut [u8]) {
        let sum = p[0x200..]
            .iter()
            .fold(0u32, |s, &b| s.wrapping_add(b as u32));
        p[0x1fc..0x200].copy_from_slice(&sum.to_be_bytes());
    }
    #[test]
    fn checksum_and_layout_reject_corruption() {
        let mut p = package();
        assert!(validate(&p).is_ok());
        p[0x200] ^= 1;
        assert!(validate(&p).is_err());
        checksum(&mut p);
        assert!(validate(&p).is_ok());
        p[0x7f] = 0xff;
        assert!(validate(&p).is_err());
        assert!(validate(&p[..100]).is_err());
    }
    #[test]
    fn protected_nor_and_error_handoff() -> io::Result<()> {
        let directory = std::env::temp_dir().join(format!("l6-update-unit-{}", std::process::id()));
        fs::create_dir(&directory)?;
        let result = (|| {
            let mut nor = vec![0x5a; NOR_SIZE];
            nor[MARKER..].fill(0xff);
            nor[MARKER..MARKER + 8].copy_from_slice(b"FW UPDT\0");
            let path = directory.join("main-nor.bin");
            fs::write(&path, &nor)?;
            assert!(matches!(
                install(&directory, None, l6fw::Model::L6max)?,
                Outcome::Failed(_)
            ));
            let failed = fs::read(&path)?;
            assert_eq!(&failed[..MARKER + 8], &nor[..MARKER + 8]);
            assert_eq!(&failed[MARKER + 8..MARKER + 20], b"FW UPDT ERR\0");
            assert_eq!(
                install(&directory, None, l6fw::Model::L6max)?,
                Outcome::PreviousFailure
            );
            let locked = OpenOptions::new().read(true).write(true).open(&path)?;
            assert_eq!(
                unsafe { libc::flock(locked.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
                0
            );
            assert!(install(&directory, None, l6fw::Model::L6max).is_err());
            Ok(())
        })();
        fs::remove_dir_all(directory)?;
        result
    }
    #[test]
    fn install_preserves_bootloader_settings_and_sd() -> io::Result<()> {
        let directory =
            std::env::temp_dir().join(format!("l6-update-success-{}", std::process::id()));
        fs::create_dir(&directory)?;
        let result = (|| {
            let p = package();
            let card = directory.join("card.img");
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&card)?;
            file.set_len(64 * 1024 * 1024)?;
            fatfs::format_volume(
                &mut file,
                fatfs::FormatVolumeOptions::new().fat_type(fatfs::FatType::Fat32),
            )?;
            let volume = fatfs::FileSystem::new(&mut file, fatfs::FsOptions::new())?;
            volume.root_dir().create_file("L6max.BIN")?.write_all(&p)?;
            volume.unmount()?;
            drop(file);
            let card_before = fs::read(&card)?;
            let mut nor = vec![0x5a; NOR_SIZE];
            nor[MARKER..].fill(0xff);
            let path = directory.join("main-nor.bin");
            fs::write(&path, &nor)?;
            assert_eq!(
                install(&directory, Some(&card), l6fw::Model::L6max)?,
                Outcome::NoPending
            );
            assert_eq!(fs::read(&path)?, nor);
            nor[MARKER..MARKER + 8].copy_from_slice(b"FW UPDT\0");
            fs::write(&path, &nor)?;
            assert_eq!(
                install(&directory, Some(&card), l6fw::Model::L6max)?,
                Outcome::Installed
            );
            let after = fs::read(&path)?;
            assert_eq!(&after[..0x50000], &nor[..0x50000]);
            assert_eq!(&after[0x50000..0x1f8000], &p[0x200..]);
            assert_eq!(&after[0x1f8000..], &nor[0x1f8000..]);
            assert_eq!(fs::read(&card)?, card_before);
            Ok(())
        })();
        fs::remove_dir_all(directory)?;
        result
    }

    #[test]
    fn l6_install_splits_main_panel_and_resource_flash_images() -> io::Result<()> {
        let package = l6_package();
        let directory =
            std::env::temp_dir().join(format!("l6-update-image-{}", std::process::id()));
        fs::create_dir(&directory)?;
        let result = (|| {
            let mut nor = vec![0x5a; NOR_SIZE];
            let original_boot = nor[..0x50000].to_vec();
            let original_calibration = nor[0x1f8000..].to_vec();
            let resource = install_l6_package(&package, &mut nor)?;
            assert_eq!(&nor[..0x50000], original_boot);
            assert_eq!(&nor[0x50000..0x50008], b"\x00\x20\x00\x20\x09\x10\x00\x80");
            assert_eq!(&nor[0x50008..0x50010], b"L6MAIN!!");
            assert!(nor[0x50100..0x1f2ff8].iter().all(|byte| *byte == 0xff));
            assert_eq!(&nor[0x1f2ffc..0x1f3000], b"0111");
            assert_eq!(&nor[0x1f3008..0x1f3010], b"L6PANEL!");
            assert_eq!(&nor[0x1f6ffc..0x1f7000], b"0101");
            assert_eq!(&nor[0x1f7ffc..0x1f8000], b"0111");
            assert_eq!(&nor[0x1f8000..], original_calibration);
            assert_eq!(&resource[..8], b"L6RESOUR");
            assert!(resource[0x100fc0..].iter().all(|byte| *byte == 0xff));
            fs::write(directory.join("resource-nor.bin"), &resource)?;
            assert_eq!(
                fs::metadata(directory.join("resource-nor.bin"))?.len(),
                0x400000
            );
            Ok(())
        })();
        fs::remove_dir_all(directory)?;
        result
    }

    #[test]
    fn l6_installer_reads_the_model_matched_card_file() -> io::Result<()> {
        let directory = std::env::temp_dir().join(format!("l6-update-card-{}", std::process::id()));
        fs::create_dir(&directory)?;
        let result = (|| {
            let package = l6_package();
            let card = directory.join("card.img");
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&card)?;
            file.set_len(64 * 1024 * 1024)?;
            fatfs::format_volume(
                &mut file,
                fatfs::FormatVolumeOptions::new().fat_type(fatfs::FatType::Fat32),
            )?;
            let volume = fatfs::FileSystem::new(&mut file, fatfs::FsOptions::new())?;
            volume
                .root_dir()
                .create_file("L6.BIN")?
                .write_all(&package)?;
            volume.unmount()?;
            drop(file);

            let mut nor = vec![0x5a; NOR_SIZE];
            nor[MARKER..].fill(0xff);
            nor[MARKER..MARKER + 8].copy_from_slice(b"FW UPDT\0");
            fs::write(directory.join("main-nor.bin"), &nor)?;
            assert_eq!(
                install(&directory, Some(&card), l6fw::Model::L6)?,
                Outcome::Installed
            );
            let installed = fs::read(directory.join("main-nor.bin"))?;
            assert_eq!(&installed[0x50008..0x50010], b"L6MAIN!!");
            assert_eq!(&installed[0x1f3008..0x1f3010], b"L6PANEL!");
            assert!(
                installed[MARKER..MARKER + 8]
                    .iter()
                    .all(|byte| *byte == 0xff)
            );
            let resource = fs::read(directory.join("resource-nor.bin"))?;
            assert_eq!(resource.len(), L6_RESOURCE_SIZE);
            assert_eq!(&resource[..8], b"L6RESOUR");
            assert_eq!(fs::metadata(&card)?.len(), 64 * 1024 * 1024);
            Ok(())
        })();
        fs::remove_dir_all(directory)?;
        result
    }
}
