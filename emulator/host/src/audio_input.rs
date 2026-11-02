//! Prepares audio files for the emulated input channels. QEMU maps each
//! channel's file directly, so assigning one never stalls the guest.
use std::{
    collections::BTreeMap,
    f64::consts::PI,
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

const RATE: u32 = 48000;
/// Sinc lobes on each side of a resampled sample.
const LOBES: usize = 16;
/// Kernel positions stored between two input samples.
const PHASES: usize = 512;

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// Where a channel's prepared audio lives in the input directory.
pub fn channel_path(directory: &Path, channel: u32) -> PathBuf {
    directory.join(format!("channel-{channel}.f32"))
}

fn stamp_path(directory: &Path, channel: u32) -> PathBuf {
    directory.join(format!("channel-{channel}.source"))
}

/// Stereo frames from a PCM or float WAV; mono is copied to both sides and
/// further channels are ignored.
fn decode_wav(bytes: &[u8]) -> io::Result<(u32, Vec<[f32; 2]>)> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(invalid("not a WAV file"));
    }
    let mut format = None;
    let mut offset = 12;
    while offset + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let body = offset + 8;
        let end = body.saturating_add(size).min(bytes.len());
        match &bytes[offset..offset + 4] {
            b"fmt " if end - body >= 16 => {
                let short =
                    |at: usize| u16::from_le_bytes([bytes[body + at], bytes[body + at + 1]]);
                let mut tag = short(0);
                // WAVE_FORMAT_EXTENSIBLE keeps the real tag at the start of its GUID.
                if tag == 0xfffe && end - body >= 26 {
                    tag = short(24);
                }
                let rate = u32::from_le_bytes(bytes[body + 4..body + 8].try_into().unwrap());
                format = Some((tag, short(2) as usize, rate, short(14)));
            }
            b"data" => {
                let (tag, channels, rate, bits) =
                    format.ok_or_else(|| invalid("WAV data precedes its format"))?;
                if channels == 0 || rate == 0 {
                    return Err(invalid("WAV has no channels or sample rate"));
                }
                let width = usize::from(bits / 8);
                let sample = |s: &[u8]| -> f32 {
                    match (tag, bits) {
                        (1, 16) => f32::from(i16::from_le_bytes([s[0], s[1]])) / 32768.,
                        (1, 24) => {
                            (i32::from_le_bytes([0, s[0], s[1], s[2]]) as f64 / 2147483648.) as f32
                        }
                        (1, 32) => {
                            (f64::from(i32::from_le_bytes(s.try_into().unwrap())) / 2147483648.)
                                as f32
                        }
                        (3, 32) => f32::from_le_bytes(s.try_into().unwrap()),
                        (3, 64) => f64::from_le_bytes(s.try_into().unwrap()) as f32,
                        _ => 0.,
                    }
                };
                if !matches!((tag, bits), (1, 16 | 24 | 32) | (3, 32 | 64)) {
                    return Err(invalid(format!(
                        "unsupported WAV format {tag} with {bits} bits"
                    )));
                }
                let frames = bytes[body..end]
                    .chunks_exact(width * channels)
                    .map(|frame| {
                        let right = if channels > 1 { width } else { 0 };
                        [
                            sample(&frame[..width]),
                            sample(&frame[right..right + width]),
                        ]
                    })
                    .collect();
                return Ok((rate, frames));
            }
            _ => {}
        }
        offset = body.saturating_add(size).saturating_add(size & 1);
    }
    Err(invalid("WAV has no data"))
}

