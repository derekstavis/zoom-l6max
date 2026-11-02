//! Create and access FAT and exFAT SD card images; callers choose whether to enable them.
pub mod pad_settings;
pub mod spec;

use fstool::{
    block::{file::FileBackend, memory::MemoryBackend, sliced::SlicedBackend},
    device::SectorDriver,
    fs::{
        EntryKind, FileMeta, Filesystem,
        exfat::{Exfat, FormatOpts},
        fat::{self, Fat32, FatFormatOpts, FatKind},
    },
};
use std::{
    fs::{self, OpenOptions},
    io::{self, Read},
    path::Path,
};

pub use fstool::{Error as DeviceError, block::BlockDevice};

/// Every generated card has this size. Images are sparse.
pub const CARD_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// The firmware rejects FAT32 cards with smaller clusters when recording.
const CLUSTER_BYTES: u32 = 32 * 1024;

fn device_error(error: DeviceError) -> io::Error {
    io::Error::other(error)
}

fn absolute(path: &str) -> String {
    format!("/{}", path.trim_start_matches('/'))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub directory: bool,
    pub length: u64,
}

/// Lets the sector-based FAT formatter, which can set the cluster size, write
/// to a block device.
struct Sectors<'a, D>(&'a mut D);

impl<D: BlockDevice> SectorDriver for Sectors<'_, D> {
    type Error = DeviceError;
    fn sector_size(&self) -> u32 {
        512
    }
    fn sector_count(&self) -> u64 {
        self.0.total_size() / 512
    }
    fn read_sectors(&mut self, lba: u64, bytes: &mut [u8]) -> Result<(), DeviceError> {
        self.0.read_at(lba * 512, bytes)
    }
    fn write_sectors(&mut self, lba: u64, bytes: &[u8]) -> Result<(), DeviceError> {
        self.0.write_at(lba * 512, bytes)
    }
}

/// A FAT or exFAT volume on a block device. Call [`Volume::flush`] to publish changes.
pub struct Volume<D: BlockDevice> {
    device: D,
    filesystem: Box<dyn Filesystem + Send>,
}

impl<D: BlockDevice> Volume<D> {
    pub fn open(mut device: D) -> io::Result<Self> {
        let mut name = [0; 8];
        device.read_at(3, &mut name).map_err(device_error)?;
        let filesystem: Box<dyn Filesystem + Send> = if name == *b"EXFAT   " {
            Box::new(Exfat::open(&mut device).map_err(device_error)?)
        } else {
            Box::new(Fat32::open(&mut device).map_err(device_error)?)
        };
        Ok(Self { device, filesystem })
    }
    /// Format the whole device as FAT with the card cluster size.
    pub fn format(mut device: D, label: &str) -> io::Result<Self> {
        let mut options = fat::FormatOpts::default();
        options.cluster_size = Some(CLUSTER_BYTES);
        options.label = *format!("{label:<11.11}").as_bytes().first_chunk().unwrap();
        let sectors = device.total_size() / 512;
        fat::Volume::<_, 512>::format_at(Sectors(&mut device), 0, sectors, &options)
            .map_err(|_| io::Error::other("Could not format SD card"))?;
        Self::open(device)
    }
    pub fn entries(&mut self, path: &str) -> io::Result<Vec<Entry>> {
        Ok(self
            .filesystem
            .list(&mut self.device, Path::new(&absolute(path)))
            .map_err(device_error)?
            .into_iter()
            .filter(|entry| entry.name != "." && entry.name != "..")
            .map(|entry| Entry {
                directory: entry.kind == EntryKind::Dir,
                length: entry.size,
                name: entry.name,
            })
            .collect())
    }
    pub fn exists(&mut self, path: &str) -> bool {
        let path = absolute(path);
        let (parent, leaf) = path.rsplit_once('/').unwrap_or(("", &path));
        self.entries(parent).is_ok_and(|entries| {
            entries
                .iter()
                .any(|entry| entry.name.eq_ignore_ascii_case(leaf))
        })
    }
    pub fn read(&mut self, path: &str) -> io::Result<Vec<u8>> {
        self.read_limited(path, u64::MAX)
    }
    /// Read at most `limit` bytes of a file.
    pub fn read_limited(&mut self, path: &str, limit: u64) -> io::Result<Vec<u8>> {
        if !self.exists(path) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{path} not found on SD card"),
            ));
        }
        let mut bytes = Vec::new();
        self.filesystem
            .open_file_ro(&mut self.device, Path::new(&absolute(path)))
            .map_err(device_error)?
            .take(limit)
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    }
    /// Create or replace a file from exactly `length` bytes of `reader`.
    pub fn write_from(&mut self, path: &str, reader: &mut dyn Read, length: u64) -> io::Result<()> {
        if self.exists(path) {
            self.remove(path)?;
        }
        self.filesystem
            .create_file_streaming(
                &mut self.device,
                Path::new(&absolute(path)),
                reader,
                length,
                FileMeta::default(),
            )
            .map_err(device_error)
    }
    pub fn write(&mut self, path: &str, mut bytes: &[u8]) -> io::Result<()> {
        let length = bytes.len() as u64;
        self.write_from(path, &mut bytes, length)
    }
    pub fn create_dir(&mut self, path: &str) -> io::Result<()> {
        self.filesystem
            .create_dir(
                &mut self.device,
                Path::new(&absolute(path)),
                FileMeta::default(),
            )
            .map_err(device_error)
    }
    pub fn remove(&mut self, path: &str) -> io::Result<()> {
        self.filesystem
            .remove(&mut self.device, Path::new(&absolute(path)))
            .map_err(device_error)
    }
    pub fn flush(&mut self) -> io::Result<()> {
        self.filesystem
            .flush(&mut self.device)
            .map_err(device_error)?;
        self.device.sync().map_err(device_error)
    }
}

