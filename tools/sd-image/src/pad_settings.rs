//! Sound-pad assignments, saved on the card in the model's pad settings file.
use crate::{BlockDevice, Volume};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SoundPad {
    pub pad: u8,
    pub file: String,
    #[serde(default)]
    pub mode: PlayMode,
    #[serde(default = "default_level")]
    pub level: u8,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
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

/// Assign pads on a card image.
pub fn apply(card: &Path, model: l6fw::Model, pads: &[SoundPad]) -> Result<(), String> {
    let mut volume = crate::open(card).map_err(|e| e.to_string())?;
    apply_to(&mut volume, model, pads)?;
    volume.flush().map_err(|e| e.to_string())
}

/// Create or update the pad settings file. The assigned WAVs must already exist.
pub fn apply_to<D: BlockDevice>(
    volume: &mut Volume<D>,
    model: l6fw::Model,
    pads: &[SoundPad],
) -> Result<(), String> {
    validate(pads)?;
    let (model_name, settings_name) = match model {
        l6fw::Model::L6 => ("L6", "L6PADSETTING.ZST"),
        l6fw::Model::L6max => ("L6MAX", "L6MAXPADSETTING.ZST"),
    };
    let settings_path = format!("SOUND_PAD/{settings_name}");
    {
        for pad in pads {
            let path = pad.file.replace('\\', "/");
            if !volume.exists(&path) {
                return Err(format!("sound_pad {} file {path}: not found", pad.pad));
            }
        }
        if !volume.exists("SOUND_PAD") {
            return Err("SOUND_PAD directory: not found".into());
        }
        let mut data = match volume.read(&settings_path) {
            Ok(bytes) => bytes,
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
        volume
            .write(&settings_path, &data)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Parse `PAD=FILE[,mode=MODE][,level=LEVEL]`, as `l6fs` takes on its command line.
impl std::str::FromStr for SoundPad {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, String> {
        let usage =
            || format!("{value}: expected PAD=FILE[,mode=one-shot|loop|hold][,level=0..59]");
        let (pad, rest) = value.split_once('=').ok_or_else(usage)?;
        let mut fields = rest.split(',');
        let mut result = SoundPad {
            pad: pad.parse().map_err(|_| usage())?,
            file: fields.next().ok_or_else(usage)?.into(),
            mode: PlayMode::default(),
            level: default_level(),
        };
        for field in fields {
            match field.split_once('=').ok_or_else(usage)? {
                ("mode", "one-shot") => result.mode = PlayMode::OneShot,
                ("mode", "loop") => result.mode = PlayMode::Loop,
                ("mode", "hold") => result.mode = PlayMode::Hold,
                ("level", level) => result.level = level.parse().map_err(|_| usage())?,
                _ => return Err(usage()),
            }
        }
        validate(std::slice::from_ref(&result))?;
        Ok(result)
    }
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
        let mut volume = crate::create_empty(&path, "TEST").unwrap();
        for directory in ["SOUND_PAD", "SOUND_PAD/PAD1", "SOUND_PAD/PAD4"] {
            volume.create_dir(directory).unwrap();
        }
        volume.write("SOUND_PAD/PAD1/ONE.WAV", b"").unwrap();
        volume.write("SOUND_PAD/PAD4/FOUR.WAV", b"").unwrap();
        volume.flush().unwrap();
        drop(volume);
        let first: SoundPad =
            toml::from_str("pad=1\nfile='SOUND_PAD/PAD1/ONE.WAV'\nmode='one-shot'\nlevel=59")
                .unwrap();
        let fourth: SoundPad =
            toml::from_str("pad=4\nfile='SOUND_PAD/PAD4/FOUR.WAV'\nmode='loop'\nlevel=25").unwrap();
        apply(&path, l6fw::Model::L6, &[first]).unwrap();
        apply(&path, l6fw::Model::L6, &[fourth]).unwrap();
        {
            let data = crate::open(&path)
                .unwrap()
                .read("SOUND_PAD/L6PADSETTING.ZST")
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
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn command_line_assignments_parse_and_reject_bad_input() {
        let pad: SoundPad = "4=SOUND_PAD/PAD4/A.WAV,mode=loop,level=25".parse().unwrap();
        assert_eq!(
            (pad.pad, pad.file.as_str(), pad.level),
            (4, "SOUND_PAD/PAD4/A.WAV", 25)
        );
        assert!(matches!(pad.mode, PlayMode::Loop));
        let pad: SoundPad = "1=A.WAV".parse().unwrap();
        assert_eq!(pad.level, 49);
        for bad in ["A.WAV", "9=A.WAV", "1=A.WAV,mode=other", "1=A.WAV,level=60"] {
            assert!(bad.parse::<SoundPad>().is_err(), "{bad}");
        }
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
