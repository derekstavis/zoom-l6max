//! Plateau / Dattorro Plate Reverb DSP Engine
//!
//! Adapted from Dale Johnson's Valley Plateau / PlateauNEVersio for bare-metal
//! ARM Cortex-M7 target for Zoom LiveTrak L6max. The physical processor
//! model has not been identified.
//!
//! Based on Jon Dattorro's 1997 AES paper "Effect Design Part 1: Reverberator".
//! Provides lush, high-density plate reverberation with modulated allpass loops,
//! dual tone damping, diffusion control, and infinite sustain freeze.

use super::math::*;

pub const SAMPLE_RATE: f32 = if cfg!(experimental_plateau_32k) {
    32000.0
} else {
    48000.0
};
const OUTPUT_FADE_SAMPLES: u32 = 64;
const PREDELAY_FADE_SAMPLES: u32 = 64;
const DIFFUSION_BYPASS_FADE_SAMPLES: u32 = 256;
const DAMPING_FADE_SAMPLES: u32 = 64;
const DATTORRO_SR: f32 = 29761.0;
const SR_SCALE: f32 = SAMPLE_RATE / DATTORRO_SR;
// Fixed input-filter defaults in the Multistomp Plateau control mapping.
const INPUT_HIGH_CUT_HZ: f32 = 14_080.0;
const INPUT_LOW_CUT_HZ: f32 = 13.75;

// The 32 kHz core is experimental: its caller must perform rate conversion
// and reserve its larger state. The firmware integration does neither yet.
const EXPERIMENTAL_32K: bool = cfg!(experimental_plateau_32k);
pub(crate) const MAX_SIZE: f32 = if EXPERIMENTAL_32K { 4.0 } else { 1.2 };
pub(crate) const MAX_PREDELAY_MS: f32 = if EXPERIMENTAL_32K { 500.0 } else { 190.0 };
const PREDELAY_MAX: usize = if EXPERIMENTAL_32K { 16001 } else { 9121 };
const IN_APF1_MAX: usize = if EXPERIMENTAL_32K { 154 } else { 274 };
const IN_APF2_MAX: usize = if EXPERIMENTAL_32K { 118 } else { 209 };
const IN_APF3_MAX: usize = if EXPERIMENTAL_32K { 410 } else { 735 };
const IN_APF4_MAX: usize = if EXPERIMENTAL_32K { 300 } else { 538 };
const L_APF1_MAX: usize = if EXPERIMENTAL_32K { 2923 } else { 1333 };
const L_DEL1_MAX: usize = if EXPERIMENTAL_32K { 19155 } else { 8620 };
const L_APF2_MAX: usize = if EXPERIMENTAL_32K { 7760 } else { 3704 };
const L_DEL2_MAX: usize = if EXPERIMENTAL_32K { 16002 } else { 7201 };
const R_APF1_MAX: usize = if EXPERIMENTAL_32K { 3938 } else { 1790 };
const R_DEL1_MAX: usize = if EXPERIMENTAL_32K { 18140 } else { 8163 };
const R_APF2_MAX: usize = if EXPERIMENTAL_32K { 11442 } else { 5160 };
const R_DEL2_MAX: usize = if EXPERIMENTAL_32K { 13606 } else { 6123 };

#[derive(Clone, Copy)]
pub struct Delay<const N: usize> {
    pub buffer: [f32; N],
    pub write_pos: usize,
    // Configured delay length; the complete backing ring keeps moving.
    pub length: usize,
}

impl<const N: usize> Delay<N> {
    pub const fn new() -> Self {
        Self {
            buffer: [0.0; N],
            write_pos: 0,
            length: N,
        }
    }

    pub fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.write_pos = 0;
    }

    #[inline(always)]
    fn valid(&self, min_length: usize, max_length: usize) -> bool {
        self.write_pos < N && self.length >= min_length && self.length <= max_length
    }

    #[inline(always)]
    pub fn write(&mut self, sample: f32) {
        self.buffer[self.write_pos] = sample;
        self.write_pos += 1;
        if self.write_pos >= N {
            self.write_pos = 0;
        }
    }

    #[inline(always)]
    pub fn read(&self, delay: usize) -> f32 {
        self.read_history(if delay >= self.length {
            self.length - 1
        } else {
            delay
        })
    }

    // The whole backing ring advances even when the configured delay is short.
    // Otherwise expanding it can replay samples from before the last shrink.
    #[inline(always)]
    fn read_history(&self, delay: usize) -> f32 {
        let read_pos = if self.write_pos >= delay {
            self.write_pos - delay
        } else {
            self.write_pos + N - delay
        };
        self.buffer[read_pos]
    }

    #[inline(always)]
    pub fn read_interpolated(&self, delay: f32) -> f32 {
        let d = clamp(delay, 0.0, (self.length - 2) as f32);
        let d_int = d as usize;
        let frac = d - d_int as f32;

        let r1 = if self.write_pos >= d_int {
            self.write_pos - d_int
        } else {
            self.write_pos + N - d_int
        };
        let r2 = if r1 == 0 { N - 1 } else { r1 - 1 };

        let s1 = self.buffer[r1];
        let s2 = self.buffer[r2];
        s1 + frac * (s2 - s1)
    }

    #[inline(always)]
    pub fn tap(&self, offset: usize) -> f32 {
        // Output taps read the backing history independently of the
        // feedback delay's current length.
        self.read_history(offset.min(N - 1))
    }

    // Keep one extra history sample for fractional interpolation.
    fn set_tank_time(&mut self, samples: f32) -> f32 {
        let samples = clamp(samples, 10.0, (N - 1) as f32);
        self.length = samples as usize;
        samples - self.length as f32
    }

    #[inline(always)]
    fn read_tank(&self, fraction: f32) -> f32 {
        let newer = self.read_history(self.length);
        if fraction == 0.0 {
            newer
        } else {
            let older = self.read_history(self.length + 1);
            newer + fraction * (older - newer)
        }
    }
}