/// Open the volume on a card image.
pub fn open(path: &Path) -> io::Result<Volume<FileBackend>> {
    Volume::open(FileBackend::open(path).map_err(device_error)?)
}

/// Read one file from a card image without writing to it. The volume spans the
/// image, or the first FAT or exFAT partition of its MBR.
pub fn read_file(path: &Path, name: &str, limit: u64) -> io::Result<Vec<u8>> {
    let mut image = FileBackend::open_read_only(path).map_err(device_error)?;
    let size = image.total_size();
    let mut sector = [0; 512];
    image.read_at(0, &mut sector).map_err(device_error)?;
    if sector[510..] != [0x55, 0xaa] {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid SD boot sector",
        ));
    }
    if sector[3..11] == *b"EXFAT   "
        || sector[82..90] == *b"FAT32   "
        || sector[54..62] == *b"FAT16   "
        || sector[54..62] == *b"FAT12   "
    {
        return Volume::open(image)?.read_limited(name, limit);
    }
    let partition = sector[446..510]
        .chunks_exact(16)
        .find(|p| matches!(p[4], 1 | 4 | 6 | 7 | 0xb | 0xc | 0xe))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no supported partition"))?;
    let start = u64::from(u32::from_le_bytes(partition[8..12].try_into().unwrap())) * 512;
    let length = u64::from(u32::from_le_bytes(partition[12..16].try_into().unwrap())) * 512;
    if length == 0 || start + length > size {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid MBR partition bounds",
        ));
    }
    let slice = SlicedBackend::new(&mut image, start, length).map_err(device_error)?;
    Volume::open(slice)?.read_limited(name, limit)
}

/// The bytes of a small blank volume, for tests of the layers below the filesystem.
pub fn blank_volume_bytes(length: u64, exfat: bool) -> io::Result<Vec<u8>> {
    let mut device = MemoryBackend::new(length);
    if exfat {
        Exfat::format(&mut device, &FormatOpts::default()).map_err(device_error)?;
    } else {
        let mut options = FatFormatOpts::default();
        options.kind = FatKind::Fat16;
        options.total_sectors = (length / 512) as u32;
        let mut fat = Fat32::format(&mut device, &options).map_err(device_error)?;
        fat.flush(&mut device).map_err(device_error)?;
    }
    let mut bytes = vec![0; length as usize];
    device.read_at(0, &mut bytes).map_err(device_error)?;
    Ok(bytes)
}