/// Windowed-sinc conversion to 48 kHz, band limited when the source is faster.
fn resample(rate: u32, frames: Vec<[f32; 2]>, progress: &dyn Fn(f32)) -> Vec<[f32; 2]> {
    if rate == RATE {
        return frames;
    }
    let cutoff = (f64::from(RATE) / f64::from(rate)).min(1.);
    let radius = (LOBES as f64 / cutoff).ceil() as usize;
    let span = 2 * radius;
    let kernel: Vec<f32> = (0..=PHASES)
        .flat_map(|phase| {
            let phase = phase as f64 / PHASES as f64;
            (0..span).map(move |tap| {
                let x = tap as f64 + 1. - radius as f64 - phase;
                let sinc = if x == 0. {
                    1.
                } else {
                    (PI * cutoff * x).sin() / (PI * cutoff * x)
                };
                let window = 0.5 + 0.5 * (PI * x / radius as f64).cos();
                (cutoff * sinc * window) as f32
            })
        })
        .collect();
    let length = (frames.len() as u64 * u64::from(RATE) / u64::from(rate)) as usize;
    let step = f64::from(rate) / f64::from(RATE);
    (0..length)
        .map(|index| {
            if index % 4096 == 0 {
                progress(index as f32 / length as f32);
            }
            let position = index as f64 * step;
            let whole = position.floor();
            let table = (position - whole) * PHASES as f64;
            let phase = table as usize;
            let blend = (table - phase as f64) as f32;
            let start = whole as isize + 1 - radius as isize;
            let mut output = [0f32; 2];
            for tap in 0..span {
                let at = start + tap as isize;
                if at < 0 || at as usize >= frames.len() {
                    continue;
                }
                let gain = kernel[phase * span + tap] * (1. - blend)
                    + kernel[(phase + 1) * span + tap] * blend;
                output[0] += frames[at as usize][0] * gain;
                output[1] += frames[at as usize][1] * gain;
            }
            output
        })
        .collect()
}

/// Convert WAV bytes for one channel and publish them for QEMU to map.
pub fn prepare_bytes(directory: &Path, channel: u32, bytes: &[u8]) -> io::Result<()> {
    prepare_with_progress(directory, channel, bytes, &|_| {})
}

fn prepare_with_progress(
    directory: &Path,
    channel: u32,
    bytes: &[u8],
    progress: &dyn Fn(f32),
) -> io::Result<()> {
    let (rate, frames) = decode_wav(bytes)?;
    let frames = resample(rate, frames, progress);
    if frames.is_empty() {
        return Err(invalid("WAV has no audio"));
    }
    fs::create_dir_all(directory)?;
    remove_if_present(&stamp_path(directory, channel))?;
    let path = channel_path(directory, channel);
    let partial = path.with_extension("partial");
    let raw: Vec<u8> = frames
        .iter()
        .flatten()
        .flat_map(|sample| sample.to_le_bytes())
        .collect();
    fs::write(&partial, raw)?;
    // QEMU may still map the previous file; replacing it leaves that mapping valid.
    fs::rename(partial, path)
}

/// Convert a WAV file for one channel. See [`prepare_bytes`].
pub fn prepare(directory: &Path, channel: u32, source: &Path) -> io::Result<()> {
    prepare_bytes(directory, channel, &fs::read(source)?)
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Remove a channel's prepared audio.
pub fn clear(directory: &Path, channel: u32) -> io::Result<()> {
    remove_if_present(&stamp_path(directory, channel))?;
    remove_if_present(&channel_path(directory, channel))
}

fn source_stamp(file: &Path) -> io::Result<String> {
    let metadata = fs::metadata(file)?;
    Ok(format!(
        "{}\n{}\n{:?}",
        file.display(),
        metadata.len(),
        metadata.modified().ok()
    ))
}

fn is_current(directory: &Path, channel: u32, stamp: &str) -> bool {
    channel_path(directory, channel).is_file()
        && fs::read_to_string(stamp_path(directory, channel)).is_ok_and(|current| current == stamp)
}

/// The length of the longest prepared channel, in milliseconds.
pub fn length_ms(directory: &Path, channels: u32) -> u32 {
    (1..=channels)
        .filter_map(|channel| fs::metadata(channel_path(directory, channel)).ok())
        .map(|metadata| metadata.len() / 8 * 1000 / u64::from(RATE))
        .max()
        .unwrap_or(0) as u32
}

/// Make the channels hold exactly this patch, converting only files that changed.
pub fn apply_patch(
    directory: &Path,
    channels: u32,
    patch: &BTreeMap<u32, PathBuf>,
) -> Result<(), String> {
    apply_patch_with_progress(directory, channels, patch, &|_, _| {})
}

/// Remove prepared audio the patch no longer uses, and anything an interrupted
/// conversion left behind. A mapped file stays readable until QEMU drops it.
fn remove_unassigned(
    directory: &Path,
    channels: u32,
    patch: &BTreeMap<u32, PathBuf>,
) -> io::Result<()> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let kept: Vec<PathBuf> = (1..=channels)
        .filter(|channel| patch.contains_key(channel))
        .flat_map(|channel| {
            [
                channel_path(directory, channel),
                stamp_path(directory, channel),
            ]
        })
        .collect();
    for entry in entries {
        let path = entry?.path();
        let ours = path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("channel-"));
        if ours && !kept.contains(&path) {
            remove_if_present(&path)?;
        }
    }
    Ok(())
}

