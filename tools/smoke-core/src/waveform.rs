//! Compares captured SAI output with the source WAV of a one-shot sound pad.

use serde_json::{Value, json};

/// Decode a 16-bit PCM or 32-bit float WAV; mono is copied to both channels.
pub fn decode_wav(bytes: &[u8]) -> Result<Vec<[f64; 2]>, String> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("source is not a RIFF/WAVE file".into());
    }
    let mut format = None;
    let mut offset = 12;
    while offset + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let body = offset + 8;
        let end = body.saturating_add(size).min(bytes.len());
        match &bytes[offset..offset + 4] {
            b"fmt " if end - body >= 16 => {
                let field =
                    |at: usize| u16::from_le_bytes([bytes[body + at], bytes[body + at + 1]]);
                format = Some((field(0), field(2), field(14)));
            }
            b"data" => {
                let (tag, channels, bits) = format.ok_or("WAV data precedes its format")?;
                if !(1..=2).contains(&channels) {
                    return Err(format!("unsupported WAV channel count {channels}"));
                }
                let channels = usize::from(channels);
                let samples: Vec<f64> = match (tag, bits) {
                    (1, 16) => bytes[body..end]
                        .chunks_exact(2)
                        .map(|s| f64::from(i16::from_le_bytes(s.try_into().unwrap())) / 32768.)
                        .collect(),
                    (3, 32) => bytes[body..end]
                        .chunks_exact(4)
                        .map(|s| f64::from(f32::from_le_bytes(s.try_into().unwrap())))
                        .collect(),
                    _ => return Err(format!("unsupported WAV format {tag}/{bits}")),
                };
                return Ok(samples
                    .chunks_exact(channels)
                    .map(|frame| [frame[0], frame[channels - 1]])
                    .collect());
            }
            _ => {}
        }
        offset = body.saturating_add(size + (size & 1));
    }
    Err("WAV has no data chunk".into())
}

/// Decode raw stereo S32 little-endian frames.
pub fn decode_capture(bytes: &[u8]) -> Vec<[f64; 2]> {
    bytes
        .chunks_exact(8)
        .map(|frame| {
            std::array::from_fn(|channel| {
                let sample = &frame[channel * 4..channel * 4 + 4];
                f64::from(i32::from_le_bytes(sample.try_into().unwrap())) / 2147483648.
            })
        })
        .collect()
}

/// Worst-channel error, in dB relative to the signal, after fitting one
/// constant gain per channel. Gain and polarity are firmware processing;
/// anything else that differs from the source remains in the residual.
fn fit(source: &[[f64; 2]], capture: &[[f64; 2]]) -> ([f64; 2], f64) {
    let mut gains = [0.; 2];
    let mut worst = f64::NEG_INFINITY;
    for channel in 0..2 {
        let (mut cross, mut energy, mut captured) = (0., 0., 0.);
        for (s, c) in source.iter().zip(capture) {
            cross += s[channel] * c[channel];
            energy += s[channel] * s[channel];
            captured += c[channel] * c[channel];
        }
        if energy == 0. {
            continue;
        }
        let gain = cross / energy;
        let signal = gain * gain * energy;
        let error = (captured - signal).max(0.);
        gains[channel] = gain;
        worst = worst.max(if signal == 0. {
            f64::INFINITY
        } else {
            10. * (error.max(1e-30) / signal).log10()
        });
    }
    (gains, worst)
}

