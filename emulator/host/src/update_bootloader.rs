//! Opt-in replacement for the unavailable ZOOM SD installer, not its ROM code.
//! Runs before QEMU owns storage. Only the observed MAIN-only layout is supported.
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::fd::AsRawFd,
    path::Path,
};
const NOR_SIZE: usize = 0x200000;
const MARKER: usize = 0x1ff000;
const PACKAGE_SIZE: usize = 0x1a8200;
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
fn read_package(card: &Path) -> io::Result<Vec<u8>> {
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
    volume
        .root_dir()
        .open_file("L6max.BIN")?
        .take((PACKAGE_SIZE + 1) as u64)
        .read_to_end(&mut package)?;
    Ok(package)
}
/// Existing NOR is exclusively locked; QEMU's storage lock prevents running
/// this installer against a live guest. No update is attempted without a marker.
pub fn install(state: &Path, card: Option<&Path>) -> io::Result<Outcome> {
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
    let result = card
        .ok_or_else(|| invalid("update requested without SD media"))
        .and_then(read_package)
        .and_then(|package| {
            let payload = validate(&package)?;
            // Entire MAIN section payload maps to [0x50000, 0x1f8000).
            // Includes the packaged panel and boot-data footer, not calibration.
            nor[0x50000..0x1f8000].copy_from_slice(payload);
            Ok(())
        });
    let outcome = match result {
        Ok(()) => Outcome::Installed,
        Err(e) => {
            nor[MARKER + 8..MARKER + 20].copy_from_slice(b"FW UPDT ERR\0");
            Outcome::Failed(e.to_string())
        }
    };
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
        p[0x60] = 2;
        p[0x64] = 0x40;
        p[0x78..0x7c].copy_from_slice(&0x100u32.to_le_bytes());
        p[0x7c..0x80].copy_from_slice(&0x1a8100u32.to_le_bytes());
        p[0x100..0x10f].copy_from_slice(b"L6max Main Data");
        p[0x1a31fc..0x1a3200].copy_from_slice(b"0111");
        p[0x1a81fc..].copy_from_slice(b"0111");
        checksum(&mut p);
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
            assert!(matches!(install(&directory, None)?, Outcome::Failed(_)));
            let failed = fs::read(&path)?;
            assert_eq!(&failed[..MARKER + 8], &nor[..MARKER + 8]);
            assert_eq!(&failed[MARKER + 8..MARKER + 20], b"FW UPDT ERR\0");
            assert_eq!(install(&directory, None)?, Outcome::PreviousFailure);
            let locked = OpenOptions::new().read(true).write(true).open(&path)?;
            assert_eq!(
                unsafe { libc::flock(locked.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
                0
            );
            assert!(install(&directory, None).is_err());
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
            assert_eq!(install(&directory, Some(&card))?, Outcome::NoPending);
            assert_eq!(fs::read(&path)?, nor);
            nor[MARKER..MARKER + 8].copy_from_slice(b"FW UPDT\0");
            fs::write(&path, &nor)?;
            assert_eq!(install(&directory, Some(&card))?, Outcome::Installed);
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
}