pub struct Allpass<const N: usize> {
    pub delay: Delay<N>,
    pub gain: f32,
    pub delay_time: usize,
    delay_fraction: f32,
    // The reference allpass consumes the delay output from the prior frame.
    delayed: f32,
}

impl<const N: usize> Allpass<N> {
    pub const fn new(gain: f32, delay_time: usize) -> Self {
        Self {
            delay: Delay::new(),
            gain,
            delay_time,
            delay_fraction: 0.0,
            delayed: 0.0,
        }
    }

    pub fn clear(&mut self) {
        self.delay.clear();
        self.delayed = 0.0;
    }

    fn set_delay_samples(&mut self, samples: f32, max: usize) {
        let samples = clamp(samples, 1.0, max as f32);
        self.delay_time = samples as usize;
        self.delay_fraction = samples - self.delay_time as f32;
    }

    #[inline(always)]
    pub fn process(&mut self, input: f32) -> f32 {
        self.process_modulated(input, self.delay_time as f32 + self.delay_fraction)
    }

    #[inline(always)]
    pub fn process_modulated(&mut self, input: f32, mod_delay: f32) -> f32 {
        let delayed = self.delayed;
        let in_sum = input + delayed * self.gain;
        let output = delayed - in_sum * self.gain;
        let d = clamp(mod_delay, 0.0, (self.delay.length - 2) as f32);
        self.delayed = if d < 1.0 {
            in_sum + d * (self.delay.read_history(1) - in_sum)
        } else {
            self.delay.read_interpolated(d)
        };
        self.delay.write(in_sum);
        output
    }
}

#[derive(Clone, Copy)]
pub struct OnePoleLP {
    pub z: f32,
    pub a: f32,
    pub b: f32,
}

impl OnePoleLP {
    pub const fn new() -> Self {
        Self {
            z: 0.0,
            a: 1.0,
            b: 0.0,
        }
    }

    pub fn set_cutoff(&mut self, cutoff_hz: f32) {
        let c = clamp(cutoff_hz, 20.0, SAMPLE_RATE * 0.49);
        // For H(z) = a / (1 - b z^-1), a = 1 - b. Solve the
        // digital -3 dB condition at c, using sin(w/2) to avoid
        // cancellation near zero and a rationalized positive root.
        let s = sin(PI * c / SAMPLE_RATE);
        let a = (2.0 * s) / (sqrt(1.0 + s * s) + s);
        self.a = a;
        self.b = 1.0 - a;
    }

    #[inline(always)]
    pub fn process(&mut self, x: f32) -> f32 {
        self.z = self.a * x + self.b * self.z;
        self.z
    }

    pub fn clear(&mut self) {
        self.z = 0.0;
    }
}

#[derive(Clone, Copy)]
pub struct OnePoleHP {
    previous_input: f32,
    previous_output: f32,
    pole: f32,
    gain: f32,
}

impl OnePoleHP {
    pub const fn new() -> Self {
        Self {
            previous_input: 0.0,
            previous_output: 0.0,
            pole: 0.0,
            gain: 1.0,
        }
    }

    pub fn set_cutoff(&mut self, cutoff_hz: f32) {
        let c = clamp(cutoff_hz, INPUT_LOW_CUT_HZ, SAMPLE_RATE * 0.49);
        // Bilinear one-pole high-pass with a prewarped cutoff. This keeps
        // the Nyquist passband at unity and the cutoff at -3 dB.
        let angle = PI * c / SAMPLE_RATE;
        let s = sin(angle);
        let c = cos(angle);
        self.pole = (c - s) / (c + s);
        self.gain = (1.0 + self.pole) * 0.5;
    }

    #[inline(always)]
    pub fn process(&mut self, x: f32) -> f32 {
        let output = self.gain * (x - self.previous_input) + self.pole * self.previous_output;
        self.previous_input = x;
        self.previous_output = output;
        output
    }

    pub fn clear(&mut self) {
        self.previous_input = 0.0;
        self.previous_output = 0.0;
    }
}

pub struct TriangleLFO {
    pub phase: f32,
    pub phase_inc: f32,
}

impl TriangleLFO {
    pub const fn new(freq: f32) -> Self {
        Self {
            phase: 0.0,
            phase_inc: freq / SAMPLE_RATE,
        }
    }

    #[inline(always)]
    pub fn step(&mut self) -> f32 {
        // The reference emits the current phase before advancing it.
        let output = if self.phase < 0.5 {
            4.0 * self.phase - 1.0
        } else {
            3.0 - 4.0 * self.phase
        };
        self.phase += self.phase_inc;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        output
    }
}

#[derive(Clone, Copy)]
pub struct PlateauParameters {
    pub predelay_ms: f32, // bounded by the selected configuration
    pub size: f32,        // time scale, bounded by the selected configuration
    pub decay: f32,       // 0.1 to 0.999 (tail duration)
    pub low_cut_hz: f32,  // 20.0 to 1000.0 Hz
    pub high_cut_hz: f32, // 1000.0 to 18000.0 Hz
    pub diffusion: f32,   // 0.0 to 1.0
    pub mod_depth: f32,   // 0.0 to 16.0 samples
    pub freeze: bool,     // infinite sustain
    pub wet: f32,         // 0.0 to 1.0
}

impl Default for PlateauParameters {
    fn default() -> Self {
        Self {
            predelay_ms: 10.0,
            size: 1.0,
            decay: 0.85,
            low_cut_hz: 80.0,
            high_cut_hz: 6500.0,
            diffusion: 0.75,
            mod_depth: 8.0,
            freeze: false,
            wet: 1.0,
        }
    }
}

pub struct PlateauReverb {
    // Predelay
    input_dc: OnePoleHP,
    pub predelay: Delay<PREDELAY_MAX>,
    pub predelay_samples: usize,
    predelay_from_samples: usize,
    predelay_remaining: u32,
    predelay_anchor: f32,
    predelay_previous: f32,
    predelay_use_anchor: bool,