/// The patch's channels whose prepared audio is missing or out of date.
pub fn stale_channels(
    directory: &Path,
    patch: &BTreeMap<u32, PathBuf>,
) -> Result<Vec<u32>, String> {
    let mut stale = Vec::new();
    for (channel, file) in patch {
        let stamp = source_stamp(file).map_err(|e| format!("{}: {e}", file.display()))?;
        if !is_current(directory, *channel, &stamp) {
            stale.push(*channel);
        }
    }
    Ok(stale)
}

/// [`apply_patch`], reporting each channel's progress from 0 to 1. Files convert in
/// parallel on half of the cores, which leaves room for a running guest.
pub fn apply_patch_with_progress(
    directory: &Path,
    channels: u32,
    patch: &BTreeMap<u32, PathBuf>,
    report: &(dyn Fn(u32, f32) + Sync),
) -> Result<(), String> {
    remove_unassigned(directory, channels, patch).map_err(|e| e.to_string())?;
    let stale = stale_channels(directory, patch)?;
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism()
        .map_or(1, |cores| cores.get() / 2)
        .clamp(1, stale.len().max(1));
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| -> Result<(), String> {
                    while let Some(&channel) = stale.get(next.fetch_add(1, Ordering::Relaxed)) {
                        let file = &patch[&channel];
                        let describe = |e: io::Error| format!("{}: {e}", file.display());
                        let stamp = source_stamp(file).map_err(describe)?;
                        report(channel, 0.);
                        let bytes = fs::read(file).map_err(describe)?;
                        prepare_with_progress(directory, channel, &bytes, &|fraction| {
                            report(channel, fraction)
                        })
                        .map_err(describe)?;
                        fs::write(stamp_path(directory, channel), stamp)
                            .map_err(|e| e.to_string())?;
                        report(channel, 1.);
                    }
                    Ok(())
                })
            })
            .collect();
        handles.into_iter().try_for_each(|handle| {
            handle
                .join()
                .map_err(|_| "conversion panicked".to_owned())?
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(tag: u16, channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
        let mut bytes = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0".to_vec();
        bytes.extend(tag.to_le_bytes());
        bytes.extend(channels.to_le_bytes());
        bytes.extend(rate.to_le_bytes());
        bytes.extend((rate * u32::from(channels) * u32::from(bits) / 8).to_le_bytes());
        bytes.extend((channels * bits / 8).to_le_bytes());
        bytes.extend(bits.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend((data.len() as u32).to_le_bytes());
        bytes.extend(data);
        bytes
    }

    #[test]
    fn decodes_common_formats() {
        let (rate, frames) = decode_wav(&wav(1, 1, 44100, 16, &[0, 0x40])).unwrap();
        assert_eq!((rate, frames), (44100, vec![[0.5, 0.5]]));
        let (_, frames) = decode_wav(&wav(1, 2, 48000, 24, &[0, 0, 0x40, 0, 0, 0xc0])).unwrap();
        assert_eq!(frames, [[0.5, -0.5]]);
        let float: Vec<u8> = [0.25f32, -0.75]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let (_, frames) = decode_wav(&wav(3, 2, 48000, 32, &float)).unwrap();
        assert_eq!(frames, [[0.25, -0.75]]);
        assert!(decode_wav(&wav(1, 1, 48000, 8, &[0])).is_err());
        assert!(decode_wav(b"not audio").is_err());
    }

    #[test]
    fn resampling_keeps_a_tone_and_its_duration() {
        let source: Vec<[f32; 2]> = (0..44100)
            .map(|n| {
                let value = (2. * PI * 1000. * n as f64 / 44100.).sin() as f32 * 0.5;
                [value, -value]
            })
            .collect();
        let output = resample(44100, source, &|_| {});
        assert_eq!(output.len(), 48000);
        for (n, frame) in output.iter().enumerate().skip(100).take(47800) {
            let expected = (2. * PI * 1000. * n as f64 / 48000.).sin() as f32 * 0.5;
            assert!((frame[0] - expected).abs() < 1e-3, "{n}: {}", frame[0]);
            assert!((frame[1] + expected).abs() < 1e-3, "{n}: {}", frame[1]);
        }
    }

    #[test]
    fn prepared_files_replace_and_clear() {
        let directory = std::env::temp_dir().join(format!("l6-audio-input-{}", std::process::id()));
        let float: Vec<u8> = [0.25f32, -0.75]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        prepare_bytes(&directory, 3, &wav(3, 2, 48000, 32, &float)).unwrap();
        assert_eq!(fs::read(channel_path(&directory, 3)).unwrap(), float);
        clear(&directory, 3).unwrap();
        clear(&directory, 3).unwrap();
        assert!(!channel_path(&directory, 3).exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_patch_converts_changed_files_and_drops_other_channels() {
        let directory = std::env::temp_dir().join(format!("l6-audio-patch-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let source = directory.join("source.wav");
        let float: Vec<u8> = [0.25f32, -0.75]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        fs::write(&source, wav(3, 2, 48000, 32, &float)).unwrap();
        prepare(&directory, 2, &source).unwrap();
        let partial = channel_path(&directory, 3).with_extension("partial");
        fs::write(&partial, b"interrupted").unwrap();
        let unrelated = directory.join("notes.txt");
        fs::write(&unrelated, b"not ours").unwrap();
        let patch = BTreeMap::from([(1, source.clone())]);
        apply_patch(&directory, 8, &patch).unwrap();
        assert!(!partial.exists());
        assert!(unrelated.exists());
        assert_eq!(fs::read(channel_path(&directory, 1)).unwrap(), float);
        assert!(!channel_path(&directory, 2).exists());
        // An unchanged source is not converted again.
        fs::write(channel_path(&directory, 1), b"kept").unwrap();
        apply_patch(&directory, 8, &patch).unwrap();
        assert_eq!(fs::read(channel_path(&directory, 1)).unwrap(), b"kept");
        // A file assigned by hand replaces the patch's until the next application.
        prepare(&directory, 1, &source).unwrap();
        apply_patch(&directory, 8, &patch).unwrap();
        assert_eq!(fs::read(channel_path(&directory, 1)).unwrap(), float);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_patch_reports_only_the_files_it_converts() {
        let directory =
            std::env::temp_dir().join(format!("l6-audio-progress-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let source = directory.join("source.wav");
        let data = vec![0u8; 8 * 44100];
        fs::write(&source, wav(3, 2, 44100, 32, &data)).unwrap();
        let patch = BTreeMap::from([(1, source.clone()), (3, source)]);
        assert_eq!(stale_channels(&directory, &patch).unwrap(), [1, 3]);
        let reports = std::sync::Mutex::new(Vec::new());
        apply_patch_with_progress(&directory, 8, &patch, &|channel, fraction| {
            reports.lock().unwrap().push((channel, fraction))
        })
        .unwrap();
        let reports = reports.into_inner().unwrap();
        for channel in [1, 3] {
            let fractions: Vec<f32> = reports
                .iter()
                .filter(|report| report.0 == channel)
                .map(|report| report.1)
                .collect();
            assert_eq!((fractions[0], *fractions.last().unwrap()), (0., 1.));
            assert!(fractions.is_sorted());
        }
        assert!(stale_channels(&directory, &patch).unwrap().is_empty());
        assert_eq!(length_ms(&directory, 8), 1000);
        apply_patch_with_progress(&directory, 8, &patch, &|_, _| panic!("nothing to convert"))
            .unwrap();
        fs::remove_dir_all(directory).unwrap();
    }
}
