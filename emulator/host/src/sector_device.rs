//! Sector-addressed storage behind one interface, and a seekable FAT volume view.
//!
//! The emulated SD card can be reached in different ways: through the firmware's
//! USB mass-storage interface ([`crate::usb_storage::UsbSectors`]), or directly as
//! an image file ([`StreamSectors`]). Both present 512-byte sectors, so the FAT
//! access in `usb-files` and tests is written once against [`SectorDevice`]. A
//! future host mount (FUSE on Linux, a disk-image service on macOS) would be one
//! more implementation.
use std::io::{self, Read, Seek, SeekFrom, Write};

pub const SECTOR: usize = 512;

pub trait SectorDevice {
    /// Number of 512-byte sectors.
    fn sector_count(&self) -> u64;
    fn read_sector(&mut self, lba: u32) -> io::Result<[u8; SECTOR]>;
    fn write_sector(&mut self, lba: u32, bytes: &[u8; SECTOR]) -> io::Result<()>;
    /// Make completed writes durable on the underlying device.
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Sectors stored in any seekable byte stream: an image [`std::fs::File`], or an
/// in-memory buffer in tests.
pub struct StreamSectors<T> {
    stream: T,
    sectors: u64,
}
impl<T: Read + Write + Seek> StreamSectors<T> {
    pub fn new(mut stream: T) -> io::Result<Self> {
        let length = stream.seek(SeekFrom::End(0))?;
        Ok(Self {
            stream,
            sectors: length / SECTOR as u64,
        })
    }
}
impl<T: Read + Write + Seek> SectorDevice for StreamSectors<T> {
    fn sector_count(&self) -> u64 {
        self.sectors
    }
    fn read_sector(&mut self, lba: u32) -> io::Result<[u8; SECTOR]> {
        if u64::from(lba) >= self.sectors {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "sector outside device",
            ));
        }
        let mut bytes = [0; SECTOR];
        self.stream
            .seek(SeekFrom::Start(u64::from(lba) * SECTOR as u64))?;
        self.stream.read_exact(&mut bytes)?;
        Ok(bytes)
    }
    fn write_sector(&mut self, lba: u32, bytes: &[u8; SECTOR]) -> io::Result<()> {
        if u64::from(lba) >= self.sectors {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "sector outside device",
            ));
        }
        self.stream
            .seek(SeekFrom::Start(u64::from(lba) * SECTOR as u64))?;
        self.stream.write_all(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

/// Seekable view of the first FAT volume on a [`SectorDevice`], for host FAT
/// access. One sector is cached; a dirty sector is written on eviction and on
/// [`Write::flush`].
pub struct SectorDisk<D: SectorDevice> {
    device: D,
    position: u64,
    length: u64,
    first: u32,
    cached: Option<(u32, [u8; SECTOR])>,
    dirty: bool,
}
impl<D: SectorDevice> SectorDisk<D> {
    /// A volume spans the device when sector 0 is a FAT boot sector; otherwise
    /// the first FAT partition of an MBR is used.
    pub fn open(mut device: D) -> io::Result<Self> {
        let blocks = device.sector_count();
        let boot = device.read_sector(0)?;
        let mut first = 0;
        let mut volume_blocks = blocks;
        if u16::from_le_bytes([boot[11], boot[12]]) != SECTOR as u16 {
            if boot[510..] != [0x55, 0xaa] {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "missing disk signature",
                ));
            }
            let p = boot[446..510]
                .chunks_exact(16)
                .find(|p| matches!(p[4], 1 | 4 | 6 | 0xb | 0xc | 0xe))
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no FAT partition"))?;
            first = u32::from_le_bytes(p[8..12].try_into().unwrap());
            volume_blocks = u32::from_le_bytes(p[12..16].try_into().unwrap()) as u64;
            if volume_blocks == 0 || first as u64 + volume_blocks > blocks {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "FAT partition exceeds the disk",
                ));
            }
        }
        Ok(Self {
            device,
            position: 0,
            length: volume_blocks * SECTOR as u64,
            first,
            cached: None,
            dirty: false,
        })
    }
    fn flush_sector(&mut self) -> io::Result<()> {
        if self.dirty {
            let (lba, bytes) = self.cached.as_ref().unwrap();
            self.device.write_sector(*lba, bytes)?;
            self.dirty = false;
        }
        Ok(())
    }
    fn sector(&mut self, lba: u32) -> io::Result<&mut [u8; SECTOR]> {
        if self.cached.as_ref().is_none_or(|(n, _)| *n != lba) {
            self.flush_sector()?;
            self.cached = Some((lba, self.device.read_sector(lba)?));
        }
        Ok(&mut self.cached.as_mut().unwrap().1)
    }
    /// Bytes available to the volume.
    pub fn len(&self) -> u64 {
        self.length
    }
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }
}
impl<D: SectorDevice> Read for SectorDisk<D> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let offset = (self.position % SECTOR as u64) as usize;
        let count = bytes
            .len()
            .min(SECTOR - offset)
            .min(self.length.saturating_sub(self.position) as usize);
        if count == 0 {
            return Ok(0);
        }
        let lba = self.first + (self.position / SECTOR as u64) as u32;
        bytes[..count].copy_from_slice(&self.sector(lba)?[offset..offset + count]);
        self.position += count as u64;
        Ok(count)
    }
}
impl<D: SectorDevice> Write for SectorDisk<D> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let offset = (self.position % SECTOR as u64) as usize;
        let count = bytes
            .len()
            .min(SECTOR - offset)
            .min(self.length.saturating_sub(self.position) as usize);
        if count == 0 {
            return Ok(0);
        }
        let lba = self.first + (self.position / SECTOR as u64) as u32;
        self.sector(lba)?[offset..offset + count].copy_from_slice(&bytes[..count]);
        self.dirty = true;
        self.position += count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.flush_sector()?;
        self.device.flush()
    }
}
impl<D: SectorDevice> Seek for SectorDisk<D> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let target = match to {
            SeekFrom::Start(n) => n as i128,
            SeekFrom::Current(n) => self.position as i128 + n as i128,
            SeekFrom::End(n) => self.length as i128 + n as i128,
        };
        if target < 0 || target > self.length as i128 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek outside the volume",
            ));
        }
        self.position = target as u64;
        Ok(self.position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A formatted FAT volume of `sectors` sectors.
    fn volume(sectors: usize) -> Vec<u8> {
        let mut bytes = Cursor::new(vec![0; sectors * SECTOR]);
        fatfs::format_volume(&mut bytes, fatfs::FormatVolumeOptions::new()).unwrap();
        bytes.into_inner()
    }
    /// One MBR with a single FAT32 (LBA) partition at sector 2048.
    fn partitioned(volume: &[u8]) -> Vec<u8> {
        let first = 2048usize;
        let mut disk = vec![0; (first + volume.len() / SECTOR) * SECTOR];
        disk[446 + 4] = 0x0c;
        disk[446 + 8..446 + 12].copy_from_slice(&(first as u32).to_le_bytes());
        disk[446 + 12..446 + 16].copy_from_slice(&((volume.len() / SECTOR) as u32).to_le_bytes());
        disk[510..512].copy_from_slice(&[0x55, 0xaa]);
        disk[first * SECTOR..].copy_from_slice(volume);
        disk
    }
    fn round_trip(image: Vec<u8>) {
        let mut device = StreamSectors::new(Cursor::new(image)).unwrap();
        {
            let mut disk = SectorDisk::open(&mut device).unwrap();
            let fs = fatfs::FileSystem::new(&mut disk, fatfs::FsOptions::new()).unwrap();
            let mut file = fs.root_dir().create_file("TRACK.WAV").unwrap();
            // Crosses several sectors and a cluster boundary.
            file.write_all(&(0..3000u32).map(|i| i as u8).collect::<Vec<_>>())
                .unwrap();
            file.flush().unwrap();
            drop(file);
            fs.unmount().unwrap();
            disk.flush().unwrap();
        }
        let mut disk = SectorDisk::open(&mut device).unwrap();
        let fs = fatfs::FileSystem::new(&mut disk, fatfs::FsOptions::new()).unwrap();
        let mut content = Vec::new();
        fs.root_dir()
            .open_file("TRACK.WAV")
            .unwrap()
            .read_to_end(&mut content)
            .unwrap();
        assert_eq!(content.len(), 3000);
        assert!(content.iter().enumerate().all(|(i, &b)| b == i as u8));
    }
    impl<D: SectorDevice> SectorDevice for &mut D {
        fn sector_count(&self) -> u64 {
            (**self).sector_count()
        }
        fn read_sector(&mut self, lba: u32) -> io::Result<[u8; SECTOR]> {
            (**self).read_sector(lba)
        }
        fn write_sector(&mut self, lba: u32, bytes: &[u8; SECTOR]) -> io::Result<()> {
            (**self).write_sector(lba, bytes)
        }
        fn flush(&mut self) -> io::Result<()> {
            (**self).flush()
        }
    }
    #[test]
    fn fat_files_round_trip_on_a_whole_disk_volume() {
        round_trip(volume(16384));
    }
    #[test]
    fn fat_files_round_trip_on_the_first_mbr_partition() {
        round_trip(partitioned(&volume(16384)));
    }
    #[test]
    fn partition_beyond_the_disk_is_rejected() {
        let mut image = partitioned(&volume(16384));
        image[446 + 12..446 + 16].copy_from_slice(&u32::MAX.to_le_bytes());
        let device = StreamSectors::new(Cursor::new(image)).unwrap();
        let error = SectorDisk::open(device).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
    #[test]
    fn disk_without_signature_or_fat_is_rejected() {
        let device = StreamSectors::new(Cursor::new(vec![0; 64 * SECTOR])).unwrap();
        assert!(SectorDisk::open(device).is_err());
    }
    #[test]
    fn sectors_outside_the_device_are_errors() {
        let mut device = StreamSectors::new(Cursor::new(vec![0; 4 * SECTOR])).unwrap();
        assert_eq!(device.sector_count(), 4);
        assert!(device.read_sector(4).is_err());
        assert!(device.write_sector(4, &[0; SECTOR]).is_err());
    }
    #[test]
    fn seeking_past_the_volume_is_an_error() {
        let device = StreamSectors::new(Cursor::new(volume(16384))).unwrap();
        let mut disk = SectorDisk::open(device).unwrap();
        assert!(disk.seek(SeekFrom::Start(disk.len() + 1)).is_err());
        assert_eq!(disk.seek(SeekFrom::End(0)).unwrap(), disk.len());
    }
}