    // Input diffusers
    pub in_apf1: Allpass<IN_APF1_MAX>,
    pub in_apf2: Allpass<IN_APF2_MAX>,
    pub in_apf3: Allpass<IN_APF3_MAX>,
    pub in_apf4: Allpass<IN_APF4_MAX>,
    pub in_lp: OnePoleLP,
    pub in_hp: OnePoleHP,

    // Left tank loop
    pub l_apf1: Allpass<L_APF1_MAX>,
    pub l_del1: Delay<L_DEL1_MAX>,
    l_del1_fraction: f32,
    pub l_lp: OnePoleLP,
    pub l_hp: OnePoleHP,
    pub l_apf2: Allpass<L_APF2_MAX>,
    pub l_del2: Delay<L_DEL2_MAX>,
    l_del2_fraction: f32,
    pub l_sum: f32,

    // Right tank loop
    pub r_apf1: Allpass<R_APF1_MAX>,
    pub r_del1: Delay<R_DEL1_MAX>,
    r_del1_fraction: f32,
    pub r_lp: OnePoleLP,
    pub r_hp: OnePoleHP,
    pub r_apf2: Allpass<R_APF2_MAX>,
    pub r_del2: Delay<R_DEL2_MAX>,
    r_del2_fraction: f32,
    pub r_sum: f32,

    // Modulation
    pub lfo1: TriangleLFO,
    pub lfo2: TriangleLFO,
    pub lfo3: TriangleLFO,
    pub lfo4: TriangleLFO,

    // Output DC blocks
    pub out_hp_l: OnePoleHP,
    pub out_hp_r: OnePoleHP,

    // Short output transition masks discontinuities from live controls.
    transition_remaining: u32,
    transition_anchor: [f32; 2],
    previous_output: [f32; 2],
    diffusion_mix: f32,
    diffusion_step: f32,
    diffusion_current: f32,
    diffusion_gain_step: f32,
    diffusion_remaining: u32,
    damping_lp_target: f32,
    damping_hp_target: f32,
    damping_lp_step: f32,
    damping_hp_step: f32,
    damping_remaining: u32,
    pub params: PlateauParameters,
}

impl PlateauReverb {
    pub const fn new() -> Self {
        Self {
            input_dc: OnePoleHP::new(),
            predelay: Delay::new(),
            predelay_samples: 480,
            predelay_from_samples: 480,
            predelay_remaining: 0,
            predelay_anchor: 0.0,
            predelay_previous: 0.0,
            predelay_use_anchor: false,
            in_apf1: Allpass::new(0.75, 227),
            in_apf2: Allpass::new(0.75, 173),
            in_apf3: Allpass::new(0.625, 611),
            in_apf4: Allpass::new(0.625, 447),
            in_lp: OnePoleLP::new(),
            in_hp: OnePoleHP::new(),

            l_apf1: Allpass::new(-0.525, 1084),
            l_del1: Delay::new(),
            l_del1_fraction: 0.0,
            l_lp: OnePoleLP::new(),
            l_hp: OnePoleHP::new(),
            l_apf2: Allpass::new(0.525, 2903),
            l_del2: Delay::new(),
            l_del2_fraction: 0.0,
            l_sum: 0.0,

            r_apf1: Allpass::new(-0.525, 1465),
            r_del1: Delay::new(),
            r_del1_fraction: 0.0,
            r_lp: OnePoleLP::new(),
            r_hp: OnePoleHP::new(),
            r_apf2: Allpass::new(0.525, 4284),
            r_del2: Delay::new(),
            r_del2_fraction: 0.0,
            r_sum: 0.0,

            lfo1: TriangleLFO::new(0.10),
            lfo2: TriangleLFO::new(0.15),
            lfo3: TriangleLFO::new(0.12),
            lfo4: TriangleLFO::new(0.18),

            out_hp_l: OnePoleHP::new(),
            out_hp_r: OnePoleHP::new(),

            transition_remaining: 0,
            transition_anchor: [0.0; 2],
            previous_output: [0.0; 2],
            diffusion_mix: 1.0,
            diffusion_step: 0.0,
            diffusion_current: 0.75,
            diffusion_gain_step: 0.0,
            diffusion_remaining: 0,
            damping_lp_target: 1.0,
            damping_hp_target: 0.0,
            damping_lp_step: 0.0,
            damping_hp_step: 0.0,
            damping_remaining: 0,
            params: PlateauParameters {
                predelay_ms: 10.0,
                size: 1.0,
                decay: 0.85,
                low_cut_hz: 80.0,
                high_cut_hz: 6500.0,
                diffusion: 0.75,
                mod_depth: 8.0,
                freeze: false,
                wet: 1.0,
            },
        }
    }

    pub fn init(&mut self, params: PlateauParameters) {
        // These lengths and input gains normally come from `new()`. Restore
        // them explicitly so initialization also works on zeroed storage.
        self.predelay.length = PREDELAY_MAX;
        self.in_apf1.delay.length = IN_APF1_MAX;
        self.in_apf2.delay.length = IN_APF2_MAX;
        self.in_apf3.delay.length = IN_APF3_MAX;
        self.in_apf4.delay.length = IN_APF4_MAX;
        self.l_apf1.delay.length = L_APF1_MAX;
        self.l_apf2.delay.length = L_APF2_MAX;
        self.r_apf1.delay.length = R_APF1_MAX;
        self.r_apf2.delay.length = R_APF2_MAX;
        self.in_apf1.gain = 0.75;
        self.in_apf2.gain = 0.75;
        self.in_apf3.gain = 0.625;
        self.in_apf4.gain = 0.625;
        self.lfo1 = TriangleLFO::new(0.10);
        self.lfo2 = TriangleLFO::new(0.15);
        self.lfo3 = TriangleLFO::new(0.12);
        self.lfo4 = TriangleLFO::new(0.18);
        self.input_dc.set_cutoff(20.0);
        self.in_lp.set_cutoff(INPUT_HIGH_CUT_HZ);
        self.in_hp.set_cutoff(INPUT_LOW_CUT_HZ);
        self.out_hp_l.set_cutoff(20.0);
        self.out_hp_r.set_cutoff(20.0);
        self.set_parameters(params);
        self.clear();
    }

