//! Prepare sound-pad assignments on a disposable fixture card.
use serde::{Deserialize, Serialize};
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::Path,
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SoundPad {
    pub pad: u8,
    pub file: String,
    #[serde(default)]
    pub mode: PlayMode,
    #[serde(default = "default_level")]
    pub level: u8,
}
#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlayMode {
    #[default]
    OneShot,
    Loop,
    Hold,
}
fn default_level() -> u8 {
    49
}

pub fn validate(pads: &[SoundPad]) -> Result<(), String> {
    let mut seen = [false; 4];
    for pad in pads {
        if !(1..=4).contains(&pad.pad) || pad.level > 59 {
            return Err("sound_pad pad must be 1..4 and level must be 0..59".into());
        }
        let index = usize::from(pad.pad - 1);
        if std::mem::replace(&mut seen[index], true) {
            return Err(format!("duplicate sound_pad {}", pad.pad));
        }
        let path = pad.file.replace('\\', "/");
        if path.starts_with('/')
            || path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || path.contains(':')
            || path.contains('\0')
        {
            return Err(format!(
                "sound_pad {} needs a relative SD card path",
                pad.pad
            ));
        }
        if format!("A:\\{}", path.replace('/', "\\"))
            .encode_utf16()
            .count()
            >= 260
        {
            return Err(format!("sound_pad {} path is too long", pad.pad));
        }
    }
    Ok(())
}

pub fn apply(card: &Path, model: &str, pads: &[SoundPad]) -> Result<(), String> {
    validate(pads)?;
    let model_name = match model {
        "l6" => "L6",
        "l6max" => "L6MAX",
        _ => return Err("unknown sound pad model".into()),
    };
    let settings_name = if model == "l6" {
        "L6PADSETTING.ZST"
    } else {
        "L6MAXPADSETTING.ZST"
    };
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(card)
        .map_err(|e| e.to_string())?;
    let fs = fatfs::FileSystem::new(file, fatfs::FsOptions::new()).map_err(|e| e.to_string())?;
    {
        let root = fs.root_dir();
        for pad in pads {
            let path = pad.file.replace('\\', "/");
            root.open_file(&path)
                .map_err(|e| format!("sound_pad {} file {path}: {e}", pad.pad))?;
        }
        let dir = root
            .open_dir("SOUND_PAD")
            .map_err(|e| format!("SOUND_PAD directory: {e}"))?;
        let mut data = match dir.open_file(settings_name) {
            Ok(mut settings) => {
                let mut bytes = Vec::new();
                settings
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                bytes
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut bytes = vec![0u8; 0x1094];
                let header = format!("ZOOM {model_name:<7}PAD SETTING VER0001");
                for (i, c) in header
                    .chars()
                    .chain(std::iter::repeat(' '))
                    .take(47)
                    .enumerate()
                {
                    let c = c as u16;
                    bytes[2 * i..2 * i + 2].copy_from_slice(&c.to_le_bytes());
                }
                bytes[0x888..0x88c].fill(1);
                bytes[0x88c..0x890].fill(1);
                bytes[0x890..0x894].fill(49);
                bytes
            }
            Err(e) => return Err(format!("{settings_name}: {e}")),
        };
        let expected = format!("ZOOM {model_name:<7}PAD SETTING VER0001")
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        if data.len() < 0x894 || !data.starts_with(&expected) {
            return Err(format!(
                "{settings_name} is not a valid {model_name} pad settings file"
            ));
        }
        for pad in pads {
            let index = usize::from(pad.pad - 1);
            let start = 0x60 + index * 0x20a;
            data[start..start + 520].fill(0);
            let path = format!("A:\\{}", pad.file.replace('/', "\\"));
            for (i, c) in path.encode_utf16().enumerate() {
                data[start + i * 2..start + i * 2 + 2].copy_from_slice(&c.to_le_bytes());
            }
            data[0x88c + index] = match pad.mode {
                PlayMode::OneShot => 0,
                PlayMode::Loop => 1,
                PlayMode::Hold => 2,
            };
            data[0x890 + index] = pad.level;
        }
        let mut settings = dir.create_file(settings_name).map_err(|e| e.to_string())?;
        settings.truncate().map_err(|e| e.to_string())?;
        settings.write_all(&data).map_err(|e| e.to_string())?;
        settings.flush().map_err(|e| e.to_string())?;
    }
    fs.unmount().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_pad_settings_without_losing_other_assignments() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("l6-pad-state-{}-{stamp}.img", std::process::id()));
        let mut image = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        image.set_len(16 * 1024 * 1024).unwrap();
        fatfs::format_volume(&mut image, fatfs::FormatVolumeOptions::new()).unwrap();
        let fs = fatfs::FileSystem::new(image, fatfs::FsOptions::new()).unwrap();
        {
            let root = fs.root_dir();
            let sounds = root.create_dir("SOUND_PAD").unwrap();
            let first = sounds.create_dir("PAD1").unwrap();
            first.create_file("ONE.WAV").unwrap();
            let fourth = sounds.create_dir("PAD4").unwrap();
            fourth.create_file("FOUR.WAV").unwrap();
        }
        fs.unmount().unwrap();
        let first: SoundPad =
            toml::from_str("pad=1\nfile='SOUND_PAD/PAD1/ONE.WAV'\nmode='one-shot'\nlevel=59")
                .unwrap();
        let fourth: SoundPad =
            toml::from_str("pad=4\nfile='SOUND_PAD/PAD4/FOUR.WAV'\nmode='loop'\nlevel=25").unwrap();
        apply(&path, "l6", &[first]).unwrap();
        apply(&path, "l6", &[fourth]).unwrap();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let fs = fatfs::FileSystem::new(file, fatfs::FsOptions::new()).unwrap();
        {
            let mut data = Vec::new();
            fs.root_dir()
                .open_file("SOUND_PAD/L6PADSETTING.ZST")
                .unwrap()
                .read_to_end(&mut data)
                .unwrap();
            let decode = |start: usize| {
                String::from_utf16(
                    &data[start..start + 520]
                        .chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .take_while(|c| *c != 0)
                        .collect::<Vec<_>>(),
                )
                .unwrap()
            };
            assert_eq!(decode(0x60), r"A:\SOUND_PAD\PAD1\ONE.WAV");
            assert_eq!(decode(0x60 + 3 * 0x20a), r"A:\SOUND_PAD\PAD4\FOUR.WAV");
            assert_eq!([data[0x88c], data[0x88f]], [0, 1]);
            assert_eq!([data[0x890], data[0x893]], [59, 25]);
        }
        fs.unmount().unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn validates_semantic_pad_configuration() {
        let pad: SoundPad =
            toml::from_str("pad=1\nfile='SOUND_PAD/PAD1/TEST.WAV'\nmode='one-shot'\nlevel=59\n")
                .unwrap();
        assert!(validate(std::slice::from_ref(&pad)).is_ok());
        assert!(
            validate(&[pad.clone(), pad])
                .unwrap_err()
                .contains("duplicate")
        );
        for input in [
            "pad=0\nfile='TEST.WAV'",
            "pad=1\nfile='../TEST.WAV'",
            "pad=1\nfile='TEST.WAV'\nlevel=60",
        ] {
            let pad: SoundPad = toml::from_str(input).unwrap();
            assert!(validate(&[pad]).is_err());
        }
    }
}
