//! Declare a card's contents in TOML and build it.
use crate::pad_settings::SoundPad;
use serde::Deserialize;
use std::{
    f64::consts::TAU,
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardSpec {
    pub version: u32,
    pub model: String,
    /// Add the firmware's system file; without it, Record opens the SD Card menu.
    #[serde(default = "enabled")]
    pub system_file: bool,
    #[serde(default)]
    pub sound_pad: Vec<SoundPad>,
    #[serde(default)]
    pub project: Vec<Project>,
    #[serde(default)]
    pub directory: Vec<String>,
    #[serde(default)]
    pub file: Vec<File>,
}

fn enabled() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct File {
    /// Path from the card root. Parent directories are created as needed.
    pub path: String,
    #[serde(flatten)]
    pub content: Content,
}

/// One hold of a `levels` tone. Silence when both levels are zero.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Level {
    pub seconds: f64,
    pub left: f64,
    pub right: f64,
}

/// A recorder project on L6max, as the firmware writes one: a settings file, eight
/// tracks and a master, all holding the same tone.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    /// Folder under `RECORDER`, by the firmware's convention `YYMMDD_HHMMSS`.
    pub name: String,
    pub hz: f64,
    pub seconds: f64,
    pub amplitude: f64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Content {
    /// A stereo tone in both channels.
    Tone {
        hz: f64,
        seconds: f64,
        amplitude: f64,
    },
    /// A stereo sweep rising on the left and falling on the right, so the
    /// channels stay distinct and no two windows of the signal repeat.
    Sweep {
        low_hz: f64,
        high_hz: f64,
        seconds: f64,
        amplitude: f64,
    },
    /// A stereo tone stepping through holds with separate channel levels.
    Levels {
        hz: f64,
        steps: Vec<Level>,
    },
    /// A host file, relative to the spec.
    Copy(PathBuf),
    Text(String),
}

const SAMPLE_RATE: u32 = 48000;
/// Avoid a click at each end of generated audio.
const FADE_SECONDS: f64 = 0.002;

/// A 48 kHz 32-bit float WAV. `pad` adds the zeroed chunk the recorder writes
/// between the format and the data.
fn wav_file(channels: u16, pad: u32, samples: &[f32]) -> Vec<u8> {
    let data_bytes = (samples.len() * 4) as u32;
    let frame_bytes = u32::from(channels) * 4;
    let pad_bytes = if pad == 0 { 0 } else { 8 + pad };
    let mut wav = Vec::with_capacity((44 + pad_bytes + data_bytes) as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + pad_bytes + data_bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * frame_bytes).to_le_bytes());
    wav.extend_from_slice(&(frame_bytes as u16).to_le_bytes());
    wav.extend_from_slice(&32u16.to_le_bytes());
    if pad != 0 {
        wav.extend_from_slice(b"PAD ");
        wav.extend_from_slice(&pad.to_le_bytes());
        wav.resize(wav.len() + pad as usize, 0);
    }
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_bytes.to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
}

/// Stereo samples with a short fade at each end.
fn stereo(seconds: f64, amplitude: f64, sample: impl Fn(f64) -> (f64, f64)) -> Vec<f32> {
    let frames = (seconds * f64::from(SAMPLE_RATE)) as u32;
    let mut samples = Vec::with_capacity(frames as usize * 2);
    for frame in 0..frames {
        let time = f64::from(frame) / f64::from(SAMPLE_RATE);
        let fade = (time.min(seconds - time) / FADE_SECONDS).min(1.);
        let (left, right) = sample(time);
        samples.push((left * fade * amplitude) as f32);
        samples.push((right * fade * amplitude) as f32);
    }
    samples
}

/// Phase of a linear sweep from `start` to `end` hertz, at `time` of `seconds`.
fn sweep_phase(start: f64, end: f64, time: f64, seconds: f64) -> f64 {
    TAU * (start * time + (end - start) * time * time / (2. * seconds))
}