    pub fn clear(&mut self) {
        self.clear_input_filters();
        self.predelay.clear();
        self.in_apf1.clear();
        self.in_apf2.clear();
        self.in_apf3.clear();
        self.in_apf4.clear();

        self.l_apf1.clear();
        self.l_del1.clear();
        self.l_lp.clear();
        self.l_hp.clear();
        self.l_apf2.clear();
        self.l_del2.clear();
        self.l_sum = 0.0;

        self.r_apf1.clear();
        self.r_del1.clear();
        self.r_lp.clear();
        self.r_hp.clear();
        self.r_apf2.clear();
        self.r_del2.clear();
        self.r_sum = 0.0;

        self.out_hp_l.clear();
        self.out_hp_r.clear();
        self.settle_fresh_controls();
    }

    fn clear_input_filters(&mut self) {
        self.input_dc.clear();
        self.in_lp.clear();
        self.in_hp.clear();
    }

    // The caller must ensure all audio histories are already clear.
    pub(crate) fn set_parameters_fresh(&mut self, params: PlateauParameters) {
        self.set_parameters(params);
        self.settle_fresh_controls();
    }

    fn settle_fresh_controls(&mut self) {
        self.predelay_from_samples = self.predelay_samples;
        self.predelay_remaining = 0;
        self.predelay_anchor = 0.0;
        self.predelay_previous = 0.0;
        self.predelay_use_anchor = false;
        self.transition_remaining = 0;
        self.transition_anchor = [0.0; 2];
        self.previous_output = [0.0; 2];
        self.diffusion_mix = if self.params.diffusion == 0.0 {
            0.0
        } else {
            1.0
        };
        self.diffusion_step = 0.0;
        self.diffusion_current = self.params.diffusion;
        self.diffusion_gain_step = 0.0;
        self.diffusion_remaining = 0;
        self.apply_diffusion_gain();
        self.damping_remaining = 0;
        self.apply_damping(self.damping_lp_target, self.damping_hp_target);
    }

    pub fn set_parameters(&mut self, mut params: PlateauParameters) {
        params.size = clamp(params.size, 0.01, MAX_SIZE);
        params.decay = clamp(params.decay, 0.1, 0.999);
        params.diffusion = clamp(params.diffusion, 0.0, 1.0);
        params.mod_depth = clamp(params.mod_depth, 0.0, 16.0);
        params.predelay_ms = clamp(params.predelay_ms, 0.0, MAX_PREDELAY_MS);
        params.wet = clamp(params.wet, 0.0, 1.0);
        // Keep the published parameter values consistent with the cutoffs
        // actually applied by the one-pole filters.
        params.low_cut_hz = clamp(params.low_cut_hz, 20.0, SAMPLE_RATE * 0.49);
        params.high_cut_hz = clamp(params.high_cut_hz, 20.0, SAMPLE_RATE * 0.49);

        self.retarget_diffusion(params.diffusion);
        if self.params.size != params.size || self.params.freeze != params.freeze {
            self.begin_output_transition();
        }
        if params.freeze && !self.params.freeze {
            self.clear_input_filters();
        }
        self.retarget_predelay(params.predelay_ms);
        if params.freeze {
            self.predelay_remaining = 0;
            self.predelay_previous = 0.0;
        }
        self.params = params;

        // Plateau scales the tank with size; input diffuser times depend only
        // on sample rate, so changing size does not retime their histories.
        self.in_apf1
            .set_delay_samples(141.0 * SR_SCALE, IN_APF1_MAX - 2);
        self.in_apf2
            .set_delay_samples(107.0 * SR_SCALE, IN_APF2_MAX - 2);
        self.in_apf3
            .set_delay_samples(379.0 * SR_SCALE, IN_APF3_MAX - 2);
        self.in_apf4
            .set_delay_samples(277.0 * SR_SCALE, IN_APF4_MAX - 2);

        self.apply_tank_size(params.size);

        // Damping controls affect the tank only. Keep the input filters at
        // their fixed defaults and retain separate histories in each channel.
        let mut lp = OnePoleLP::new();
        let mut hp = OnePoleHP::new();
        lp.set_cutoff(params.high_cut_hz);
        hp.set_cutoff(params.low_cut_hz);
        if lp.a != self.damping_lp_target || hp.pole != self.damping_hp_target {
            self.damping_lp_target = lp.a;
            self.damping_hp_target = hp.pole;
            self.damping_lp_step = (lp.a - self.l_lp.a) / DAMPING_FADE_SAMPLES as f32;
            self.damping_hp_step = (hp.pole - self.l_hp.pole) / DAMPING_FADE_SAMPLES as f32;
            self.damping_remaining = DAMPING_FADE_SAMPLES;
        }
    }

    #[inline(always)]
    fn apply_damping(&mut self, lp: f32, hp: f32) {
        self.l_lp.a = lp;
        self.l_lp.b = 1.0 - lp;
        self.r_lp.a = lp;
        self.r_lp.b = 1.0 - lp;
        self.l_hp.pole = hp;
        self.l_hp.gain = (1.0 + hp) * 0.5;
        self.r_hp.pole = hp;
        self.r_hp.gain = (1.0 + hp) * 0.5;
    }

    #[inline(always)]
    fn advance_damping(&mut self) {
        if self.damping_remaining != 0 {
            self.damping_remaining -= 1;
            if self.damping_remaining == 0 {
                self.apply_damping(self.damping_lp_target, self.damping_hp_target);
            } else {
                self.apply_damping(
                    self.l_lp.a + self.damping_lp_step,
                    self.l_hp.pole + self.damping_hp_step,
                );
            }
        }
    }

