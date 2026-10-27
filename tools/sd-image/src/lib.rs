//! Create reusable SD demo media; callers choose whether to enable it.
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
};

/// Create once; publish only a complete card and never replace existing media.
pub fn create(path: &Path, firmware: &Path) -> io::Result<()> {
    let package = fs::read(firmware).map_err(|error| {
        io::Error::new(error.kind(), format!("{}: {error}", firmware.display()))
    })?;
    let model = l6fw::model_from_package(&package)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".demo-sd-{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.set_len(128 * 1024 * 1024)?;
        // Observed firmware geometry policy requires 32 sectors/cluster here.
        fatfs::format_volume(
            &mut file,
            fatfs::FormatVolumeOptions::new()
                .fat_type(fatfs::FatType::Fat16)
                .bytes_per_cluster(16384)
                .volume_label(*b"L6MAX DEMO "),
        )?;
        let volume = fatfs::FileSystem::new(&mut file, fatfs::FsOptions::new())?;
        let root = volume.root_dir();
        let package_name = match model {
            l6fw::Model::L6 => "L6.BIN",
            l6fw::Model::L6max => "L6max.BIN",
        };
        root.create_file(package_name)?.write_all(&package)?;
        let pads = root.create_dir("SOUND_PAD")?;
        for (i, frequency) in [220., 330., 440., 660.].into_iter().enumerate() {
            pads.create_dir(&format!("PAD{}", i + 1))?
                .create_file(&format!("TONE{}.WAV", frequency as u32))?
                .write_all(&tone(frequency))?;
        }
        pads.open_dir("PAD1")?
            .create_file("SNARE8.WAV")?
            .write_all(include_bytes!("../assets/SNARE8.WAV"))?;
        root.create_file("README.TXT")?.write_all(format!("L6/L6max emulator demo card\r\nFour generated 1-second mono 48 kHz / 16-bit WAV tones in SOUND_PAD/PAD1-PAD4.\r\nSNARE8.WAV in PAD1 is a 3.8-second stereo 48 kHz / 32-bit float sample.\r\n{package_name} is a copy of the local firmware package.\r\nL6 pad playback can be routed to a configured QEMU audio backend.\r\n").as_bytes())?;
        drop(pads);
        drop(root);
        volume.unmount()?;
        mark_wav_files_archived(&mut file)?;
        file.sync_all()?;
        // hard_link fails if another creator has already published this path.
        fs::hard_link(&temporary, path)
    })();
    drop(file);
    let cleanup = fs::remove_file(&temporary);
    result.and(cleanup)
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