impl Content {
    fn bytes(&self, base: &Path) -> io::Result<Vec<u8>> {
        Ok(match *self {
            Content::Tone {
                hz,
                seconds,
                amplitude,
            } => wav_file(
                2,
                0,
                &stereo(seconds, amplitude, |time| {
                    let value = (TAU * hz * time).sin();
                    (value, value)
                }),
            ),
            Content::Sweep {
                low_hz,
                high_hz,
                seconds,
                amplitude,
            } => wav_file(
                2,
                0,
                &stereo(seconds, amplitude, |time| {
                    (
                        sweep_phase(low_hz, high_hz, time, seconds).sin(),
                        sweep_phase(high_hz, low_hz, time, seconds).sin(),
                    )
                }),
            ),
            Content::Levels { hz, ref steps } => {
                let mut samples = Vec::new();
                for step in steps {
                    samples.extend(stereo(step.seconds, 1., |time| {
                        let value = (TAU * hz * time).sin();
                        (value * step.left, value * step.right)
                    }));
                }
                wav_file(2, 0, &samples)
            }
            Content::Copy(ref path) => fs::read(base.join(path))?,
            Content::Text(ref text) => text.clone().into_bytes(),
        })
    }
}

/// The firmware's project settings as it writes them for a new recording.
fn project_settings() -> Vec<u8> {
    let mut bytes = vec![0; 3388];
    bytes[..47].copy_from_slice(format!("{:<47}", "ZOOM L6MAX   PROJECT DATA VER0001").as_bytes());
    const VALUES: [u8; 34] = [
        0x32, 0, 0x28, 0, 0x32, 0, 0x32, 0, 0x5a, 0, 0x1e, 0, 0xf8, 0x01, 0x23, 0, 0xf8, 0x01,
        0x32, 0, 0, 0, 0, 0, 0, 0, 0x20, 0x03, 6, 6, 6, 6, 6, 6,
    ];
    for base in [0, 0x268] {
        for strip in 0..12 {
            let offset = base + strip * 0x18;
            bytes[offset + 0x32..offset + 0x39].fill(0x40);
            bytes[offset + 0x46..offset + 0x48].fill(1);
        }
        bytes[base + 0x160..base + 0x160 + VALUES.len()].copy_from_slice(&VALUES);
    }
    bytes[0x15e] = 5;
    bytes[0x28c] = 1;
    bytes[0x290] = 1;
    bytes
}

impl Project {
    fn write<D: crate::BlockDevice>(&self, volume: &mut crate::Volume<D>) -> io::Result<()> {
        const PAD_BYTES: u32 = 858;
        let folder = format!("RECORDER/{}", self.name);
        create_parents(volume, &format!("{folder}/"))?;
        volume.write(&format!("{folder}/L6MAXPRJDATA.prj"), &project_settings())?;
        let stereo_samples = stereo(self.seconds, self.amplitude, |time| {
            let value = (TAU * self.hz * time).sin();
            (value, value)
        });
        let mono_samples: Vec<f32> = stereo_samples.iter().copied().step_by(2).collect();
        let mono = wav_file(1, PAD_BYTES, &mono_samples);
        let stereo = wav_file(2, PAD_BYTES, &stereo_samples);
        for track in 1..=4 {
            volume.write(&format!("{folder}/TRACK{track:02}_01.wav"), &mono)?;
        }
        for track in 5..=8 {
            volume.write(&format!("{folder}/TRACK{track:02}_ST_01.wav"), &stereo)?;
        }
        volume.write(&format!("{folder}/MASTER_01.wav"), &stereo)
    }
}