    fn retarget_diffusion(&mut self, diffusion: f32) {
        if self.params.diffusion != diffusion {
            let target = if diffusion == 0.0 { 0.0 } else { 1.0 };
            self.diffusion_step =
                (target - self.diffusion_mix) / DIFFUSION_BYPASS_FADE_SAMPLES as f32;
            self.diffusion_gain_step =
                (diffusion - self.diffusion_current) / DIFFUSION_BYPASS_FADE_SAMPLES as f32;
            self.diffusion_remaining = DIFFUSION_BYPASS_FADE_SAMPLES;
        }
    }

    fn apply_tank_size(&mut self, size: f32) {
        let scale = SR_SCALE * size;
        self.l_apf1
            .set_delay_samples(672.0 * scale, L_APF1_MAX - 32);
        self.l_del1_fraction = self.l_del1.set_tank_time(4453.0 * scale);
        self.l_apf2
            .set_delay_samples(1800.0 * scale, L_APF2_MAX - 2);
        self.l_del2_fraction = self.l_del2.set_tank_time(3720.0 * scale);

        self.r_apf1
            .set_delay_samples(908.0 * scale, R_APF1_MAX - 32);
        self.r_del1_fraction = self.r_del1.set_tank_time(4217.0 * scale);
        self.r_apf2
            .set_delay_samples(2656.0 * scale, R_APF2_MAX - 2);
        self.r_del2_fraction = self.r_del2.set_tank_time(3163.0 * scale);
    }

    pub(crate) fn set_predelay_live(&mut self, value: f32) {
        let value = clamp(value, 0.0, MAX_PREDELAY_MS);
        self.retarget_predelay(value);
        self.params.predelay_ms = value;
    }

    fn retarget_predelay(&mut self, milliseconds: f32) {
        let samples = ((milliseconds / 1000.0) * SAMPLE_RATE) as usize;
        if samples == self.predelay_samples {
            return;
        }
        if self.params.freeze {
            self.predelay_samples = samples;
            self.predelay_remaining = 0;
            self.predelay_previous = 0.0;
            return;
        }
        self.predelay_use_anchor = self.predelay_remaining != 0;
        if self.predelay_use_anchor {
            // A second edit during a fade starts at the last emitted value.
            self.predelay_anchor = self.predelay_previous;
        } else {
            self.predelay_from_samples = self.predelay_samples;
        }
        self.predelay_samples = samples;
        self.predelay_remaining = PREDELAY_FADE_SAMPLES;
    }

    pub(crate) fn set_size_live(&mut self, value: f32) {
        let value = clamp(value, 0.01, MAX_SIZE);
        if self.params.size != value {
            self.begin_output_transition();
        }
        self.params.size = value;
        self.apply_tank_size(value);
    }

    pub(crate) fn set_diffusion_live(&mut self, value: f32) {
        let value = clamp(value, 0.0, 1.0);
        self.retarget_diffusion(value);
        self.params.diffusion = value;
    }

    fn begin_output_transition(&mut self) {
        self.transition_anchor = self.previous_output;
        self.transition_remaining = OUTPUT_FADE_SAMPLES;
    }

    #[inline(always)]
    fn apply_diffusion_gain(&mut self) {
        let gain = 0.7 * self.diffusion_current;
        self.l_apf1.gain = -gain;
        self.l_apf2.gain = gain;
        self.r_apf1.gain = -gain;
        self.r_apf2.gain = gain;
    }

    #[inline(always)]
    fn advance_diffusion_mix(&mut self) -> f32 {
        if self.diffusion_remaining != 0 {
            self.diffusion_remaining -= 1;
            if self.diffusion_remaining == 0 {
                self.diffusion_mix = if self.params.diffusion == 0.0 {
                    0.0
                } else {
                    1.0
                };
                self.diffusion_current = self.params.diffusion;
            } else {
                self.diffusion_mix += self.diffusion_step;
                self.diffusion_current += self.diffusion_gain_step;
            }
            self.apply_diffusion_gain();
        }
        self.diffusion_mix
    }

    // The audio callback updates these controls without rebuilding delays or
    // filters; a freeze edge needs the same output transition as the full setter.
    pub(crate) fn set_live_parameters(&mut self, params: PlateauParameters) {
        if self.params.freeze != params.freeze {
            self.begin_output_transition();
        }
        if params.freeze && !self.params.freeze {
            self.clear_input_filters();
        }
        if params.freeze {
            self.predelay_remaining = 0;
            self.predelay_previous = 0.0;
        }
        self.params.decay = clamp(params.decay, 0.1, 0.999);
        self.params.mod_depth = clamp(params.mod_depth, 0.0, 16.0);
        self.params.freeze = params.freeze;
    }

    #[inline(always)]
    fn process_predelay(&mut self, mono_in: f32) -> f32 {
        let target = if self.predelay_samples == 0 {
            mono_in
        } else {
            self.predelay.read(self.predelay_samples)
        };
        let predelayed = if self.predelay_remaining == 1 {
            self.predelay_remaining = 0;
            target
        } else if self.predelay_remaining != 0 {
            let from = if self.predelay_use_anchor {
                self.predelay_anchor
            } else if self.predelay_from_samples == 0 {
                mono_in
            } else {
                self.predelay.read(self.predelay_from_samples)
            };
            let fade = (PREDELAY_FADE_SAMPLES - self.predelay_remaining + 1) as f32
                / PREDELAY_FADE_SAMPLES as f32;
            self.predelay_remaining -= 1;
            from + (target - from) * fade
        } else {
            target
        };
        self.predelay_previous = if self.params.freeze { 0.0 } else { predelayed };
        // Advance the ring during freeze without retaining audio received
        // while input to the tank is disabled.
        self.predelay
            .write(if self.params.freeze { 0.0 } else { mono_in });

        // Stop feeding the input filters while frozen; let their history drain.
        if self.params.freeze { 0.0 } else { predelayed }
    }