/// Whether an entry exists on a card image, without writing to it.
pub fn entry_exists(path: &Path, name: &str) -> io::Result<bool> {
    let image = FileBackend::open_read_only(path).map_err(device_error)?;
    Ok(Volume::open(image)?.exists(name))
}

/// Create an empty card. Never replaces an existing file.
pub fn create_empty(path: &Path, label: &str) -> io::Result<Volume<FileBackend>> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)?;
    file.set_len(CARD_BYTES)?;
    drop(file);
    let volume = Volume::format(FileBackend::open(path).map_err(device_error)?, label)?;
    let mut boot = [0; 90];
    Read::read_exact(&mut fs::File::open(path)?, &mut boot)?;
    if boot[82..90] != *b"FAT32   " || u32::from(boot[13]) * 512 != CLUSTER_BYTES {
        return Err(io::Error::other(
            "SD card does not have the FAT32 layout the firmware needs",
        ));
    }
    Ok(volume)
}

/// The file the firmware expects at the card root: its name and contents.
pub fn system_file(model: l6fw::Model) -> (&'static str, Vec<u8>) {
    let (name, year, label) = match model {
        l6fw::Model::L6 => ("ZOOM_L6.SYS", 2024, "L6"),
        l6fw::Model::L6max => ("ZOOM_L6max.sys", 2025, "L6max"),
    };
    let mut bytes = vec![0; 2188];
    let header = format!("ZOOM SYSTEM FILE(c) {year} ZOOM CORPORATION. All Rights Reserved.");
    bytes[..header.len()].copy_from_slice(header.as_bytes());
    bytes[0x50..0x55].copy_from_slice(b" 1.00");
    bytes[0x60..0x80].copy_from_slice(format!("{label:<32}").as_bytes());
    (name, bytes)
}

fn write_system_file<D: BlockDevice>(volume: &mut Volume<D>, model: l6fw::Model) -> io::Result<()> {
    let (name, bytes) = system_file(model);
    if volume.exists(name) {
        return Ok(());
    }
    volume.write(name, &bytes)
}

/// What `l6fs card create` puts on a card beyond the system file.
#[derive(Clone, Debug, Default)]
pub struct Contents {
    pub omit_system_file: bool,
    /// Card path and host file.
    pub files: Vec<(String, std::path::PathBuf)>,
    pub sound_pads: Vec<pad_settings::SoundPad>,
}

/// Create a card holding `contents`.
pub fn create_with(path: &Path, model: l6fw::Model, contents: &Contents) -> io::Result<()> {
    publish(path, "L6MAX", |volume| {
        if !contents.omit_system_file {
            write_system_file(volume, model)?;
        }
        for (card_path, host) in &contents.files {
            spec::create_parents(volume, card_path)?;
            volume.write(card_path, &fs::read(host)?)?;
        }
        if !contents.sound_pads.is_empty() {
            spec::create_parents(volume, "SOUND_PAD/")?;
            pad_settings::apply_to(volume, model, &contents.sound_pads)
                .map_err(io::Error::other)?;
        }
        Ok(())
    })
}

/// Build a card at a temporary path; publish it only when complete, and never
/// replace existing media.
fn publish(
    path: &Path,
    label: &str,
    populate: impl FnOnce(&mut Volume<FileBackend>) -> io::Result<()>,
) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".sd-card-{}.tmp", std::process::id()));
    let result = (|| {
        let mut volume = create_empty(&temporary, label)?;
        populate(&mut volume)?;
        volume.flush()?;
        drop(volume);
        // hard_link fails if another creator has already published this path.
        fs::hard_link(&temporary, path)
    })();
    let cleanup = fs::remove_file(&temporary);
    result.and(cleanup)
}