impl CardSpec {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let spec: Self = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if spec.version != 1 {
            return Err(format!("{}: card spec needs version = 1", path.display()));
        }
        spec.model()?;
        crate::pad_settings::validate(&spec.sound_pad)?;
        if !spec.project.is_empty() && spec.model != "l6max" {
            return Err(format!("{}: project requires model l6max", path.display()));
        }
        for name in spec
            .directory
            .iter()
            .chain(spec.file.iter().map(|file| &file.path))
            .chain(spec.project.iter().map(|project| &project.name))
        {
            if name.starts_with('/')
                || name
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")
            {
                return Err(format!(
                    "{}: {name} needs a relative card path",
                    path.display()
                ));
            }
        }
        Ok(spec)
    }

    fn model(&self) -> Result<l6fw::Model, String> {
        match self.model.as_str() {
            "l6" => Ok(l6fw::Model::L6),
            "l6max" => Ok(l6fw::Model::L6max),
            other => Err(format!("unknown card spec model {other}")),
        }
    }

    /// Build the card. `base` resolves `copy` paths.
    pub fn build(&self, card: &Path, base: &Path) -> io::Result<()> {
        let model = self.model().map_err(io::Error::other)?;
        crate::publish(card, "L6MAX", |volume| {
            if self.system_file {
                crate::write_system_file(volume, model)?;
            }
            for directory in &self.directory {
                create_parents(volume, &format!("{directory}/"))?;
            }
            for file in &self.file {
                create_parents(volume, &file.path)?;
                volume.write(&file.path, &file.content.bytes(base)?)?;
            }
            for project in &self.project {
                project.write(volume)?;
            }
            if !self.sound_pad.is_empty() {
                crate::pad_settings::apply_to(volume, model, &self.sound_pad)
                    .map_err(io::Error::other)?;
            }
            Ok(())
        })
    }
}