    #[inline(always)]
    pub fn process_sample(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        // Check every history before updating any state. An invalid cursor or
        // length must not reach a bounds panic in an audio callback; the
        // owner must reset or replace the damaged processor before reuse.
        if !self.predelay.valid(1, PREDELAY_MAX)
            || !self.in_apf1.delay.valid(2, IN_APF1_MAX)
            || !self.in_apf2.delay.valid(2, IN_APF2_MAX)
            || !self.in_apf3.delay.valid(2, IN_APF3_MAX)
            || !self.in_apf4.delay.valid(2, IN_APF4_MAX)
            || !self.l_apf1.delay.valid(2, L_APF1_MAX)
            || !self.l_del1.valid(1, L_DEL1_MAX - 1)
            || !self.l_apf2.delay.valid(2, L_APF2_MAX)
            || !self.l_del2.valid(1, L_DEL2_MAX - 1)
            || !self.r_apf1.delay.valid(2, R_APF1_MAX)
            || !self.r_del1.valid(1, R_DEL1_MAX - 1)
            || !self.r_apf2.delay.valid(2, R_APF2_MAX)
            || !self.r_del2.valid(1, R_DEL2_MAX - 1)
        {
            return (0.0, 0.0);
        }
        // Do not let corrupt samples poison the feedback tank.
        let in_l = audio_input(in_l);
        let in_r = audio_input(in_r);
        // The reference blocks DC on both channels before summing. Their
        // identical linear filters can share one history after the sum.
        // Discard frozen input before filtering so it cannot leak on release.
        let input = if self.params.freeze {
            0.0
        } else {
            0.5 * in_l + 0.5 * in_r
        };
        let filtered = self.input_dc.process(input);
        let filtered = self.in_hp.process(self.in_lp.process(filtered));
        let tank_in = self.process_predelay(filtered);

        // 2. Input Diffusers
        let mix = self.advance_diffusion_mix();
        let diff1 = self.in_apf1.process(tank_in);
        let diff2 = self.in_apf2.process(diff1);
        let diff3 = self.in_apf3.process(diff2);
        let diffused = self.in_apf4.process(diff3);
        // A zero-gain allpass is still a delay. Bypass the input diffusers
        // at zero, but keep their histories advancing for subsequent edits.
        let diffused = if mix == 0.0 {
            tank_in
        } else if mix == 1.0 {
            diffused
        } else {
            tank_in * (1.0 - mix) + diffused * mix
        };
        // The diffusers may still contain pre-freeze audio. Do not inject it
        // into the frozen feedback tank while that history drains.
        let diff_in = if self.params.freeze { 0.0 } else { diffused };

        // 3. Modulations
        let lfo1_val = self.lfo1.step();
        let lfo2_val = self.lfo2.step();
        let lfo3_val = self.lfo3.step();
        let lfo4_val = self.lfo4.step();
        // Interpolating a moving delay dissipates energy in the feedback
        // loop. Hold it at an integer delay during freeze while advancing the
        // LFOs, so the tank sustains and modulation resumes on release.
        let frozen = self.params.freeze;
        let depth = if frozen { 0.0 } else { self.params.mod_depth };
        let fraction = |value| if frozen { 0.0 } else { value };
        let l_mod =
            self.l_apf1.delay_time as f32 + fraction(self.l_apf1.delay_fraction) + lfo1_val * depth;
        let l_mod2 =
            self.l_apf2.delay_time as f32 + fraction(self.l_apf2.delay_fraction) + lfo2_val * depth;
        let r_mod =
            self.r_apf1.delay_time as f32 + fraction(self.r_apf1.delay_fraction) + lfo3_val * depth;
        let r_mod2 =
            self.r_apf2.delay_time as f32 + fraction(self.r_apf2.delay_fraction) + lfo4_val * depth;

        let decay = if self.params.freeze {
            1.0
        } else {
            self.params.decay
        };
        self.advance_damping();

        // 4. Left Tank Loop
        self.l_sum += diff_in;
        let l_ap1_out = self.l_apf1.process_modulated(self.l_sum, l_mod);
        // Read the configured delay from continuously advancing history.
        let l_del1_out = self.l_del1.read_tank(fraction(self.l_del1_fraction));
        self.l_del1.write(l_ap1_out);

        // Keep damping history current while bypassed so releasing freeze
        // does not reintroduce samples from before it was engaged.
        let l_damped = self.l_hp.process(self.l_lp.process(l_del1_out));
        let l_filtered = if self.params.freeze {
            l_del1_out
        } else {
            l_damped
        };

        let l_ap2_out = self.l_apf2.process_modulated(l_filtered * decay, l_mod2);
        let l_del2_out = self.l_del2.read_tank(fraction(self.l_del2_fraction));
        self.l_del2.write(l_ap2_out);

        // 5. Right Tank Loop
        self.r_sum += diff_in;
        let r_ap1_out = self.r_apf1.process_modulated(self.r_sum, r_mod);
        let r_del1_out = self.r_del1.read_tank(fraction(self.r_del1_fraction));
        self.r_del1.write(r_ap1_out);

        let r_damped = self.r_hp.process(self.r_lp.process(r_del1_out));
        let r_filtered = if self.params.freeze {
            r_del1_out
        } else {
            r_damped
        };

        let r_ap2_out = self.r_apf2.process_modulated(r_filtered * decay, r_mod2);
        let r_del2_out = self.r_del2.read_tank(fraction(self.r_del2_fraction));
        self.r_del2.write(r_ap2_out);

        // Cross feedback
        self.l_sum = r_del2_out * decay;
        self.r_sum = l_del2_out * decay;

        // Plateau's output tap times scale with sample rate, not tank size.
        let tap = |val: f32| (val * SR_SCALE) as usize;

        let mut out_l = l_ap1_out + self.l_del1.tap(tap(266.0)) + self.l_del1.tap(tap(2974.0))
            - self.l_apf2.delay.tap(tap(1913.0))
            + self.l_del2.tap(tap(1996.0))
            - self.r_del1.tap(tap(1990.0))
            - self.r_apf2.delay.tap(tap(187.0))
            - self.r_del2.tap(tap(1066.0));

        let mut out_r = r_ap1_out + self.r_del1.tap(tap(266.0)) + self.r_del1.tap(tap(2974.0))
            - self.r_apf2.delay.tap(tap(1913.0))
            + self.r_del2.tap(tap(1996.0))
            - self.l_del1.tap(tap(1990.0))
            - self.l_apf2.delay.tap(tap(187.0))
            - self.l_del2.tap(tap(1066.0));

        // Plateau's tank output is halved after the DC-blocking filters.
        out_l = self.out_hp_l.process(out_l) * 0.5;
        out_r = self.out_hp_r.process(out_r) * 0.5;

        // Continuous cubic soft clipper to prevent harsh digital clipping
        out_l = soft_clip(out_l);
        out_r = soft_clip(out_r);
        if self.transition_remaining != 0 {
            let fade = (OUTPUT_FADE_SAMPLES - self.transition_remaining + 1) as f32
                / OUTPUT_FADE_SAMPLES as f32;
            out_l = self.transition_anchor[0] * (1.0 - fade) + out_l * fade;
            out_r = self.transition_anchor[1] * (1.0 - fade) + out_r * fade;
            self.transition_remaining -= 1;
        }
        self.previous_output = [out_l, out_r];
        (out_l * self.params.wet, out_r * self.params.wet)
    }