/// Create a demo card with generated tones and a copy of the firmware package.
pub fn create(path: &Path, firmware: &Path) -> io::Result<()> {
    let package = fs::read(firmware).map_err(|error| {
        io::Error::new(error.kind(), format!("{}: {error}", firmware.display()))
    })?;
    let model = l6fw::model_from_package(&package)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    publish(path, "L6MAX DEMO", |volume| {
        let package_name = match model {
            l6fw::Model::L6 => "L6.BIN",
            l6fw::Model::L6max => "L6max.BIN",
        };
        write_system_file(volume, model)?;
        volume.write(package_name, &package)?;
        volume.create_dir("SOUND_PAD")?;
        for (i, frequency) in [220., 330., 440., 660.].into_iter().enumerate() {
            volume.create_dir(&format!("SOUND_PAD/PAD{}", i + 1))?;
            volume.write(
                &format!("SOUND_PAD/PAD{}/TONE{}.WAV", i + 1, frequency as u32),
                &tone(frequency),
            )?;
        }
        volume.write(
            "SOUND_PAD/PAD1/SNARE8.WAV",
            include_bytes!("../assets/SNARE8.WAV"),
        )?;
        volume.write("README.TXT", format!("L6/L6max emulator demo card\r\nFour generated 1-second mono 48 kHz / 16-bit WAV tones in SOUND_PAD/PAD1-PAD4.\r\nSNARE8.WAV in PAD1 is a 3.8-second stereo 48 kHz / 32-bit float sample.\r\n{package_name} is a copy of the local firmware package.\r\nL6 pad playback can be routed to a configured QEMU audio backend.\r\n").as_bytes())
    })
}