/// The L6 sound-pad browser ignores ordinary FAT files unless their archive
/// attribute is set. fatfs creates files with attribute zero, so normalize
/// WAV entries after unmounting the volume.
fn mark_wav_files_archived(file: &mut fs::File) -> io::Result<()> {
    fn u16_at(file: &mut fs::File, offset: u64) -> io::Result<u16> {
        let mut bytes = [0; 2];
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(&mut bytes)?;
        Ok(u16::from_le_bytes(bytes))
    }
    fn visit_directory(
        file: &mut fs::File,
        first_sector: u64,
        fixed_sectors: Option<u64>,
        data_start: u64,
        sectors_per_cluster: u64,
        fat_start: u64,
        fat_sectors: u64,
        bytes_per_sector: u64,
        cluster: u16,
        depth: usize,
    ) -> io::Result<()> {
        if depth > 32 {
            return Err(io::Error::other("FAT directory nesting exceeds 32 levels"));
        }
        let mut sectors = Vec::new();
        if let Some(count) = fixed_sectors {
            sectors.extend(first_sector..first_sector + count);
        } else {
            let mut current = cluster;
            loop {
                if current < 2 || current >= 0xfff8 {
                    break;
                }
                sectors.extend(
                    data_start + u64::from(current - 2) * sectors_per_cluster
                        ..data_start
                            + u64::from(current - 2) * sectors_per_cluster
                            + sectors_per_cluster,
                );
                let next = u16_at(file, fat_start + u64::from(current) * 2)?;
                if next >= 0xfff8 {
                    break;
                }
                if next < 2 || next >= 0xfff0 {
                    return Err(io::Error::other("Invalid FAT16 directory chain"));
                }
                current = next;
            }
        }
        let mut subdirectories = Vec::new();
        'sectors: for sector in sectors {
            let offset = sector * bytes_per_sector;
            let mut contents = vec![0; bytes_per_sector as usize];
            file.seek(SeekFrom::Start(offset))?;
            file.read_exact(&mut contents)?;
            for (index, entry) in contents.chunks_exact_mut(32).enumerate() {
                if entry[0] == 0 {
                    break 'sectors;
                }
                if entry[0] == 0xe5 || entry[11] == 0x0f || entry[11] & 0x08 != 0 {
                    continue;
                }
                if entry[11] & 0x10 != 0 {
                    if &entry[..11] != b".          " && &entry[..11] != b"..         " {
                        let child = u16::from_le_bytes([entry[26], entry[27]]);
                        if child >= 2 {
                            subdirectories.push(child);
                        }
                    }
                } else if &entry[8..11] == b"WAV" && entry[11] & 0x20 == 0 {
                    entry[11] |= 0x20;
                    file.seek(SeekFrom::Start(offset + (index * 32) as u64))?;
                    file.write_all(entry)?;
                }
            }
        }
        for child in subdirectories {
            visit_directory(
                file,
                0,
                None,
                data_start,
                sectors_per_cluster,
                fat_start,
                fat_sectors,
                bytes_per_sector,
                child,
                depth + 1,
            )?;
        }
        let _ = fat_sectors;
        Ok(())
    }

    let mut bpb = [0; 36];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut bpb)?;
    let bytes_per_sector = u64::from(u16::from_le_bytes([bpb[11], bpb[12]]));
    let sectors_per_cluster = u64::from(bpb[13]);
    let reserved = u64::from(u16::from_le_bytes([bpb[14], bpb[15]]));
    let fat_count = u64::from(bpb[16]);
    let root_entries = u64::from(u16::from_le_bytes([bpb[17], bpb[18]]));
    let fat_sectors = u64::from(u16::from_le_bytes([bpb[22], bpb[23]]));
    if bytes_per_sector != 512 || sectors_per_cluster == 0 || fat_count == 0 || fat_sectors == 0 {
        return Err(io::Error::other("Unsupported FAT16 geometry"));
    }
    let fat_start = reserved * bytes_per_sector;
    let root_sector = reserved + fat_count * fat_sectors;
    let root_sectors = (root_entries * 32).div_ceil(bytes_per_sector);
    let data_start = root_sector + root_sectors;
    visit_directory(
        file,
        root_sector,
        Some(root_sectors),
        data_start,
        sectors_per_cluster,
        fat_start,
        fat_sectors,
        bytes_per_sector,
        0,
        0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn populated_card_preserves_existing_media() -> io::Result<()> {
        let directory = std::env::temp_dir().join(format!("l6-demo-card-{}", std::process::id()));
        fs::create_dir(&directory)?;
        let result = (|| {
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
            let source = directory.join("firmware.bin");
            fs::write(&source, &package)?;
            let card = directory.join("card.img");
            create(&card, &source)?;
            let file = OpenOptions::new().read(true).write(true).open(&card)?;
            assert_eq!(file.metadata()?.len(), 128 * 1024 * 1024);
            let volume = fatfs::FileSystem::new(file, fatfs::FsOptions::new())?;
            assert_eq!(volume.fat_type(), fatfs::FatType::Fat16);
            assert_eq!(volume.stats()?.cluster_size(), 16384);
            let root = volume.root_dir();
            let mut stored = Vec::new();
            root.open_file("L6max.BIN")?.read_to_end(&mut stored)?;
            assert_eq!(stored, package);
            for (i, hz) in [220, 330, 440, 660].into_iter().enumerate() {
                let mut wav = Vec::new();
                root.open_file(&format!("SOUND_PAD/PAD{}/TONE{hz}.WAV", i + 1))?
                    .read_to_end(&mut wav)?;
                assert_eq!(&wav[..4], b"RIFF");
                assert_eq!(&wav[8..12], b"WAVE");
                assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 48000);
                assert_eq!(wav.len(), 96044);
            }
            let mut snare = Vec::new();
            root.open_file("SOUND_PAD/PAD1/SNARE8.WAV")?
                .read_to_end(&mut snare)?;
            assert_eq!(snare, include_bytes!("../assets/SNARE8.WAV"));
            assert_eq!(u32::from_le_bytes(snare[24..28].try_into().unwrap()), 48000);
            assert_eq!(u16::from_le_bytes(snare[22..24].try_into().unwrap()), 2);
            assert_eq!(u16::from_le_bytes(snare[20..22].try_into().unwrap()), 3);
            assert_eq!(u16::from_le_bytes(snare[34..36].try_into().unwrap()), 32);
            let pad = root.open_dir("SOUND_PAD/PAD1")?;
            let pad_files = pad
                .iter()
                .filter_map(Result::ok)
                .filter(|entry| !entry.is_dir())
                .map(|entry| entry.file_name())
                .collect::<Vec<_>>();
            assert_eq!(pad_files.first().map(String::as_str), Some("TONE220.WAV"));
            assert!(
                pad.iter()
                    .filter_map(Result::ok)
                    .any(|entry| entry.attributes().contains(fatfs::FileAttributes::ARCHIVE))
            );
            drop(pad);
            root.create_file("KEEP.TXT")?.write_all(b"user file")?;
            drop(root);
            volume.unmount()?;
            assert_eq!(
                create(&card, &source).unwrap_err().kind(),
                io::ErrorKind::AlreadyExists
            );
            let volume = fatfs::FileSystem::new(fs::File::open(&card)?, fatfs::FsOptions::new())?;
            let mut kept = String::new();
            volume
                .root_dir()
                .open_file("KEEP.TXT")?
                .read_to_string(&mut kept)?;
            assert_eq!(kept, "user file");
            volume.unmount()?;
            Ok(())
        })();
        fs::remove_dir_all(directory)?;
        result
    }
}