    // The caller must guarantee all histories and filter state are zero and
    // revoke that guarantee on arbitrary mutable access. Advancing cursors and
    // LFOs preserves the state needed when processing resumes.
    pub(crate) fn advance_pristine(&mut self) {
        macro_rules! advance {
            ($d:expr) => {{
                let d = &mut $d;
                d.write_pos += 1;
                if d.write_pos >= d.buffer.len() {
                    d.write_pos = 0;
                }
            }};
        }
        advance!(self.predelay);
        self.predelay_previous = 0.0;
        self.predelay_remaining = self.predelay_remaining.saturating_sub(1);
        advance!(self.in_apf1.delay);
        advance!(self.in_apf2.delay);
        advance!(self.in_apf3.delay);
        advance!(self.in_apf4.delay);
        advance!(self.l_apf1.delay);
        advance!(self.l_del1);
        advance!(self.l_apf2.delay);
        advance!(self.l_del2);
        advance!(self.r_apf1.delay);
        advance!(self.r_del1);
        advance!(self.r_apf2.delay);
        advance!(self.r_del2);
        self.lfo1.step();
        self.lfo2.step();
        self.lfo3.step();
        self.lfo4.step();
        self.advance_diffusion_mix();
        self.advance_damping();
        self.transition_remaining = self.transition_remaining.saturating_sub(1);
    }

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
    use super::{
        Allpass, Delay, MAX_SIZE, PlateauParameters, PlateauReverb, SAMPLE_RATE, SR_SCALE,
        TriangleLFO,
    };

    #[test]
    fn triangle_modulation_starts_at_reference_phase() {
        // TriSawLFO at the equivalent 1/8-cycle increment in the supplied
        // Multistomp core, sampled across its first wrap.
        let mut lfo = TriangleLFO::new(SAMPLE_RATE / 8.0);
        let expected = [-1.0, -0.5, 0.0, 0.5, 1.0, 0.5, 0.0, -0.5, -1.0, -0.5];
        for value in expected {
            assert_eq!(lfo.step(), value);
        }
    }

    #[test]
    fn allpass_impulse_timing_matches_multistomp_core() {
        // AllpassFilter from ZoomMultistompZDL commit 7ea50ae6, with a
        // 16-sample ring, gain 0.5 and delay 3, processing a unit impulse.
        for (delay, expected) in [
            (3.0, [-0.5, 0.0, 0.0, 0.0, 0.75, 0.0, 0.0, 0.0, 0.375, 0.0]),
            (
                3.5,
                [-0.5, 0.0, 0.0, 0.0, 0.375, 0.375, 0.0, 0.0, 0.09375, 0.1875],
            ),
        ] {
            let mut ap = Allpass::<16>::new(0.5, 3);
            ap.set_delay_samples(delay, 14);
            for (frame, expected) in expected.into_iter().enumerate() {
                assert_eq!(ap.process(if frame == 0 { 1.0 } else { 0.0 }), expected);
            }
        }
    }

    #[test]
    fn modulated_allpass_uses_the_preceding_delay_read() {
        // Same reference, changing its fractional delay before each frame.
        let times = [3.0, 3.5, 2.25, 4.0, 3.0, 2.5, 3.75, 2.0, 3.0, 4.0, 2.5, 3.0];
        let expected = [
            -0.5,
            0.0,
            0.0,
            0.5625,
            0.0,
            0.0,
            0.140625,
            0.0703125,
            0.0,
            0.0,
            0.0,
            0.017578125,
        ];
        let mut ap = Allpass::<16>::new(0.5, 3);
        for (frame, (delay, expected)) in times.into_iter().zip(expected).enumerate() {
            assert_eq!(
                ap.process_modulated(if frame == 0 { 1.0 } else { 0.0 }, delay),
                expected
            );
        }
    }

    #[test]
    fn tank_delay_interpolates_between_adjacent_history_samples() {
        let mut delay = Delay::<16>::new();
        let fraction = delay.set_tank_time(10.5);
        let mut output = [0.0; 13];
        for (frame, sample) in output.iter_mut().enumerate() {
            *sample = delay.read_tank(fraction);
            delay.write(if frame == 0 { 1.0 } else { 0.0 });
        }
        assert_eq!(output[10], 0.5);
        assert_eq!(output[11], 0.5);
        assert_eq!(output.iter().sum::<f32>(), 1.0);
        let fraction = delay.set_tank_time(1000.0);
        assert_eq!((delay.length, fraction), (15, 0.0));
    }