pub(crate) fn create_parents<D: crate::BlockDevice>(
    volume: &mut crate::Volume<D>,
    path: &str,
) -> io::Result<()> {
    for (end, _) in path.match_indices('/') {
        if !volume.exists(&path[..end]) {
            volume.create_dir(&path[..end])?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_builds_directories_generated_audio_and_the_system_file() -> Result<(), String> {
        let dir = std::env::temp_dir().join(format!("l6-card-spec-{}", std::process::id()));
        fs::create_dir(&dir).map_err(|e| e.to_string())?;
        let result = (|| {
            fs::write(dir.join("asset.bin"), b"asset").map_err(|e| e.to_string())?;
            let spec_path = dir.join("card.toml");
            fs::write(
                &spec_path,
                r#"
version = 1
model = "l6"
directory = ["RECORDER"]

[[file]]
path = "SOUND_PAD/PAD1/SWEEP.WAV"
sweep = { low_hz = 100.0, high_hz = 8000.0, seconds = 0.5, amplitude = 0.5 }

[[file]]
path = "SOUND_PAD/PAD2/TONE.WAV"
tone = { hz = 440.0, seconds = 0.25, amplitude = 0.5 }

[[file]]
path = "ASSET.BIN"
copy = "asset.bin"

[[file]]
path = "NOTE.TXT"
text = "hello"

[[sound_pad]]
pad = 1
file = "SOUND_PAD/PAD1/SWEEP.WAV"
mode = "loop"
level = 33
"#,
            )
            .map_err(|e| e.to_string())?;
            let card = dir.join("card.img");
            CardSpec::load(&spec_path)?
                .build(&card, &dir)
                .map_err(|e| e.to_string())?;
            let mut volume = crate::open(&card).map_err(|e| e.to_string())?;
            assert!(volume.exists("RECORDER"));
            assert!(volume.exists("ZOOM_L6.SYS"));
            let settings = volume.read("SOUND_PAD/L6PADSETTING.ZST").unwrap();
            assert_eq!([settings[0x88c], settings[0x890]], [1, 33]);
            assert_eq!(volume.read("ASSET.BIN").unwrap(), b"asset");
            assert_eq!(volume.read("NOTE.TXT").unwrap(), b"hello");
            let sweep = volume.read("SOUND_PAD/PAD1/SWEEP.WAV").unwrap();
            assert_eq!(sweep.len(), 44 + 24000 * 8);
            let tone = volume.read("SOUND_PAD/PAD2/TONE.WAV").unwrap();
            assert_eq!(tone.len(), 44 + 12000 * 8);
            let left = f32::from_le_bytes(sweep[44 + 8000..44 + 8004].try_into().unwrap());
            let right = f32::from_le_bytes(sweep[44 + 8004..44 + 8008].try_into().unwrap());
            assert_ne!(left, right);
            Ok(())
        })();
        fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
        result
    }

    #[test]
    fn levels_hold_each_channel_and_projects_match_the_firmware_layout() -> Result<(), String> {
        let dir = std::env::temp_dir().join(format!("l6-card-levels-{}", std::process::id()));
        fs::create_dir(&dir).map_err(|e| e.to_string())?;
        let result = (|| {
            let spec_path = dir.join("card.toml");
            fs::write(
                &spec_path,
                r#"
version = 1
model = "l6max"

[[file]]
path = "LEVELS.WAV"
levels = { hz = 1000.0, steps = [
  { seconds = 0.5, left = 0.5, right = 0.0 },
  { seconds = 0.5, left = 0.0, right = 0.25 },
] }

[[project]]
name = "260101_000000"
hz = 440.0
seconds = 0.1
amplitude = 0.25
"#,
            )
            .map_err(|e| e.to_string())?;
            let card = dir.join("card.img");
            CardSpec::load(&spec_path)?
                .build(&card, &dir)
                .map_err(|e| e.to_string())?;
            let mut volume = crate::open(&card).map_err(|e| e.to_string())?;
            let levels = volume.read("LEVELS.WAV").unwrap();
            let peak = |channel: usize, range: std::ops::Range<usize>| {
                levels[44..]
                    .chunks_exact(8)
                    .skip(range.start)
                    .take(range.len())
                    .map(|frame| {
                        f32::from_le_bytes(frame[channel * 4..channel * 4 + 4].try_into().unwrap())
                            .abs()
                    })
                    .fold(0f32, f32::max)
            };
            assert!((peak(0, 0..24000) - 0.5).abs() < 0.01);
            assert_eq!(peak(1, 0..24000), 0.);
            assert_eq!(peak(0, 24000..48000), 0.);
            assert!((peak(1, 24000..48000) - 0.25).abs() < 0.01);
            let folder = "RECORDER/260101_000000";
            let settings = volume.read(&format!("{folder}/L6MAXPRJDATA.prj")).unwrap();
            assert_eq!(settings.len(), 3388);
            assert!(settings.starts_with(b"ZOOM L6MAX   PROJECT DATA VER0001"));
            assert_eq!(settings.iter().filter(|byte| **byte != 0).count(), 306);
            let names = volume.entries(folder).unwrap();
            assert_eq!(names.len(), 10);
            let track = volume.read(&format!("{folder}/TRACK01_01.wav")).unwrap();
            assert_eq!(&track[36..40], b"PAD ");
            assert_eq!(&track[902..906], b"data");
            assert_eq!(u16::from_le_bytes([track[22], track[23]]), 1);
            let master = volume.read(&format!("{folder}/MASTER_01.wav")).unwrap();
            assert_eq!(u16::from_le_bytes([master[22], master[23]]), 2);
            assert_eq!(master.len(), 910 + 4800 * 8);
            Ok(())
        })();
        fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
        result
    }

    #[test]
    fn spec_can_omit_the_system_file_and_rejects_bad_input() -> Result<(), String> {
        let dir = std::env::temp_dir().join(format!("l6-card-spec-bad-{}", std::process::id()));
        fs::create_dir(&dir).map_err(|e| e.to_string())?;
        let result = (|| {
            let spec_path = dir.join("card.toml");
            let write = |text: &str| fs::write(&spec_path, text).map_err(|e| e.to_string());
            write("version = 1\nmodel = \"l6max\"\nsystem_file = false\n")?;
            let card = dir.join("card.img");
            CardSpec::load(&spec_path)?
                .build(&card, &dir)
                .map_err(|e| e.to_string())?;
            let mut volume = crate::open(&card).map_err(|e| e.to_string())?;
            assert!(!volume.exists("ZOOM_L6max.sys"));
            write("version = 1\nmodel = \"other\"\n")?;
            assert!(CardSpec::load(&spec_path).is_err());
            write("version = 1\nmodel = \"l6\"\ndirectory = [\"../OUT\"]\n")?;
            assert!(CardSpec::load(&spec_path).is_err());
            Ok(())
        })();
        fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
        result
    }
}