/// Snapshot a directory into a new FAT16 card. Never writes to the source tree.
/// Images are 128–512 MiB with the firmware-tested 16 KiB cluster geometry.
pub fn from_directory(path: &Path, source: &Path) -> io::Result<()> {
    const CLUSTER: u64 = 16384;
    const MAX_BYTES: u64 = 512 * 1024 * 1024;
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
                    "SD folder exceeds the supported 512 MiB card capacity",
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
    let bytes = (required + 8 * 1024 * 1024)
        .max(128 * 1024 * 1024)
        .next_power_of_two();
    if bytes > MAX_BYTES {
        return Err(io::Error::other(
            "SD folder exceeds the supported 512 MiB card capacity",
        ));
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".folder-sd-{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.set_len(bytes)?;
        fatfs::format_volume(
            &mut file,
            fatfs::FormatVolumeOptions::new()
                .fat_type(fatfs::FatType::Fat16)
                .bytes_per_cluster(CLUSTER as u32)
                .volume_label(*b"L6MAX CARD "),
        )?;
        let volume = fatfs::FileSystem::new(&mut file, fatfs::FsOptions::new())?;
        let root = volume.root_dir();
        for entry in entries {
            let relative = entry
                .path
                .to_str()
                .ok_or_else(|| io::Error::other("Invalid filename"))?;
            if entry.directory {
                root.create_dir(relative)?;
            } else {
                let input = source.join(&entry.path);
                let mut source_file = fs::File::open(&input)?;
                if !source_file.metadata()?.is_file()
                    || fs::symlink_metadata(&input)?.file_type().is_symlink()
                {
                    return Err(io::Error::other("SD source changed during import"));
                }
                let copied = io::copy(&mut source_file, &mut root.create_file(relative)?)?;
                if copied != entry.length {
                    return Err(io::Error::other(
                        "SD source changed during import; try again",
                    ));
                }
            }
        }
        drop(root);
        volume.unmount()?;
        mark_wav_files_archived(&mut file)?;
        file.sync_all()?;
        fs::hard_link(&temporary, path)
    })();
    drop(file);
    let cleanup = fs::remove_file(&temporary);
    result.and(cleanup)
}

#[cfg(test)]
mod directory_tests {
    use super::*;
    use std::io::Read;
    #[test]
    fn folder_snapshot_preserves_hierarchy_and_guest_writes_do_not_touch_source() -> io::Result<()>
    {
        let dir = std::env::temp_dir().join(format!("l6-folder-sd-{}", std::process::id()));
        fs::create_dir(&dir)?;
        let result = (|| {
            let source = dir.join("source");
            fs::create_dir_all(source.join("SOUND_PAD/PAD1"))?;
            fs::write(
                source.join("SOUND_PAD/PAD1/Long tone name.wav"),
                b"original audio",
            )?;
            fs::write(source.join("L6max.BIN"), b"local update")?;
            let card = dir.join("card.img");
            from_directory(&card, &source)?;
            let file = OpenOptions::new().read(true).write(true).open(&card)?;
            let volume = fatfs::FileSystem::new(file, fatfs::FsOptions::new())?;
            assert_eq!(volume.stats()?.cluster_size(), 16384);
            let root = volume.root_dir();
            let mut bytes = Vec::new();
            root.open_file("SOUND_PAD/PAD1/Long tone name.wav")?
                .read_to_end(&mut bytes)?;
            assert_eq!(bytes, b"original audio");
            let pad = root.open_dir("SOUND_PAD/PAD1")?;
            assert!(
                pad.iter()
                    .filter_map(Result::ok)
                    .any(|entry| entry.attributes().contains(fatfs::FileAttributes::ARCHIVE))
            );
            drop(pad);
            root.create_file("GUEST.TXT")?.write_all(b"guest writes")?;
            drop(root);
            volume.unmount()?;
            assert_eq!(
                fs::read(source.join("SOUND_PAD/PAD1/Long tone name.wav"))?,
                b"original audio"
            );
            assert!(!source.join("GUEST.TXT").exists());
            assert_eq!(
                from_directory(&card, &source).unwrap_err().kind(),
                io::ErrorKind::AlreadyExists
            );
            let invalid = dir.join("invalid.img");
            std::os::unix::fs::symlink(source.join("L6max.BIN"), source.join("link.bin"))?;
            assert!(from_directory(&invalid, &source).is_err());
            assert!(!invalid.exists());
            Ok(())
        })();
        fs::remove_dir_all(dir)?;
        result
    }
}
