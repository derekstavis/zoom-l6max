//! Velvet Noise Decorrelator (VND) DSP Engine
//!
//! Ported and adapted from 115dkk/Velvet-Noise-Decorrelator-VST3 for bare-metal
//! ARM Cortex-M7 target for Zoom LiveTrak L6max. The physical processor
//! model has not been identified.
//!
//! Generates a sparse, unit-energy FIR sequence of pseudo-random impulses
//! to decorrelate stereo audio channels without comb filtering or transient smearing.

use super::math::*;

pub const SAMPLE_RATE: f32 = 48000.0;
pub const MAX_DELAY_SAMPLES: usize = 4802; // Up to 100 ms at 48 kHz
// The Multistomp range reaches 100 ms at 4,000 impulses per second.
pub const MAX_TAPS: usize = 400;
const AMOUNT_FADE_SAMPLES: u32 = 64;

#[cold]
#[inline(never)]
fn wrap_out_of_range_delay(delay: usize) -> usize {
    delay % MAX_DELAY_SAMPLES
}

#[inline(always)]
fn history_index(write_position: usize, delay: usize) -> usize {
    // For in-range positions and delays, add one ring only on underflow.
    write_position
        .wrapping_sub(delay)
        .wrapping_add(MAX_DELAY_SAMPLES & 0usize.wrapping_sub((write_position < delay) as usize))
}

#[derive(Clone, Copy)]
pub struct Tap {
    pub delay: u32,
    pub gain: f32,
}

