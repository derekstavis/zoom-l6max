//! Experimental streaming 48 kHz ↔ 32 kHz Plateau rate converter.
//!
//! The caller owns this state and invokes it once per 48 kHz frame. This does
//! not establish a firmware allocation, return alignment, or target deadline.

use super::math::audio_input;

mod coefficients {
    include!("plateau_rate_coeffs.rs");
}

const TAPS: usize = 64;

pub(crate) struct PlateauRate {
    input: [[f32; TAPS]; 2],
    output: [[f32; TAPS]; 2],
    input_write: usize,
    output_write: usize,
    phase: usize,
}

impl PlateauRate {
    pub(crate) const fn new() -> Self {
        Self {
            input: [[0.0; TAPS]; 2],
            output: [[0.0; TAPS]; 2],
            input_write: 0,
            output_write: 0,
            phase: 0,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.input = [[0.0; TAPS]; 2];
        self.output = [[0.0; TAPS]; 2];
        self.input_write = 0;
        self.output_write = 0;
        self.phase = 0;
    }

    pub(crate) fn clear_input(&mut self) {
        self.input = [[0.0; TAPS]; 2];
    }

    #[inline(always)]
    fn filter(history: &[[f32; TAPS]; 2], write: usize, coefficients: &[f32; TAPS]) -> (f32, f32) {
        let mut left = 0.0;
        let mut right = 0.0;
        for (i, &coefficient) in coefficients.iter().enumerate() {
            let index = write.wrapping_sub(i) & (TAPS - 1);
            left += history[0][index] * coefficient;
            right += history[1][index] * coefficient;
        }
        (left, right)
    }

    /// Process one host frame, calling `internal` on exactly two of every
    /// three frames. A damaged cursor fails silent until `clear`.
    pub(crate) fn process(
        &mut self,
        left: f32,
        right: f32,
        mut internal: impl FnMut(f32, f32) -> (f32, f32),
    ) -> (f32, f32) {
        if self.input_write >= TAPS || self.output_write >= TAPS || self.phase >= 3 {
            return (0.0, 0.0);
        }
        self.input_write = (self.input_write + 1) & (TAPS - 1);
        self.input[0][self.input_write] = audio_input(left);
        self.input[1][self.input_write] = audio_input(right);
        self.phase += 2;
        if self.phase >= 3 {
            self.phase -= 3;
            let (left, right) = Self::filter(
                &self.input,
                self.input_write,
                &coefficients::DOWN[self.phase],
            );
            let (left, right) = internal(left, right);
            self.output_write = (self.output_write + 1) & (TAPS - 1);
            self.output[0][self.output_write] = audio_input(left);
            self.output[1][self.output_write] = audio_input(right);
        }
        Self::filter(
            &self.output,
            self.output_write,
            &coefficients::UP[2 - self.phase],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::PlateauRate;

    #[test]
    fn rate_count_dc_reset_and_invalid_state() {
        let mut rate = PlateauRate::new();
        let mut calls = 0;
        for frame in 0..48_000 {
            let (left, right) = rate.process(0.25, -0.125, |l, r| {
                calls += 1;
                (l, r)
            });
            if frame > 1_000 {
                assert!((left - 0.25).abs() < 1e-6);
                assert!((right + 0.125).abs() < 1e-6);
            }
        }
        assert_eq!(calls, 32_000);
        rate.phase = 3;
        assert_eq!(
            rate.process(1.0, 1.0, |_, _| panic!("invalid state invoked core")),
            (0.0, 0.0)
        );
        rate.clear();
        assert_eq!(rate.process(0.0, 0.0, |l, r| (l, r)), (0.0, 0.0));
    }

    #[test]
    fn impulse_alignment_and_channel_isolation() {
        for phase in 0..3 {
            let mut rate = PlateauRate::new();
            let mut peak = (0, 0.0f32);
            let mut sum = 0.0;
            for frame in 0..300 {
                let input = if frame == phase { 1.0 } else { 0.0 };
                let (left, right) = rate.process(input, 0.0, |l, r| (l, r));
                assert_eq!(right, 0.0);
                if left.abs() > peak.1 {
                    peak = (frame, left.abs());
                }
                sum += left;
            }
            assert_eq!(peak.0 - phase, 79);
            assert!((sum - 1.0f32).abs() < 0.0001);
        }
    }

    #[test]
    fn sampled_passband_and_alias_rejection() {
        for frequency in [
            100.0f64, 1_000.0, 6_000.0, 12_000.0, 16_000.0, 17_000.0, 19_000.0, 23_000.0,
        ] {
            let mut rate = PlateauRate::new();
            let mut energy_in = 0.0f64;
            let mut energy_out = 0.0f64;
            for frame in 0..24_000 {
                let input =
                    (2.0 * std::f64::consts::PI * frequency * frame as f64 / 48_000.0).sin() as f32;
                let (left, right) = rate.process(input, 0.0, |l, r| (l, r));
                assert_eq!(right, 0.0);
                if frame >= 2_400 {
                    energy_in += f64::from(input) * f64::from(input);
                    energy_out += f64::from(left) * f64::from(left);
                }
            }
            let db = 10.0 * (energy_out / energy_in).log10();
            if frequency <= 12_000.0 {
                assert!(db.abs() < 0.03, "{frequency} Hz: {db} dB");
            } else {
                assert!(db < -65.0, "{frequency} Hz: {db} dB");
            }
        }
    }
}