    #[test]
    fn configured_size_retains_reference_fractions() {
        let mut reverb = Box::new(PlateauReverb::new());
        let params = PlateauParameters {
            size: 1.0,
            ..PlateauParameters::default()
        };
        reverb.init(params);
        let expected = 4453.0 * SR_SCALE;
        assert_eq!(
            reverb.l_del1.length as f32 + reverb.l_del1_fraction,
            expected
        );
        let expected_apf = 141.0 * SR_SCALE;
        assert_eq!(
            reverb.in_apf1.delay_time as f32 + reverb.in_apf1.delay_fraction,
            expected_apf
        );
        let expected_mod_apf = 2656.0 * SR_SCALE;
        assert_eq!(
            reverb.r_apf2.delay_time as f32 + reverb.r_apf2.delay_fraction,
            expected_mod_apf
        );
        reverb.set_size_live(MAX_SIZE);
        let scale = SR_SCALE * MAX_SIZE;
        for (actual, expected) in [
            (
                reverb.l_apf1.delay_time as f32 + reverb.l_apf1.delay_fraction,
                672.0 * scale,
            ),
            (
                reverb.l_apf2.delay_time as f32 + reverb.l_apf2.delay_fraction,
                1800.0 * scale,
            ),
            (
                reverb.r_apf1.delay_time as f32 + reverb.r_apf1.delay_fraction,
                908.0 * scale,
            ),
            (
                reverb.r_apf2.delay_time as f32 + reverb.r_apf2.delay_fraction,
                2656.0 * scale,
            ),
            (
                reverb.l_del1.length as f32 + reverb.l_del1_fraction,
                4453.0 * scale,
            ),
            (
                reverb.l_del2.length as f32 + reverb.l_del2_fraction,
                3720.0 * scale,
            ),
            (
                reverb.r_del1.length as f32 + reverb.r_del1_fraction,
                4217.0 * scale,
            ),
            (
                reverb.r_del2.length as f32 + reverb.r_del2_fraction,
                3163.0 * scale,
            ),
        ] {
            assert_eq!(actual, expected);
        }
    }

    #[cfg(experimental_plateau_32k)]
    #[test]
    fn full_range_32k_experiment_retains_delays_and_renders() {
        let mut reverb = Box::new(PlateauReverb::new());
        let params = PlateauParameters {
            size: 4.0,
            predelay_ms: 500.0,
            mod_depth: 16.0,
            ..PlateauParameters::default()
        };
        reverb.init(params);
        assert_eq!(reverb.params.size, 4.0);
        assert_eq!(reverb.predelay_samples, 16_000);
        assert!(reverb.predelay_samples < reverb.predelay.buffer.len());
        for (delay, fraction, capacity) in [
            (
                reverb.l_apf1.delay_time,
                reverb.l_apf1.delay_fraction,
                reverb.l_apf1.delay.buffer.len(),
            ),
            (
                reverb.l_apf2.delay_time,
                reverb.l_apf2.delay_fraction,
                reverb.l_apf2.delay.buffer.len(),
            ),
            (
                reverb.r_apf1.delay_time,
                reverb.r_apf1.delay_fraction,
                reverb.r_apf1.delay.buffer.len(),
            ),
            (
                reverb.r_apf2.delay_time,
                reverb.r_apf2.delay_fraction,
                reverb.r_apf2.delay.buffer.len(),
            ),
        ] {
            assert!(delay > params.mod_depth as usize);
            assert!(delay as f32 + fraction + params.mod_depth <= (capacity - 2) as f32);
        }
        for (tap, capacity) in [
            (2974.0, reverb.l_del1.buffer.len()),
            (1913.0, reverb.l_apf2.delay.buffer.len()),
            (1996.0, reverb.l_del2.buffer.len()),
            (2974.0, reverb.r_del1.buffer.len()),
            (1913.0, reverb.r_apf2.delay.buffer.len()),
            (1996.0, reverb.r_del2.buffer.len()),
        ] {
            assert!(((tap * SR_SCALE) as usize) < capacity);
        }
        let mut peak = 0.0f32;
        for frame in 0..96_000 {
            let input = if frame == 0 { 0.1 } else { 0.0 };
            let (left, right) = reverb.process_sample(input, input);
            assert!(left.is_finite() && right.is_finite());
            if frame < 16_000 {
                assert_eq!((left, right), (0.0, 0.0));
            }
            peak = peak.max(left.abs()).max(right.abs());
        }
        assert!(peak > 0.0001);
        reverb.set_size_live(0.0);
        reverb.set_predelay_live(1000.0);
        assert_eq!(reverb.params.size, 0.01);
        assert_eq!(reverb.params.predelay_ms, 500.0);
    }

    #[test]
    fn modulated_allpass_handles_subsample_delay_and_clear() {
        // Short-delay reference exercises the just-written sample as well as
        // interpolation; ordinary Plateau settings are longer.
        let times = [0.0, 0.5, 1.0, 0.25, 0.0, 1.5, 0.75, 0.0, 2.0, 1.0, 0.5, 0.0];
        let expected = [
            -0.5,
            0.75,
            0.5625,
            0.375,
            0.2109375,
            0.10546875,
            0.146484375,
            0.0578613281,
            0.0289306641,
            0.0732421875,
            0.014465332,
            0.0219268799,
        ];
        let mut ap = Allpass::<16>::new(0.5, 0);
        for (frame, (delay, expected)) in times.into_iter().zip(expected).enumerate() {
            let actual = ap.process_modulated(if frame == 0 { 1.0 } else { 0.0 }, delay);
            assert!((actual - expected).abs() < 1e-7, "frame {frame}: {actual}");
        }
        ap.clear();
        assert_eq!(ap.process_modulated(0.0, 0.0), 0.0);
    }
}