impl Tap {
    pub const fn zero() -> Self {
        Self {
            delay: 0,
            gain: 0.0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Bank {
    pub storage: [Tap; MAX_TAPS],
    pub count: usize,
}

impl Bank {
    pub const fn new() -> Self {
        Self {
            storage: [Tap::zero(); MAX_TAPS],
            count: 0,
        }
    }
}

/// A separate mono send history using the decorrelator's filter banks.
pub(crate) struct MonoHistory {
    delay_line: [f32; MAX_DELAY_SAMPLES],
    write_position: usize,
    nonzero_samples: usize,
}

impl MonoHistory {
    pub(crate) const fn new() -> Self {
        Self {
            delay_line: [0.0; MAX_DELAY_SAMPLES],
            write_position: 0,
            nonzero_samples: 0,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.delay_line.fill(0.0);
        self.write_position = 0;
        self.nonzero_samples = 0;
    }

    fn write(&mut self, sample: f32) {
        if self.delay_line[self.write_position] != 0.0 {
            self.nonzero_samples -= 1;
        }
        if sample != 0.0 {
            self.nonzero_samples += 1;
        }
        self.delay_line[self.write_position] = sample;
    }

    fn advance(&mut self) {
        self.write_position += 1;
        if self.write_position == MAX_DELAY_SAMPLES {
            self.write_position = 0;
        }
    }

    pub(crate) fn silence(&mut self) {
        // Do not index or advance a damaged history. Reinitialization is
        // required before it can be used again.
        if self.write_position >= MAX_DELAY_SAMPLES {
            return;
        }
        self.write(0.0);
        self.advance();
    }
}

pub struct Channel {
    // Only process_sample/reset may change history, so nonzero_samples stays exact.
    delay_line: [f32; MAX_DELAY_SAMPLES],
    write_position: usize,
    nonzero_samples: usize,
    pub banks: [Bank; 2],
}

// One step generates one tap or normalizes up to eight taps per channel. The
// pending bank is never exposed until both channels have been built.
struct BankBuild {
    bank: usize,
    count: usize,
    index: usize,
    tail: usize,
    phase: u8, // 0 = ready, 1 = generate, 2 = normalize
    random: [u64; 2],
    previous: [u32; 2],
    energy: [f32; 2],
    scale: [f32; 2],
    grid: f32,
    decay_db: f32,
}

impl BankBuild {
    const fn new() -> Self {
        Self {
            bank: 0,
            count: 0,
            index: 0,
            tail: 0,
            phase: 0,
            random: [0; 2],
            previous: [0; 2],
            energy: [0.0; 2],
            scale: [0.0; 2],
            grid: 0.0,
            decay_db: 0.0,
        }
    }
}

impl Channel {
    pub const fn new() -> Self {
        Self {
            delay_line: [0.0; MAX_DELAY_SAMPLES],
            write_position: 0,
            nonzero_samples: 0,
            banks: [Bank::new(), Bank::new()],
        }
    }
}

#[derive(Clone, Copy)]
pub struct Parameters {
    pub dynamic: bool,
    pub amount: f32,          // 0.0 to 1.0 (wet mix)
    pub length_ms: f32,       // 1.0 to 100.0 ms (default ~27.56 ms)
    pub density: f32,         // pulses per second (default ~1088.4)
    pub refresh_seconds: f32, // time between renewals (default 5.0 s)
    pub transition_ms: f32,   // crossfade time (default 250.0 ms)
    pub decay_db: f32,        // end-of-filter decay (default -60.0 dB)
    pub seed: u64,            // deterministic base seed
}

impl Default for Parameters {
    fn default() -> Self {
        Self {
            dynamic: true,
            amount: 1.0,
            length_ms: 27.5625,
            density: 1088.435,
            refresh_seconds: 5.0,
            transition_ms: 250.0,
            decay_db: -60.0,
            seed: 2050083136,
        }
    }
}

/// SplitMix64 PRNG generator.
pub fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e3779b97f4a7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}

/// Generates a signed uniform float in [-1.0, 1.0].
pub fn uniform_signed(state: &mut u64) -> f32 {
    let bits = splitmix64(state) >> 11;
    // The centered integer is exact in f64. Round it once to f32 and scale
    // by a power of two, avoiding software double arithmetic on Cortex-M7.
    ((bits as i64 - (1i64 << 52)) as f32) * (1.0 / 4503599627370496.0)
}

/// Mixes base seed with channel index and generation count.
pub fn mixed_seed(seed: u64, channel: u64, generation: u64) -> u64 {
    let state = seed
        ^ (channel.wrapping_add(1)).wrapping_mul(0xd6e8feb86659fd93)
        ^ (generation.wrapping_add(1)).wrapping_mul(0xa5a3564e27f8862f);
    let mut s = state;
    splitmix64(&mut s)
}

pub struct VelvetDecorrelator {
    pub channels: [Channel; 2], // Stereo (Left, Right)
    pub current: Parameters,
    pub current_tail_samples: usize,
    pub refresh_samples: usize,
    pub transition_samples: usize,
    pub samples_until_transition: usize,
    pub transition_position: usize,
    pub generation: u64,
    pub active_bank: usize,
    pub pending_bank: usize,
    pub transitioning: bool,
    // A filter edit during a crossfade is applied after that crossfade ends.
    pub filter_update_pending: bool,
    pub active_transition_samples: usize,
    transition_phase_step: f32,
    amount_current: f32,
    amount_step: f32,
    amount_remaining: u32,
    build: BankBuild,
}

impl VelvetDecorrelator {
    pub const fn new() -> Self {
        Self {
            channels: [Channel::new(), Channel::new()],
            current: Parameters {
                dynamic: true,
                amount: 1.0,
                length_ms: 27.5625,
                density: 1088.435,
                refresh_seconds: 5.0,
                transition_ms: 250.0,
                decay_db: -60.0,
                seed: 2050083136,
            },
            current_tail_samples: 1323,
            refresh_samples: 240000,
            transition_samples: 12000,
            samples_until_transition: 240000,
            transition_position: 0,
            generation: 2,
            active_bank: 0,
            pending_bank: 1,
            transitioning: false,
            filter_update_pending: false,
            active_transition_samples: 12000,
            transition_phase_step: 0.0,
            amount_current: 1.0,
            amount_step: 0.0,
            amount_remaining: 0,
            build: BankBuild::new(),
        }
    }

    /// Initializes and computes the first set of sparse taps for both channels.
    pub fn init(&mut self, params: Parameters) {
        // Do not crossfade against banks from a previous initialization.
        self.channels[0].banks[0].count = 0;
        self.set_parameters(params);
        self.reset();
    }

    pub fn set_parameters(&mut self, mut params: Parameters) {
        let initialized = self.channels[0].banks[0].count != 0;
        // A rejected edit must not try to rebuild through an invalid bank.
        // Initialization starts with zeroed storage and sets banks in reset.
        if initialized && !self.valid_banks() {
            return;
        }
        params.amount = clamp(params.amount, 0.0, 1.0);
        params.length_ms = clamp(params.length_ms, 1.0, 100.0);
        params.density = clamp(params.density, 10.0, 4000.0);
        params.refresh_seconds = clamp(params.refresh_seconds, 0.1, 60.0);
        params.transition_ms = clamp(params.transition_ms, 1.0, params.refresh_seconds * 900.0);
        params.decay_db = clamp(params.decay_db, -120.0, 0.0);

        let filter_changed = params.length_ms != self.current.length_ms
            || self.tap_count(params.density) != self.tap_count(self.current.density)
            || params.decay_db != self.current.decay_db
            || params.seed != self.current.seed;
        if params.amount != self.current.amount {
            self.amount_step = (params.amount - self.amount_current) / AMOUNT_FADE_SAMPLES as f32;
            self.amount_remaining = AMOUNT_FADE_SAMPLES;
        }
        self.current = params;
        self.current_tail_samples = clamp_usize(
            (SAMPLE_RATE * params.length_ms / 1000.0) as usize,
            1,
            MAX_DELAY_SAMPLES - 2,
        );

        self.refresh_samples = (SAMPLE_RATE * params.refresh_seconds) as usize;
        if self.refresh_samples == 0 {
            self.refresh_samples = 1;
        }

        self.transition_samples = (SAMPLE_RATE * (params.transition_ms / 1000.0)) as usize;
        if self.transition_samples == 0 {
            self.transition_samples = 1;
        }

        self.samples_until_transition = self.samples_until_transition.min(self.refresh_samples);
        if initialized && filter_changed {
            if self.transitioning {
                self.filter_update_pending = true;
            } else {
                self.start_next_build(self.pending_bank);
                self.filter_update_pending = true;
            }
        }
    }

    #[inline(always)]
    fn valid_banks(&self) -> bool {
        self.active_bank < 2 && self.pending_bank < 2 && self.active_bank != self.pending_bank
    }

    /// Clears history and rebuilds both banks for the current parameters.
    /// Bank generation is not suitable for a real-time audio callback.
    pub fn reset(&mut self) {
        self.amount_current = self.current.amount;
        self.amount_step = 0.0;
        self.amount_remaining = 0;
        self.start_build(0, [0, 0]);
        while self.build.phase != 0 {
            self.build_step();
        }
        self.start_build(1, [1, 1]);
        while self.build.phase != 0 {
            self.build_step();
        }
        self.generation = 2;
        for ch in &mut self.channels {
            ch.delay_line.fill(0.0);
            ch.write_position = 0;
            ch.nonzero_samples = 0;
        }
        self.transitioning = false;
        self.transition_position = 0;
        self.samples_until_transition = self.refresh_samples;
        self.active_bank = 0;
        self.pending_bank = 1;
        self.filter_update_pending = false;
        self.active_transition_samples = self.transition_samples;
        self.transition_phase_step = 0.0;
    }

    pub(crate) fn tap_count(&self, density: f32) -> usize {
        clamp_usize(
            (self.current.length_ms / 1000.0 * density + 0.5) as usize,
            2,
            MAX_TAPS.min(self.current_tail_samples + 1),
        )
    }

    fn start_next_build(&mut self, bank: usize) {
        let generation = self.generation;
        self.start_build(bank, [generation, generation.wrapping_add(1)]);
        self.generation = generation.wrapping_add(2);
    }

    fn start_build(&mut self, bank: usize, generations: [u64; 2]) {
        let count = self.tap_count(self.current.density);
        self.build = BankBuild {
            bank,
            count,
            index: 0,
            tail: self.current_tail_samples,
            phase: 1,
            random: [
                mixed_seed(self.current.seed, 0, generations[0]),
                mixed_seed(self.current.seed, 1, generations[1]),
            ],
            previous: [0; 2],
            energy: [0.0; 2],
            scale: [0.0; 2],
            grid: self.current_tail_samples as f32 / (count - 1) as f32,
            decay_db: self.current.decay_db,
        };
        for channel in &mut self.channels {
            channel.banks[bank].count = count;
        }
    }

    // Normalization is cheaper than generating a tap. Eight per step keep a
    // maximum-size bank ready during the shortest gap between timed fades.
    fn build_step(&mut self) {
        let build = &mut self.build;
        let i = build.index;
        if build.phase == 1 {
            for c in 0..2 {
                // The reference starts with a positive impulse at zero delay.
                // It consumes no random numbers for that impulse.
                let delay = if i == 0 {
                    0
                } else {
                    let centre = build.grid * i as f32;
                    let jitter = uniform_signed(&mut build.random[c]) * build.grid * 0.45;
                    let lower = build.previous[c] + 1;
                    let upper = (build.tail - (build.count - 1 - i)) as u32;
                    let proposed = (centre + jitter + 0.5) as i32;
                    if proposed < lower as i32 {
                        lower
                    } else if proposed > upper as i32 {
                        upper
                    } else {
                        proposed as u32
                    }
                };
                build.previous[c] = delay;
                let progress = delay as f32 / build.tail as f32;
                let envelope = pow10(build.decay_db * progress / 20.0);
                let sign = if i == 0 || splitmix64(&mut build.random[c]) & 1 != 0 {
                    1.0
                } else {
                    -1.0
                };
                let gain = sign * envelope;
                self.channels[c].banks[build.bank].storage[i] = Tap { delay, gain };
                build.energy[c] += gain * gain;
            }
            build.index += 1;
            if build.index == build.count {
                for c in 0..2 {
                    build.scale[c] = if build.energy[c] > 1e-12 {
                        1.0 / sqrt(build.energy[c])
                    } else {
                        1.0
                    };
                }
                build.index = 0;
                build.phase = 2;
            }
        } else if build.phase == 2 {
            for _ in 0..8 {
                for c in 0..2 {
                    self.channels[c].banks[build.bank].storage[build.index].gain *= build.scale[c];
                }
                build.index += 1;
                if build.index == build.count {
                    build.phase = 0;
                    break;
                }
            }
        }
    }

    #[inline(always)]
    fn convolve(bank: &Bank, delay_line: &[f32; MAX_DELAY_SAMPLES], wp: usize) -> f32 {
        let mut result = 0.0f32;
        let size = MAX_DELAY_SAMPLES;
        // A malformed count must not panic inside an audio callback. Silence
        // this bank rather than convolving an incomplete or unrelated table.
        let taps = bank.storage.get(..bank.count).unwrap_or(&[]);
        // A corrupt position cannot safely address the history. Fail closed.
        if wp >= size {
            return 0.0;
        }
        for tap in taps {
            let delay = tap.delay as usize;
            let d = if delay < size {
                delay
            } else {
                wrap_out_of_range_delay(delay)
            };
            let read = history_index(wp, d);
            // SAFETY: wp is in range and d is normalized to the ring
            // length, so their wrapped difference indexes delay_line.
            result += tap.gain * unsafe { delay_line.get_unchecked(read) };
        }
        result
    }

    #[inline(always)]
    fn transition_weights(&self) -> (f32, f32) {
        if self.transitioning {
            let phase = if self.active_transition_samples <= 1 {
                PI_2
            } else {
                self.transition_position as f32 * self.transition_phase_step
            };
            (cos(phase), sin(phase))
        } else {
            (1.0, 0.0)
        }
    }

    #[inline(always)]
    fn convolve_pair(ca: &Channel, history: &MonoHistory, bank: usize) -> (f32, f32) {
        let wp = ca.write_position;
        let mut sum_a = 0.0f32;
        let mut sum_b = 0.0f32;
        let taps = ca.banks[bank]
            .storage
            .get(..ca.banks[bank].count)
            .unwrap_or(&[]);
        if ca.nonzero_samples != 0
            && history.nonzero_samples != 0
            && wp == history.write_position
            && wp < MAX_DELAY_SAMPLES
        {
            for tap in taps {
                let delay = tap.delay as usize;
                let d = if delay < MAX_DELAY_SAMPLES {
                    delay
                } else {
                    wrap_out_of_range_delay(delay)
                };
                let read = history_index(wp, d);
                // SAFETY: wp is checked above and d is normalized to the
                // ring length. Their wrapped difference is therefore less
                // than MAX_DELAY_SAMPLES, the length of both arrays.
                unsafe {
                    sum_a += tap.gain * ca.delay_line.get_unchecked(read);
                    sum_b += tap.gain * history.delay_line.get_unchecked(read);
                }
            }
        } else if ca.nonzero_samples != 0 && history.nonzero_samples != 0 {
            for tap in taps {
                let delay = tap.delay as usize;
                let d = if delay < MAX_DELAY_SAMPLES {
                    delay
                } else {
                    wrap_out_of_range_delay(delay)
                };
                let ra = history_index(wp, d);
                let rb = history_index(history.write_position, d);
                sum_a += tap.gain * ca.delay_line[ra];
                sum_b += tap.gain * history.delay_line[rb];
            }
        } else if ca.nonzero_samples != 0 {
            sum_a = Self::convolve(&ca.banks[bank], &ca.delay_line, wp);
        } else if history.nonzero_samples != 0 {
            sum_b = Self::convolve(&ca.banks[bank], &history.delay_line, history.write_position);
        }
        (sum_a, sum_b)
    }

    /// Processes a single stereo frame (in_l, in_r) -> (out_l, out_r).
    #[inline(always)]
    pub fn process_sample(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        // Validate both before touching either channel, so a damaged cursor
        // cannot cause a partial frame or a bounds panic in the callback.
        if !self.valid_banks()
            || self.channels[0].write_position >= MAX_DELAY_SAMPLES
            || self.channels[1].write_position >= MAX_DELAY_SAMPLES
        {
            return (0.0, 0.0);
        }
        let amount = self.advance_amount();
        let (old_weight, new_weight) = self.transition_weights();

        let inputs = [audio_input(in_l), audio_input(in_r)];
        let mut outputs = [0.0f32; 2];

        for c in 0..2 {
            let dry = inputs[c];
            let wp = self.channels[c].write_position;
            if self.channels[c].delay_line[wp] != 0.0 {
                self.channels[c].nonzero_samples -= 1;
            }
            if dry != 0.0 {
                self.channels[c].nonzero_samples += 1;
            }
            self.channels[c].delay_line[wp] = dry;

            let wet = if self.channels[c].nonzero_samples == 0 {
                0.0
            } else {
                let mut wet = Self::convolve(
                    &self.channels[c].banks[self.active_bank],
                    &self.channels[c].delay_line,
                    self.channels[c].write_position,
                );
                if self.transitioning {
                    wet = old_weight * wet
                        + new_weight
                            * Self::convolve(
                                &self.channels[c].banks[self.pending_bank],
                                &self.channels[c].delay_line,
                                self.channels[c].write_position,
                            );
                }
                wet
            };

            // In an Aux Send bus effect, return pure wet scaled by amount, soft-clipped to [-1.0, 1.0].
            outputs[c] = soft_clip(wet * amount);
        }

        for c in 0..2 {
            let mut wp = self.channels[c].write_position + 1;
            if wp >= MAX_DELAY_SAMPLES {
                wp = 0;
            }
            self.channels[c].write_position = wp;
        }

        self.advance_filter();
        (outputs[0], outputs[1])
    }

    /// Processes two independent mono histories using the same filter timeline.
    pub(crate) fn process_shared_mono(
        &mut self,
        history: &mut MonoHistory,
        in_a: f32,
        in_b: f32,
    ) -> ((f32, f32), (f32, f32)) {
        if !self.valid_banks()
            || self.channels[0].write_position >= MAX_DELAY_SAMPLES
            || self.channels[1].write_position >= MAX_DELAY_SAMPLES
            || history.write_position >= MAX_DELAY_SAMPLES
        {
            return ((0.0, 0.0), (0.0, 0.0));
        }
        let amount = self.advance_amount();
        let (old_weight, new_weight) = self.transition_weights();
        let input_a = audio_input(in_a);
        let input_b = audio_input(in_b);
        history.write(input_b);
        let mut out_a = [0.0; 2];
        let mut out_b = [0.0; 2];
        for c in 0..2 {
            let ca = &mut self.channels[c];
            let wp = ca.write_position;
            if ca.delay_line[wp] != 0.0 {
                ca.nonzero_samples -= 1;
            }
            if input_a != 0.0 {
                ca.nonzero_samples += 1;
            }
            ca.delay_line[wp] = input_a;
            let (mut wa, mut wb) = Self::convolve_pair(ca, history, self.active_bank);
            if self.transitioning && (ca.nonzero_samples != 0 || history.nonzero_samples != 0) {
                let (na, nb) = Self::convolve_pair(ca, history, self.pending_bank);
                wa = old_weight * wa + new_weight * na;
                wb = old_weight * wb + new_weight * nb;
            }
            out_a[c] = soft_clip(wa * amount);
            out_b[c] = soft_clip(wb * amount);
            ca.write_position = if wp + 1 >= MAX_DELAY_SAMPLES {
                0
            } else {
                wp + 1
            };
        }
        history.advance();
        self.advance_filter();
        ((out_a[0], out_a[1]), (out_b[0], out_b[1]))
    }

    #[inline(always)]
    fn advance_amount(&mut self) -> f32 {
        if self.amount_remaining != 0 {
            self.amount_remaining -= 1;
            if self.amount_remaining == 0 {
                self.amount_current = self.current.amount;
            } else {
                self.amount_current += self.amount_step;
            }
        }
        self.amount_current
    }

    // Progress a fade and prepare future taps without exposing a partial bank.
    fn advance_filter(&mut self) {
        if self.transitioning {
            self.transition_position += 1;
            if self.transition_position >= self.active_transition_samples {
                self.finish_transition();
            }
        } else {
            if self.current.dynamic && self.samples_until_transition > 0 {
                self.samples_until_transition -= 1;
            }
            if self.build.phase == 0
                && (self.filter_update_pending
                    || (self.current.dynamic && self.samples_until_transition == 0))
            {
                self.filter_update_pending = false;
                self.transitioning = true;
                self.transition_position = 0;
                self.active_transition_samples = self.transition_samples;
                // Compute once per fade, with a nonzero divisor even if a
                // future caller allows a one-sample transition.
                self.transition_phase_step =
                    PI_2 / self.active_transition_samples.saturating_sub(1).max(1) as f32;
            }
        }

        if self.build.phase != 0 {
            self.build_step();
        }
    }

    fn finish_transition(&mut self) {
        self.active_bank = self.pending_bank;
        self.pending_bank = 1 - self.active_bank;
        self.transitioning = false;
        self.transition_position = 0;
        // The fade just completed at the duration captured when it began;
        // transition_samples may have been edited while it was in progress.
        self.samples_until_transition = if self.refresh_samples > self.active_transition_samples {
            self.refresh_samples - self.active_transition_samples
        } else {
            1
        };

        self.start_next_build(self.pending_bank);
    }

    /// Process a block of stereo frames in-place or from buffer.
    pub fn process_block(&mut self, left: &mut [f32], right: &mut [f32]) {
        let count = left.len().min(right.len());
        for i in 0..count {
            let (out_l, out_r) = self.process_sample(left[i], right[i]);
            left[i] = out_l;
            right[i] = out_r;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_stereo_bank_seeds_follow_the_upstream_sequence() {
        // Delay/sign samples from upstream a391a12a0b96b190a05cd7cefa25becf4f31660c
        // at 48 kHz, 25 ms, 400 pps, 100 ms renewal and a 1 ms fade.
        let expected = [
            [[144, 366, -648, -903], [-112, 449, -636, 885]],
            [[-183, -424, -615, 901], [114, 435, 643, -925]],
            [[88, -413, 696, 939], [102, -442, -670, 899]],
            [[76, -375, -711, -890], [-167, -430, 724, -896]],
        ];
        let mut engine = Box::new(VelvetDecorrelator::new());
        engine.init(Parameters {
            length_ms: 25.0,
            density: 400.0,
            refresh_seconds: 0.1,
            transition_ms: 1.0,
            dynamic: true,
            ..Parameters::default()
        });
        for (bank, expected_channels) in expected.iter().enumerate() {
            if bank != 0 {
                let frames = if bank == 1 { 4848 } else { 4800 };
                for _ in 0..frames {
                    engine.advance_filter();
                }
            }
            for (c, expected_taps) in expected_channels.iter().enumerate() {
                let actual = &engine.channels[c].banks[engine.active_bank];
                let selected = [1, 3, 5, 7].map(|i| {
                    let tap = actual.storage[i];
                    if tap.gain < 0.0 {
                        -(tap.delay as i32)
                    } else {
                        tap.delay as i32
                    }
                });
                assert_eq!(&selected, expected_taps, "bank {bank}, channel {c}");
            }
        }
    }

    #[test]
    fn maximum_density_renewal_stays_on_schedule_after_long_fades() {
        let mut effect = Box::new(VelvetDecorrelator::new());
        effect.init(Parameters {
            length_ms: 100.0,
            density: 4000.0,
            refresh_seconds: 0.1,
            transition_ms: 90.0,
            dynamic: true,
            ..Parameters::default()
        });
        assert_eq!(effect.channels[0].banks[0].count, MAX_TAPS);
        let mut starts = Vec::new();
        for frame in 0..(effect.refresh_samples * 4) {
            let was_transitioning = effect.transitioning;
            effect.advance_filter();
            if !was_transitioning && effect.transitioning {
                starts.push(frame);
            }
        }
        assert!(starts.len() >= 3, "too few transitions: {starts:?}");
        for pair in starts.windows(2) {
            assert_eq!(
                pair[1] - pair[0],
                effect.refresh_samples,
                "transition starts: {starts:?}"
            );
        }
    }

    #[test]
    fn changing_fade_length_during_renewal_preserves_start_cadence() {
        for (initial_ms, next_ms) in [(90.0, 5.0), (5.0, 90.0)] {
            let mut effect = Box::new(VelvetDecorrelator::new());
            let mut params = Parameters {
                refresh_seconds: 0.1,
                transition_ms: initial_ms,
                dynamic: true,
                ..Parameters::default()
            };
            effect.init(params);
            let mut first_start = None;
            let mut second_start = None;
            let mut edited = false;
            for frame in 0..(effect.refresh_samples * 4) {
                let was_transitioning = effect.transitioning;
                effect.advance_filter();
                if !was_transitioning && effect.transitioning {
                    if first_start.is_none() {
                        first_start = Some(frame);
                    } else {
                        second_start = Some(frame);
                        break;
                    }
                }
                if effect.transitioning && effect.transition_position > 1 && !edited {
                    params.transition_ms = next_ms;
                    effect.set_parameters(params);
                    edited = true;
                }
            }
            assert!(edited);
            assert_eq!(
                second_start.unwrap() - first_start.unwrap(),
                effect.refresh_samples,
                "initial fade {initial_ms} ms, new fade {next_ms} ms"
            );
        }
    }

    #[test]
    fn batched_normalization_preserves_banks_and_bounds_each_step() {
        for (length_ms, density) in [
            (25.0, 100.0),
            (25.0, 400.0),
            (95.0, 3000.0),
            (100.0, 4000.0),
        ] {
            let mut effect = Box::new(VelvetDecorrelator::new());
            effect.init(Parameters {
                length_ms,
                density,
                ..Parameters::default()
            });
            let expected = [effect.channels[0].banks[1], effect.channels[1].banks[1]];
            let count = expected[0].count;
            effect.start_build(1, [1, 1]);
            let mut steps = 0;
            let mut sequential = None;
            while effect.build.phase != 0 {
                let phase = effect.build.phase;
                let before = [effect.channels[0].banks[1], effect.channels[1].banks[1]];
                if phase == 2 && sequential.is_none() {
                    let mut reference = before;
                    for c in 0..2 {
                        for tap in &mut reference[c].storage[..count] {
                            tap.gain *= effect.build.scale[c];
                        }
                    }
                    sequential = Some(reference);
                }
                effect.build_step();
                steps += 1;
                for c in 0..2 {
                    let after = &effect.channels[c].banks[1];
                    let changed = before[c]
                        .storage
                        .iter()
                        .zip(after.storage.iter())
                        .filter(|(a, b)| a.delay != b.delay || a.gain.to_bits() != b.gain.to_bits())
                        .count();
                    assert!(changed <= if phase == 1 { 1 } else { 8 });
                }
            }
            assert_eq!(steps, count + (count + 7) / 8);
            let sequential = sequential.unwrap();
            for c in 0..2 {
                let actual = &effect.channels[c].banks[1];
                for ((a, b), reference) in actual.storage[..count]
                    .iter()
                    .zip(&expected[c].storage[..count])
                    .zip(&sequential[c].storage[..count])
                {
                    assert_eq!((a.delay, a.gain.to_bits()), (b.delay, b.gain.to_bits()));
                    assert_eq!(
                        (a.delay, a.gain.to_bits()),
                        (reference.delay, reference.gain.to_bits())
                    );
                }
            }
        }
    }

    #[test]
    fn invalid_history_positions_fail_silent_before_any_history_write() {
        for invalid in [MAX_DELAY_SAMPLES, usize::MAX] {
            for channel in 0..2 {
                let mut engine = Box::new(VelvetDecorrelator::new());
                engine.init(Parameters::default());
                engine.channels[channel].write_position = invalid;
                assert_eq!(engine.process_sample(0.25, 0.5), (0.0, 0.0));
                assert_eq!(engine.channels[1 - channel].write_position, 0);
                assert_eq!(engine.channels[1 - channel].delay_line[0], 0.0);

                let mut history = MonoHistory::new();
                assert_eq!(
                    engine.process_shared_mono(&mut history, 0.25, 0.5),
                    ((0.0, 0.0), (0.0, 0.0))
                );
                assert_eq!(history.write_position, 0);
                assert_eq!(history.delay_line[0], 0.0);
            }
            let mut engine = Box::new(VelvetDecorrelator::new());
            engine.init(Parameters::default());
            let mut history = MonoHistory::new();
            history.write_position = invalid;
            assert_eq!(
                engine.process_shared_mono(&mut history, 0.25, 0.5),
                ((0.0, 0.0), (0.0, 0.0))
            );
            assert_eq!(engine.channels[0].write_position, 0);
            assert_eq!(engine.channels[0].delay_line[0], 0.0);
            history.silence();
            assert_eq!(history.write_position, invalid);
        }
    }

    #[test]
    fn invalid_bank_selection_rejects_audio_and_edits_until_reset() {
        for (active, pending) in [
            (2, 1),
            (usize::MAX, 1),
            (0, 2),
            (0, usize::MAX),
            (0, 0),
            (1, 1),
        ] {
            for transitioning in [false, true] {
                let mut engine = Box::new(VelvetDecorrelator::new());
                engine.init(Parameters::default());
                engine.active_bank = active;
                engine.pending_bank = pending;
                engine.transitioning = transitioning;
                let before_generation = engine.generation;
                let before_spread = engine.current.length_ms;
                let before_timer = engine.samples_until_transition;
                let mut history = MonoHistory::new();

                assert_eq!(engine.process_sample(0.25, 0.5), (0.0, 0.0));
                assert_eq!(
                    engine.process_shared_mono(&mut history, 0.25, 0.5),
                    ((0.0, 0.0), (0.0, 0.0))
                );
                assert_eq!(engine.channels[0].write_position, 0);
                assert_eq!(engine.channels[1].write_position, 0);
                assert_eq!(history.write_position, 0);
                assert_eq!(engine.channels[0].delay_line[0], 0.0);
                assert_eq!(history.delay_line[0], 0.0);
                assert_eq!(engine.samples_until_transition, before_timer);

                let mut changed = engine.current;
                changed.length_ms += 1.0;
                engine.set_parameters(changed);
                assert_eq!(engine.current.length_ms, before_spread);
                assert_eq!(engine.generation, before_generation);

                engine.reset();
                assert_eq!((engine.active_bank, engine.pending_bank), (0, 1));
                let (left, right) = engine.process_sample(0.25, 0.5);
                assert!(left.is_finite() && right.is_finite() && left != 0.0 && right != 0.0);
            }
        }
    }

    #[test]
    fn history_index_matches_ring_subtraction_at_every_wrap_boundary() {
        for write_position in 0..MAX_DELAY_SAMPLES {
            for delay in [
                0,
                1,
                write_position.saturating_sub(1),
                write_position,
                (write_position + 1).min(MAX_DELAY_SAMPLES - 1),
                MAX_DELAY_SAMPLES - 1,
            ] {
                assert_eq!(
                    history_index(write_position, delay),
                    (write_position + MAX_DELAY_SAMPLES - delay) % MAX_DELAY_SAMPLES,
                );
            }
        }
    }

    #[test]
    fn history_index_stays_in_bounds_for_all_valid_positions_and_delays() {
        for write_position in 0..MAX_DELAY_SAMPLES {
            for delay in 0..MAX_DELAY_SAMPLES {
                let index = history_index(write_position, core::hint::black_box(delay));
                assert!(index < MAX_DELAY_SAMPLES);
                assert_eq!(
                    index,
                    (write_position + MAX_DELAY_SAMPLES - delay) % MAX_DELAY_SAMPLES
                );
            }
        }
    }

    #[test]
    fn invalid_aligned_positions_take_the_checked_fallback() {
        let mut channel = Box::new(Channel::new());
        let mut history = Box::new(MonoHistory::new());
        channel.write_position = MAX_DELAY_SAMPLES;
        history.write_position = MAX_DELAY_SAMPLES;
        channel.nonzero_samples = 1;
        history.nonzero_samples = 1;
        channel.banks[0].count = 1;
        // A bad position still encounters a bounds check, not an unchecked read.
        assert!(
            std::panic::catch_unwind(|| VelvetDecorrelator::convolve_pair(&channel, &history, 0))
                .is_err()
        );
        assert_eq!(
            VelvetDecorrelator::convolve(&channel.banks[0], &channel.delay_line, MAX_DELAY_SAMPLES),
            0.0
        );
        channel.banks[0].count = MAX_TAPS + 1;
        assert_eq!(
            VelvetDecorrelator::convolve(&channel.banks[0], &channel.delay_line, 0),
            0.0
        );
    }

    #[test]
    fn aligned_shared_history_reads_match_checked_reference() {
        let mut channel = Box::new(Channel::new());
        let mut history = Box::new(MonoHistory::new());
        for i in 0..MAX_DELAY_SAMPLES {
            channel.delay_line[i] = (i % 17) as f32 / 32.0;
            history.delay_line[i] = -((i % 13) as f32) / 32.0;
        }
        channel.nonzero_samples = 1;
        history.nonzero_samples = 1;
        for wp in [
            0,
            1,
            MAX_DELAY_SAMPLES / 2,
            MAX_DELAY_SAMPLES - 2,
            MAX_DELAY_SAMPLES - 1,
        ] {
            channel.write_position = wp;
            history.write_position = wp;
            let delays = [
                0,
                1,
                wp as u32,
                (wp + 1) as u32,
                (MAX_DELAY_SAMPLES - 1) as u32,
                MAX_DELAY_SAMPLES as u32,
                u32::MAX,
            ];
            let bank = &mut channel.banks[0];
            bank.count = MAX_TAPS;
            for i in 0..bank.count {
                bank.storage[i] = Tap {
                    delay: delays[i % delays.len()],
                    gain: (i as f32 + 1.0) / 16.0,
                };
            }
            let mut expected = (0.0f32, 0.0f32);
            for tap in &bank.storage[..bank.count] {
                let d = tap.delay as usize % MAX_DELAY_SAMPLES;
                let read = (wp + MAX_DELAY_SAMPLES - d) % MAX_DELAY_SAMPLES;
                expected.0 += tap.gain * channel.delay_line[read];
                expected.1 += tap.gain * history.delay_line[read];
            }
            assert_eq!(
                VelvetDecorrelator::convolve_pair(&channel, &history, 0),
                expected,
                "wp={wp}"
            );
            assert_eq!(
                VelvetDecorrelator::convolve(&channel.banks[0], &channel.delay_line, wp),
                expected.0,
                "wp={wp}"
            );
            assert_eq!(
                VelvetDecorrelator::convolve(&channel.banks[0], &history.delay_line, wp),
                expected.1,
                "wp={wp}"
            );
        }
    }

    #[test]
    fn crossfade_phase_matches_division_and_keeps_its_duration_during_edits() {
        for duration_ms in [1.0, 250.0, 54_000.0] {
            let mut effect = Box::new(VelvetDecorrelator::new());
            let mut params = Parameters {
                dynamic: false,
                refresh_seconds: 60.0,
                transition_ms: duration_ms,
                ..Parameters::default()
            };
            effect.init(params);
            params.length_ms += 1.0;
            effect.set_parameters(params);
            for _ in 0..(2 * MAX_TAPS + 2) {
                if effect.transitioning {
                    break;
                }
                effect.advance_filter();
            }
            assert!(effect.transitioning);
            let length = effect.active_transition_samples;
            params.transition_ms = 5.0;
            effect.set_parameters(params);
            assert_eq!(effect.active_transition_samples, length);
            for step in 0..=1024 {
                let position = (length - 1) * step / 1024;
                effect.transition_position = position;
                let phase = PI_2 * position as f32 / (length - 1) as f32;
                let (old, new) = effect.transition_weights();
                assert!(
                    (old - cos(phase)).abs() < 2e-6,
                    "duration={duration_ms} position={position}"
                );
                assert!(
                    (new - sin(phase)).abs() < 2e-6,
                    "duration={duration_ms} position={position}"
                );
            }
        }
    }
}