fn tone(frequency: f64) -> Vec<u8> {
    let samples = 48000u32;
    let data_bytes = samples * 2;
    let mut wav = Vec::with_capacity(44 + data_bytes as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&samples.to_le_bytes());
    wav.extend_from_slice(&(samples * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_bytes.to_le_bytes());
    for i in 0..samples {
        let fade = (i.min(samples - 1 - i) as f64 / 480.).min(1.);
        let value = (std::f64::consts::TAU * frequency * i as f64 / samples as f64).sin();
        wav.extend_from_slice(&((value * fade * 8192.) as i16).to_le_bytes());
    }
    wav
}

/// Snapshot a directory into a new card, adding the system file if it has none. Never writes to the source tree.
pub fn from_directory(path: &Path, source: &Path, model: l6fw::Model) -> io::Result<()> {
    const CLUSTER: u64 = CLUSTER_BYTES as u64;
    const MAX_BYTES: u64 = CARD_BYTES;
    struct Entry {
        path: std::path::PathBuf,
        directory: bool,
        length: u64,
    }
    fn scan(
        source: &Path,
        relative: &Path,
        depth: usize,
        entries: &mut Vec<Entry>,
        required: &mut u64,
    ) -> io::Result<()> {
        if depth > 32 {
            return Err(io::Error::other("SD folder nesting exceeds 32 levels"));
        }
        let mut children = fs::read_dir(source.join(relative))?.collect::<Result<Vec<_>, _>>()?;
        children.sort_by_key(|entry| entry.file_name());
        let mut names = std::collections::HashSet::new();
        let mut slots = 2u64;
        for child in children {
            let name = child.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| io::Error::other("SD filenames must be valid Unicode"))?;
            if name.encode_utf16().count() > 255
                || name.chars().any(|c| c < ' ' || "\"*?:<>|/\\".contains(c))
                || name.ends_with([' ', '.'])
            {
                return Err(io::Error::other(format!(
                    "Filename cannot be represented on FAT: {name}"
                )));
            }
            if !names.insert(name.to_lowercase()) {
                return Err(io::Error::other(format!(
                    "Case-insensitive filename collision: {name}"
                )));
            }
            slots += 1 + (name.encode_utf16().count() as u64).div_ceil(13);
            let kind = child.file_type()?;
            if !kind.is_dir() && !kind.is_file() {
                return Err(io::Error::other(format!(
                    "SD folders cannot contain symlinks or special files: {}",
                    child.path().display()
                )));
            }
            let length = child.metadata()?.len();
            let path = relative.join(name);
            if entries.len() >= 32768 {
                return Err(io::Error::other("SD folder exceeds 32768 entries"));
            }
            entries.push(Entry {
                path: path.clone(),
                directory: kind.is_dir(),
                length,
            });
            if kind.is_dir() {
                scan(source, &path, depth + 1, entries, required)?;
            } else {
                *required = required
                    .checked_add(length.div_ceil(CLUSTER) * CLUSTER)
                    .ok_or_else(|| io::Error::other("SD folder size overflow"))?;
            }
            if *required > MAX_BYTES - 8 * 1024 * 1024 {
                return Err(io::Error::other(
                    "SD folder exceeds the 4 GiB card capacity",
                ));
            }
        }
        *required += (slots * 32).div_ceil(CLUSTER) * CLUSTER;
        Ok(())
    }
    let source = source.canonicalize()?;
    if !source.is_dir() {
        return Err(io::Error::other("Select a directory for the SD card"));
    }
    let absolute_output = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let existing_parent = absolute_output
        .parent()
        .unwrap()
        .ancestors()
        .find(|p| p.exists())
        .ok_or_else(|| io::Error::other("Cannot resolve SD output directory"))?
        .canonicalize()?;
    if existing_parent.starts_with(&source) {
        return Err(io::Error::other(
            "SD card output must be outside the source folder",
        ));
    }
    let mut entries = Vec::new();
    let mut required = 0;
    scan(&source, Path::new(""), 0, &mut entries, &mut required)?;
    publish(path, "L6MAX CARD", |volume| {
        for entry in entries {
            let relative = entry
                .path
                .to_str()
                .ok_or_else(|| io::Error::other("Invalid filename"))?;
            if entry.directory {
                volume.create_dir(relative)?;
            } else {
                let input = source.join(&entry.path);
                let mut source_file = fs::File::open(&input)?;
                if !source_file.metadata()?.is_file()
                    || fs::symlink_metadata(&input)?.file_type().is_symlink()
                    || source_file.metadata()?.len() != entry.length
                {
                    return Err(io::Error::other(
                        "SD source changed during import; try again",
                    ));
                }
                volume.write_from(relative, &mut source_file, entry.length)?;
            }
        }
        write_system_file(volume, model)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> io::Result<std::path::PathBuf> {
        let directory = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        fs::create_dir(&directory)?;
        Ok(directory)
    }

    fn assert_archived(path: &Path, file: &str) {
        let mut image = FileBackend::open(path).unwrap();
        let fat = Fat32::open(&mut image).unwrap();
        let (entry, _) = fat.resolve_entry(&mut image, file).unwrap();
        assert_ne!(entry.attr & 0x20, 0, "{file} lacks the archive attribute");
    }

    /// A package l6fw accepts as L6max.
    fn package() -> Vec<u8> {
        let mut package = vec![0; 0x1a8200];
        package[..17].copy_from_slice(b"L6max System Data");
        package[0x60..0x64].copy_from_slice(&2u32.to_le_bytes());
        package[0x64..0x68].copy_from_slice(&0x40u32.to_le_bytes());
        package[0x68..0x6c].copy_from_slice(b"BOOT");
        package[0x74..0x78].copy_from_slice(b"MAIN");
        package[0x78..0x7c].copy_from_slice(&0x100u32.to_le_bytes());
        package[0x7c..0x80].copy_from_slice(&0x1a8100u32.to_le_bytes());
        package[0x100..0x10f].copy_from_slice(b"L6max Main Data");
        package[0x200..0x204].copy_from_slice(&0x20220000u32.to_le_bytes());
        package[0x204..0x208].copy_from_slice(&0x80000009u32.to_le_bytes());
        package[0x1a3200..0x1a3204].copy_from_slice(&0x20220000u32.to_le_bytes());
        package[0x1a3204..0x1a3208].copy_from_slice(&0x08000009u32.to_le_bytes());
        package[0x1a31f8..0x1a31fc].copy_from_slice(&0x40u32.to_le_bytes());
        package[0x1a31fc..0x1a3200].copy_from_slice(b"0110");
        package[0x1a71fc..0x1a7200].copy_from_slice(b"0100");
        package[0x1a71f8..0x1a71fc].copy_from_slice(&0x40u32.to_le_bytes());
        package[0x1a81f8..0x1a81fc].copy_from_slice(&0x40u32.to_le_bytes());
        package[0x1a81fc..0x1a8200].copy_from_slice(b"0110");
        let sum = package[0x200..]
            .iter()
            .fold(0u32, |sum, byte| sum.wrapping_add(u32::from(*byte)));
        package[0x1fc..0x200].copy_from_slice(&sum.to_be_bytes());
        package
    }

    #[test]
    fn demo_card_has_the_required_layout_and_keeps_existing_media() -> io::Result<()> {
        let directory = scratch("l6-demo-card")?;
        let result = (|| {
            let source = directory.join("firmware.bin");
            let package = package();
            fs::write(&source, &package)?;
            let card = directory.join("card.img");
            create(&card, &source)?;
            assert_eq!(fs::metadata(&card)?.len(), CARD_BYTES);
            let mut sector = vec![0; 90];
            io::Read::read_exact(&mut fs::File::open(&card)?, &mut sector)?;
            assert_eq!(&sector[82..90], b"FAT32   ");
            assert_eq!(u32::from(sector[13]) * 512, CLUSTER_BYTES);
            assert_archived(&card, "/SOUND_PAD/PAD1/TONE220.WAV");
            let mut volume = open(&card)?;
            assert_eq!(volume.read("L6max.BIN")?, package);
            for (i, hz) in [220, 330, 440, 660].into_iter().enumerate() {
                let wav = volume.read(&format!("SOUND_PAD/PAD{}/TONE{hz}.WAV", i + 1))?;
                assert_eq!(&wav[..4], b"RIFF");
                assert_eq!(&wav[8..12], b"WAVE");
                assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 48000);
                assert_eq!(wav.len(), 96044);
            }
            let snare = volume.read("SOUND_PAD/PAD1/SNARE8.WAV")?;
            assert_eq!(snare, include_bytes!("../assets/SNARE8.WAV"));
            let pad = volume.entries("SOUND_PAD/PAD1")?;
            assert_eq!(pad.first().map(|e| e.name.as_str()), Some("TONE220.WAV"));
            volume.write("KEEP.TXT", b"user file")?;
            volume.flush()?;
            drop(volume);
            assert_eq!(
                create(&card, &source).unwrap_err().kind(),
                io::ErrorKind::AlreadyExists
            );
            assert_eq!(open(&card)?.read("KEEP.TXT")?, b"user file");
            Ok(())
        })();
        fs::remove_dir_all(directory)?;
        result
    }

    #[test]
    fn folder_snapshot_preserves_hierarchy_and_guest_writes_do_not_touch_source() -> io::Result<()>
    {
        let dir = scratch("l6-folder-sd")?;
        let result = (|| {
            let source = dir.join("source");
            fs::create_dir_all(source.join("SOUND_PAD/PAD1"))?;
            fs::write(
                source.join("SOUND_PAD/PAD1/Long tone name.wav"),
                b"original audio",
            )?;
            fs::write(source.join("L6max.BIN"), b"local update")?;
            let card = dir.join("card.img");
            from_directory(&card, &source, l6fw::Model::L6max)?;
            let mut volume = open(&card)?;
            assert_eq!(
                volume.read("SOUND_PAD/PAD1/Long tone name.wav")?,
                b"original audio"
            );
            volume.write("GUEST.TXT", b"guest writes")?;
            volume.flush()?;
            drop(volume);
            assert!(!source.join("GUEST.TXT").exists());
            assert_eq!(
                from_directory(&card, &source, l6fw::Model::L6max)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::AlreadyExists
            );
            let invalid = dir.join("invalid.img");
            std::os::unix::fs::symlink(source.join("L6max.BIN"), source.join("link.bin"))?;
            assert!(from_directory(&invalid, &source, l6fw::Model::L6max).is_err());
            assert!(!invalid.exists());
            Ok(())
        })();
        fs::remove_dir_all(dir)?;
        result
    }

    #[test]
    fn system_file_has_the_observed_layout() {
        let (name, bytes) = system_file(l6fw::Model::L6max);
        assert_eq!(name, "ZOOM_L6max.sys");
        assert_eq!(bytes.len(), 2188);
        assert!(bytes.starts_with(b"ZOOM SYSTEM FILE(c) 2025 ZOOM CORPORATION."));
        assert_eq!(&bytes[0x50..0x55], b" 1.00");
        assert_eq!(&bytes[0x60..0x80], b"L6max                           ");
        assert!(bytes[0x80..].iter().all(|byte| *byte == 0));
        let (name, bytes) = system_file(l6fw::Model::L6);
        assert_eq!(name, "ZOOM_L6.SYS");
        assert!(bytes.starts_with(b"ZOOM SYSTEM FILE(c) 2024 ZOOM CORPORATION."));
        assert_eq!(&bytes[0x60..0x80], b"L6                              ");
    }

    #[test]
    fn card_contents_put_assignments_and_files_on_the_card() -> io::Result<()> {
        let dir = scratch("l6-card-contents")?;
        let result = (|| {
            let wav = dir.join("a.wav");
            fs::write(&wav, b"audio")?;
            let card = dir.join("card.img");
            let contents = Contents {
                omit_system_file: false,
                files: vec![("SOUND_PAD/PAD2/A.WAV".into(), wav)],
                sound_pads: vec!["2=SOUND_PAD/PAD2/A.WAV,level=20".parse().unwrap()],
            };
            create_with(&card, l6fw::Model::L6, &contents)?;
            let mut volume = open(&card)?;
            assert!(volume.exists("ZOOM_L6.SYS"));
            assert_eq!(volume.read("SOUND_PAD/PAD2/A.WAV")?, b"audio");
            let settings = volume.read("SOUND_PAD/L6PADSETTING.ZST")?;
            assert_eq!(settings[0x891], 20);
            Ok(())
        })();
        fs::remove_dir_all(dir)?;
        result
    }

    #[test]
    fn writing_replaces_a_file_and_lookups_ignore_case() -> io::Result<()> {
        let mut volume = Volume::format(MemoryBackend::new(64 * 1024 * 1024), "TEST")?;
        volume.create_dir("SOUND_PAD")?;
        volume.write("SOUND_PAD/SET.ZST", b"first version")?;
        volume.write("SOUND_PAD/SET.ZST", b"second")?;
        assert_eq!(volume.read("sound_pad/set.zst")?, b"second");
        assert_eq!(volume.read_limited("SOUND_PAD/SET.ZST", 3)?, b"sec");
        assert_eq!(
            volume.read("MISSING/FILE").unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        volume.remove("SOUND_PAD/SET.ZST")?;
        assert!(!volume.exists("SOUND_PAD/SET.ZST"));
        Ok(())
    }

    #[test]
    fn exfat_cards_can_still_be_read_and_written() -> io::Result<()> {
        let bytes = blank_volume_bytes(16 * 1024 * 1024, true)?;
        let mut device = MemoryBackend::new(bytes.len() as u64);
        device.write_at(0, &bytes).map_err(device_error)?;
        let mut volume = Volume::open(device)?;
        volume.write("L6.BIN", b"package")?;
        volume.flush()?;
        assert_eq!(volume.read("L6.BIN")?, b"package");
        Ok(())
    }

    #[test]
    fn small_fat16_cards_can_still_be_read_and_written() -> io::Result<()> {
        let dir = scratch("l6-fat-card")?;
        let result = (|| {
            let card = dir.join("fat.img");
            fs::write(&card, blank_volume_bytes(8 * 1024 * 1024, false)?)?;
            let mut volume = open(&card)?;
            volume.write("L6.BIN", b"package")?;
            volume.flush()?;
            drop(volume);
            assert_eq!(read_file(&card, "L6.BIN", 3)?, b"pac");
            Ok(())
        })();
        fs::remove_dir_all(dir)?;
        result
    }
}
