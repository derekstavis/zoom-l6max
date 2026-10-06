//! Create reusable SD demo media; callers choose whether to enable it.
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
};

/// Create once; publish only a complete card and never replace existing media.
pub fn create(path: &Path, firmware: &Path) -> io::Result<()> {
    let package = fs::read(firmware).map_err(|error| {
        io::Error::new(error.kind(), format!("{}: {error}", firmware.display()))
    })?;
    l6fw::update::validate(&package)?;
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
        root.create_file("L6max.BIN")?.write_all(&package)?;
        let pads = root.create_dir("SOUND_PAD")?;
        for (i, frequency) in [220., 330., 440., 660.].into_iter().enumerate() {
            pads.create_dir(&format!("PAD{}", i + 1))?
                .create_file(&format!("TONE{}.WAV", frequency as u32))?
                .write_all(&tone(frequency))?;
        }
        root.create_file("README.TXT")?.write_all(b"L6max emulator demo card\r\nFour generated 1-second mono 48 kHz / 16-bit WAV tones in SOUND_PAD/PAD1-PAD4.\r\nL6max.BIN is a copy of the local firmware package.\r\nAudio output is not modeled by the emulator.\r\n")?;
        drop(pads);
        drop(root);
        volume.unmount()?;
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
            package[0x60] = 2;
            package[0x64] = 0x40;
            package[0x78..0x7c].copy_from_slice(&0x100u32.to_le_bytes());
            package[0x7c..0x80].copy_from_slice(&0x1a8100u32.to_le_bytes());
            package[0x100..0x10f].copy_from_slice(b"L6max Main Data");
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