/// Locate the complete source in the capture and report how closely it fits.
pub fn compare(source: &[[f64; 2]], capture: &[[f64; 2]]) -> Result<Value, String> {
    const SEARCH: isize = 64;
    // Idle output can carry low-level firmware dither, so find the signal
    // relative to each stream's own peak rather than the first nonzero word.
    let span = |frames: &[[f64; 2]]| -> Option<(usize, usize)> {
        let peak = frames
            .iter()
            .flatten()
            .fold(0., |peak: f64, v| peak.max(v.abs()));
        let audible = |frame: &[f64; 2]| frame.iter().any(|v| v.abs() > peak * 1e-3);
        Some((
            frames.iter().position(audible)?,
            frames.iter().rposition(audible)? + 1,
        ))
    };
    let (source_start, _) = span(source).ok_or("source is silent")?;
    let (capture_start, capture_end) = span(capture).ok_or("capture is silent")?;
    let nominal = capture_start as isize - source_start as isize;
    let (mut best_offset, mut best_gains, mut best_residual) = (0, [0.; 2], f64::INFINITY);
    for offset in nominal - SEARCH..=nominal + SEARCH {
        if offset < 0 || offset as usize + source.len() > capture.len() {
            continue;
        }
        let window = &capture[offset as usize..offset as usize + source.len()];
        let (gains, residual) = fit(source, window);
        if residual < best_residual {
            (best_offset, best_gains, best_residual) = (offset as usize, gains, residual);
        }
    }
    if !best_residual.is_finite() {
        return Err(format!(
            "capture of {} frames does not contain the {}-frame source at frame {capture_start}",
            capture.len(),
            source.len()
        ));
    }
    Ok(json!({
        "source_frames": source.len(),
        "audible_capture_frames": capture_end - capture_start,
        "offset_frames": best_offset,
        "gains": best_gains,
        "residual_db": best_residual,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> Vec<[f64; 2]> {
        (1..4800)
            .map(|i| {
                let t = i as f64 / 48000.;
                let left = (std::f64::consts::TAU * (300. + 40000. * t) * t).sin();
                [left * 0.7, (std::f64::consts::TAU * 733. * t).sin() * 0.5]
            })
            .collect()
    }

    fn captured(source: &[[f64; 2]], gain: f64) -> Vec<[f64; 2]> {
        let mut capture = vec![[1e-9; 2]; 100];
        capture.extend(source.iter().map(|f| [f[0] * gain, f[1] * gain]));
        capture.extend([[0.; 2]; 100]);
        capture
    }

    fn residual(source: &[[f64; 2]], capture: &[[f64; 2]]) -> f64 {
        compare(source, capture).unwrap()["residual_db"]
            .as_f64()
            .unwrap()
    }

    #[test]
    fn inverted_scaled_copy_matches() {
        let source = source();
        let report = compare(&source, &captured(&source, -0.35)).unwrap();
        assert_eq!(report["offset_frames"], 100);
        assert!(report["residual_db"].as_f64().unwrap() < -120.);
    }

    #[test]
    fn one_stale_frame_per_block_is_detected() {
        let source = source();
        let mut capture = captured(&source, 0.5);
        for frame in (228..capture.len() - 100).step_by(64) {
            capture[frame] = capture[frame - 128];
        }
        assert!(residual(&source, &capture) > -30.);
    }

    #[test]
    fn slowed_playback_does_not_fit() {
        let source = source();
        let slowed: Vec<_> = source.iter().flat_map(|f| [*f; 4]).collect();
        assert!(residual(&source, &slowed) > -10.);
    }

    #[test]
    fn truncated_capture_is_rejected() {
        let source = source();
        assert!(compare(&source, &captured(&source, 1.)[..2000]).is_err());
    }

    #[test]
    fn decodes_wav_formats() {
        let wav = |tag: u16, channels: u16, bits: u16, data: &[u8]| {
            let mut w = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0".to_vec();
            w.extend(tag.to_le_bytes());
            w.extend(channels.to_le_bytes());
            w.extend([0; 10]);
            w.extend(bits.to_le_bytes());
            w.extend(b"data");
            w.extend((data.len() as u32).to_le_bytes());
            w.extend(data);
            w
        };
        assert_eq!(
            decode_wav(&wav(1, 1, 16, &[0, 0x40])).unwrap(),
            [[0.5, 0.5]]
        );
        let float: Vec<u8> = [0.25f32, -0.5]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(decode_wav(&wav(3, 2, 32, &float)).unwrap(), [[0.25, -0.5]]);
        assert!(decode_wav(&wav(1, 2, 24, &[0; 6])).is_err());
    }
}
