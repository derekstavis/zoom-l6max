//! USB Bulk-Only Transport. SCSI requests are executed by firmware, including SD DMA.
use crate::{
    sector_device::{SECTOR, SectorDevice, SectorDisk},
    usb::{Endpoints, UsbHost},
};
use std::io;

pub struct MassStorage<'a> {
    usb: &'a UsbHost,
    endpoints: Endpoints,
    tag: u32,
}
impl<'a> MassStorage<'a> {
    pub fn new(usb: &'a UsbHost, endpoints: Endpoints) -> Self {
        Self {
            usb,
            endpoints,
            tag: 0,
        }
    }
    fn command(&mut self, cdb: &[u8], length: usize, output: Option<&[u8]>) -> io::Result<Vec<u8>> {
        if cdb.is_empty()
            || cdb.len() > 16
            || length > 16384
            || output.is_some_and(|d| d.len() != length)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid SCSI transfer",
            ));
        }
        self.tag = self.tag.wrapping_add(1).max(1);
        let mut cbw = [0u8; 31];
        cbw[..4].copy_from_slice(b"USBC");
        cbw[4..8].copy_from_slice(&self.tag.to_le_bytes());
        cbw[8..12].copy_from_slice(&(length as u32).to_le_bytes());
        cbw[12] = if output.is_none() { 0x80 } else { 0 };
        cbw[14] = cdb.len() as u8;
        cbw[15..15 + cdb.len()].copy_from_slice(cdb);
        self.usb.send(self.endpoints.output, &cbw)?;
        let mut input = Vec::new();
        if length != 0 {
            if let Some(data) = output {
                self.usb.send(self.endpoints.output, data)?;
            } else {
                while input.len() < length {
                    let data = self.usb.receive(self.endpoints.input, 16384)?;
                    if data.is_empty() {
                        break;
                    }
                    if input.len() + data.len() > length {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "SCSI data exceeds expected transfer",
                        ));
                    }
                    input.extend(data);
                }
            }
        }
        let csw = self.usb.receive(self.endpoints.input, 16384)?;
        if csw.len() != 13
            || &csw[..4] != b"USBS"
            || u32::from_le_bytes(csw[4..8].try_into().unwrap()) != self.tag
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid SCSI command status {csw:02x?}"),
            ));
        }
        let residue = u32::from_le_bytes(csw[8..12].try_into().unwrap()) as usize;
        if csw[12] != 0 {
            return Err(io::Error::other(format!(
                "SCSI opcode {:02x} failed, status {}, residue {residue}",
                cdb[0], csw[12]
            )));
        }
        if residue != 0 || (output.is_none() && input.len() != length) {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "short SCSI transfer",
            ));
        }
        Ok(input)
    }
    pub fn inquiry(&mut self) -> io::Result<Vec<u8>> {
        self.command(&[0x12, 0, 0, 0, 36, 0], 36, None)
    }
    pub fn request_sense(&mut self) -> io::Result<Vec<u8>> {
        self.command(&[3, 0, 0, 0, 18, 0], 18, None)
    }
    pub fn ready(&mut self) -> io::Result<()> {
        for _ in 0..40 {
            match self.command(&[0, 0, 0, 0, 0, 0], 0, None) {
                Ok(_) => return Ok(()),
                Err(e) if e.kind() == io::ErrorKind::Other => {
                    let sense = self.request_sense()?;
                    let key = sense[2] & 15;
                    if key != 6 && !(key == 2 && sense[12] == 4) {
                        return Err(io::Error::other(format!(
                            "SCSI not ready: sense key {key:x}, ASC {:02x}, ASCQ {:02x}",
                            sense[12], sense[13]
                        )));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "SCSI readiness deadline",
        ))
    }
    pub fn capacity(&mut self) -> io::Result<(u64, u32)> {
        let data = self.command(&[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0], 8, None)?;
        Ok((
            u32::from_be_bytes(data[..4].try_into().unwrap()) as u64 + 1,
            u32::from_be_bytes(data[4..8].try_into().unwrap()),
        ))
    }
    pub fn read_sector(&mut self, lba: u32) -> io::Result<[u8; 512]> {
        let mut cdb = [0u8; 10];
        cdb[0] = 0x28;
        cdb[2..6].copy_from_slice(&lba.to_be_bytes());
        cdb[8] = 1;
        self.command(&cdb, 512, None)?
            .try_into()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "sector length"))
    }
    pub fn write_sector(&mut self, lba: u32, bytes: &[u8; 512]) -> io::Result<()> {
        let mut cdb = [0u8; 10];
        cdb[0] = 0x2a;
        cdb[2..6].copy_from_slice(&lba.to_be_bytes());
        cdb[8] = 1;
        self.command(&cdb, 512, Some(bytes)).map(|_| ())
    }
    pub fn synchronize(&mut self) -> io::Result<()> {
        self.command(&[0x35, 0, 0, 0, 0, 0, 0, 0, 0, 0], 0, None)
            .map(|_| ())
    }
}

/// A USB mass-storage device as a [`SectorDevice`]. The firmware owns every
/// read and write; this only issues SCSI requests.
pub struct UsbSectors<'a> {
    disk: MassStorage<'a>,
    blocks: u64,
}
impl<'a> UsbSectors<'a> {
    pub fn open(mut disk: MassStorage<'a>) -> io::Result<Self> {
        disk.ready()?;
        let (blocks, size) = disk.capacity()?;
        if size as usize != SECTOR || blocks > u32::MAX as u64 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "USB FAT access requires 512-byte sectors and READ(10) addressing",
            ));
        }
        Ok(Self { disk, blocks })
    }
}
impl SectorDevice for UsbSectors<'_> {
    fn sector_count(&self) -> u64 {
        self.blocks
    }
    fn read_sector(&mut self, lba: u32) -> io::Result<[u8; SECTOR]> {
        self.disk.read_sector(lba)
    }
    fn write_sector(&mut self, lba: u32, bytes: &[u8; SECTOR]) -> io::Result<()> {
        self.disk.write_sector(lba, bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.disk.synchronize()
    }
}

/// Seekable FAT volume over the emulated USB disk.
pub type UsbDisk<'a> = SectorDisk<UsbSectors<'a>>;
impl<'a> SectorDisk<UsbSectors<'a>> {
    pub fn new(disk: MassStorage<'a>) -> io::Result<Self> {
        Self::open(UsbSectors::open(disk)?)
    }
}
