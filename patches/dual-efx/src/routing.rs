//! Dual-Effect Routing and Signal Flow Matrix
//!
//! Handles per-channel send accumulation, cascading (A -> B),
//! and summing to the master return bus. Pre-AINR is not implemented.

use super::plateau::{PlateauParameters, PlateauReverb};
use super::velvet::{MonoHistory, Parameters as VelvetParameters, VelvetDecorrelator};

pub const MAX_CHANNELS: usize = 12; // Four mono strips and four paired strips.
pub const BLOCK_SIZE: usize = 64; // Max block size processed per callback
const CASCADE_FADE_SAMPLES: u32 = 64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum RoutingMode {
    Neither = 0,
    SendA = 1,    // Velvet Noise Decorrelator
    SendB = 2,    // Plateau Plate Reverb
    SendAtoB = 3, // Velvet Noise cascaded into Plateau Reverb
}

impl RoutingMode {
    pub fn from_u8(val: u8) -> Self {
        match val {
            1 => Self::SendA,
            2 => Self::SendB,
            3 => Self::SendAtoB,
            _ => Self::Neither,
        }
    }
}

/// One mono mixer contribution before routing to an effect bus.
/// `gain` may be signed or exceed unity; `ramp`, when present, supplies a
/// multiplier per frame.
#[derive(Clone, Copy)]
pub struct WeightedChannel<'a> {
    pub samples: &'a [f32],
    pub gain: f32,
    pub ramp: Option<&'a [f32]>,
}

/// Scratch buses for weighted routing. Each concurrent processor needs its
/// own exclusively borrowed workspace.
pub struct RoutingWorkspace {
    buses: [[f32; BLOCK_SIZE]; 3],
}

impl RoutingWorkspace {
    pub const fn new() -> Self {
        Self { buses: [[0.0; BLOCK_SIZE]; 3] }
    }

    /// Initializes caller-owned storage without constructing a large stack value.
    pub fn initialize_in_place(storage: &mut core::mem::MaybeUninit<Self>) -> &mut Self {
        // Every field is an f32 array, so all-zero bytes are valid zeros.
        unsafe {
            core::ptr::write_bytes(storage.as_mut_ptr(), 0, 1);
            storage.assume_init_mut()
        }
    }
}

fn strip_slots(strip: usize) -> Option<core::ops::Range<usize>> {
    match strip {
        0..=3 => Some(strip..strip + 1),
        4..=7 => {
            let first = 4 + (strip - 4) * 2;
            Some(first..first + 2)
        }
        _ => None,
    }
}

/// Skips reverb arithmetic only while its freshly cleared state is untouched.
/// A mutable borrow revokes that guarantee; validated parameter updates do not.
pub struct IdlePlateau {
    inner: PlateauReverb,
    pristine: bool,
    #[cfg(experimental_plateau_32k)]
    rate: super::plateau_rate::PlateauRate,
}

impl IdlePlateau {
    pub const fn new() -> Self {
        Self { inner: PlateauReverb::new(), pristine: false,
            #[cfg(experimental_plateau_32k)]
            rate: super::plateau_rate::PlateauRate::new(),
        }
    }

    pub fn init(&mut self, params: PlateauParameters) {
        self.inner.init(params);
        self.pristine = true;
        #[cfg(experimental_plateau_32k)]
        self.rate.clear();
    }

    #[cfg(experimental_plateau_32k)]
    pub fn clear(&mut self) {
        self.inner.clear();
        self.rate.clear();
        self.pristine = true;
    }

    pub fn set_parameters(&mut self, params: PlateauParameters) {
        #[cfg(experimental_plateau_32k)]
        if params.freeze && !self.inner.params.freeze { self.rate.clear_input(); }
        self.inner.set_parameters(params);
    }

    /// Applies parameters to a freshly cleared processor without re-clearing histories.
    pub(crate) fn set_parameters_fresh(&mut self, params: PlateauParameters) {
        self.inner.set_parameters_fresh(params);
    }

    pub(crate) fn set_predelay_live(&mut self, value: f32) { self.inner.set_predelay_live(value); }
    pub(crate) fn set_size_live(&mut self, value: f32) { self.inner.set_size_live(value); }
    pub(crate) fn set_diffusion_live(&mut self, value: f32) { self.inner.set_diffusion_live(value); }

    /// Update controls consumed directly per sample, without recalculating
    /// delay lengths or filter coefficients on the audio task.
    pub(crate) fn set_live_parameters(&mut self, params: PlateauParameters) {
        #[cfg(experimental_plateau_32k)]
        if params.freeze && !self.inner.params.freeze { self.rate.clear_input(); }
        self.inner.set_live_parameters(params);
    }

    pub fn process_sample(&mut self, l: f32, r: f32) -> (f32, f32) {
        #[cfg(experimental_plateau_32k)]
        {
            let inner = &mut self.inner;
            let pristine = &mut self.pristine;
            let (l, r) = if inner.params.freeze { (0.0, 0.0) } else { (l, r) };
            self.rate.process(l, r, |l, r| Self::process_internal(inner, pristine, l, r))
        }
        #[cfg(not(experimental_plateau_32k))]
        {
            Self::process_internal(&mut self.inner, &mut self.pristine, l, r)
        }
    }

    #[inline(always)]
    fn process_internal(inner: &mut PlateauReverb, pristine: &mut bool, l: f32, r: f32) -> (f32, f32) {
        // Requiring positive zero preserves the ordinary path for signed zero.
        if *pristine && l.to_bits() == 0 && r.to_bits() == 0 {
            inner.advance_pristine();
            (0.0, 0.0)
        } else {
            *pristine = false;
            inner.process_sample(l, r)
        }
    }
}

impl core::ops::Deref for IdlePlateau {
    type Target = PlateauReverb;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl core::ops::DerefMut for IdlePlateau {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.pristine = false;
        &mut self.inner
    }
}

pub struct DualEfxProcessor {
    pub velvet: VelvetDecorrelator,
    // A separate history keeps A-only sends out of the A-to-B cascade.
    // Both histories use the same Velvet filter timeline.
    cascade_history: MonoHistory,
    pub plateau: IdlePlateau,

    // Per-channel routing modes (12 mixer channels)
    pub channel_routing: [RoutingMode; MAX_CHANNELS],
    // Per-channel send levels (0.0 to 1.0)
    pub channel_send: [f32; MAX_CHANNELS],

    // Global cascade amount (0.0 to 1.0) for SendAtoB channels
    pub cascade_gain: f32,
    cascade_current: f32,
    cascade_target: f32,
    cascade_step: f32,
    cascade_remaining: u32,
    // Effect return levels
    pub return_gain_a: f32,
    pub return_gain_b: f32,

    // Reserved for future Pre-AINR integration; currently has no effect.
    pub pre_ainr_enabled: bool,
}

impl DualEfxProcessor {
    pub const fn new() -> Self {
        Self {
            velvet: VelvetDecorrelator::new(),
            cascade_history: MonoHistory::new(),
            plateau: IdlePlateau::new(),
            channel_routing: [RoutingMode::Neither; MAX_CHANNELS],
            channel_send: [0.0; MAX_CHANNELS],
            cascade_gain: 1.0,
            cascade_current: 1.0,
            cascade_target: 1.0,
            cascade_step: 0.0,
            cascade_remaining: 0,
            return_gain_a: 1.0,
            return_gain_b: 1.0,
            pre_ainr_enabled: false,
        }
    }

    /// Constructs and initializes the processor in caller-owned storage.
    ///
    /// The storage must be reserved exclusively for this processor for the
    /// returned borrow's lifetime. Initialization clears hundreds of KiB and
    /// generates filter banks, so it must not run in a real-time callback.
    pub fn initialize_in_place(storage: &mut core::mem::MaybeUninit<Self>) -> &mut Self {
        // All fields admit the all-zero representation: floats and integers,
        // false bools, and RoutingMode::Neither. No typed reference may be
        // created until the bytes are valid. Keep this invariant current when
        // adding fields to this type or either DSP engine.
        unsafe {
            core::ptr::write_bytes(storage.as_mut_ptr(), 0, 1);
            let processor = storage.assume_init_mut();
            processor.init();
            processor
        }
    }

    pub fn init(&mut self) {
        self.channel_routing.fill(RoutingMode::Neither);
        self.channel_send.fill(0.0);
        self.cascade_gain = 1.0;
        self.cascade_current = 1.0;
        self.cascade_target = 1.0;
        self.cascade_step = 0.0;
        self.cascade_remaining = 0;
        self.return_gain_a = 1.0;
        self.return_gain_b = 1.0;
        self.pre_ainr_enabled = false;
        self.velvet.init(VelvetParameters::default());
        self.cascade_history.clear();
        self.plateau.init(PlateauParameters::default());
    }

    pub fn set_channel_routing(&mut self, channel: usize, mode: RoutingMode) {
        if channel < MAX_CHANNELS {
            self.channel_routing[channel] = mode;
        }
    }

    /// Sets routing for one zero-based L6max 1.10 channel strip, using the
    /// mapping observed in the setup-complete fixture. Other link states have
    /// not been qualified. The first four strips use one slot; the rest use pairs.
    pub fn set_strip_routing(&mut self, strip: usize, mode: RoutingMode) {
        if let Some(slots) = strip_slots(strip) {
            self.channel_routing[slots].fill(mode);
        }
    }

    /// Cycles a strip's route by signed encoder steps, updating paired slots
    /// together. This mapping is specific to the observed L6max fixture.
    pub fn cycle_strip_routing(&mut self, strip: usize, steps: i32) -> Option<RoutingMode> {
        let slots = strip_slots(strip)?;
        let current = self.channel_routing[slots.start] as i32;
        let next = RoutingMode::from_u8(((current + steps % 4 + 4) % 4) as u8);
        self.channel_routing[slots].fill(next);
        Some(next)
    }

    pub fn set_channel_send(&mut self, channel: usize, level: f32) {
        if channel < MAX_CHANNELS {
            self.channel_send[channel] = if level.is_nan() || level < 0.0 {
                0.0
            } else if level > 1.0 {
                1.0
            } else {
                level
            };
        }
    }

    // Only live encoder edits request a fade. Direct field writes retain their
    // existing immediate behavior, including initialization and restoration.
    pub(crate) fn set_cascade_gain_smooth(&mut self, gain: f32) {
        if gain == self.cascade_gain { return; }
        if self.cascade_remaining == 0 || self.cascade_gain.to_bits() != self.cascade_target.to_bits() {
            self.cascade_current = self.cascade_gain;
        }
        self.cascade_gain = gain;
        self.cascade_target = gain;
        self.cascade_step = (gain - self.cascade_current) / CASCADE_FADE_SAMPLES as f32;
        self.cascade_remaining = CASCADE_FADE_SAMPLES;
    }

    #[inline(always)]
    fn advance_cascade_gain(&mut self) -> f32 {
        if self.cascade_remaining == 0 || self.cascade_gain.to_bits() != self.cascade_target.to_bits() {
            self.cascade_remaining = 0;
            return self.cascade_gain;
        }
        self.cascade_remaining -= 1;
        if self.cascade_remaining == 0 {
            self.cascade_current = self.cascade_target;
        } else {
            self.cascade_current += self.cascade_step;
        }
        self.cascade_current
    }

    /// Processes a block of audio frames.
    ///
    /// - `channels`: slice of channel input buffers [ch][frame].
    /// - `out_l`, `out_r`: destination buffers for the wet effect return to master bus.
    pub fn process_block(&mut self, channels: &[&[f32]], out_l: &mut [f32], out_r: &mut [f32]) {
        let mut weighted = [WeightedChannel { samples: &[], gain: 0.0, ramp: None }; MAX_CHANNELS];
        let count = channels.len().min(MAX_CHANNELS);
        for ch in 0..count {
            weighted[ch] = WeightedChannel { samples: channels[ch], gain: self.channel_send[ch], ramp: None };
        }
        self.process_weighted_block(&weighted[..count], out_l, out_r);
    }

    /// Routes channels whose mixer gains have already been calculated.
    /// Samples and optional ramps shorter than the output contribute silence
    /// beyond their lengths; effects still run to preserve their tails. Invalid
    /// or extreme weighted contributions are discarded before bus summation.
    pub fn process_weighted_block(&mut self, channels: &[WeightedChannel<'_>], out_l: &mut [f32], out_r: &mut [f32]) {
        let mut workspace = RoutingWorkspace::new();
        self.process_weighted_with_workspace(channels, out_l, out_r, &mut workspace);
    }

    /// Routes weighted channels using caller-owned scratch, avoiding the
    /// block-sized temporary buses in the caller's stack frame.
    pub fn process_weighted_with_workspace(&mut self, channels: &[WeightedChannel<'_>], out_l: &mut [f32], out_r: &mut [f32], workspace: &mut RoutingWorkspace) {
        let frames = out_l.len().min(out_r.len());
        let mut offset = 0;
        while offset < frames {
            // Each physical input is mono here, so accumulate it only once per bus.
            let [send_a, send_b, send_cascade] = &mut workspace.buses;
            send_a.fill(0.0);
            send_b.fill(0.0);
            send_cascade.fill(0.0);
            let n = (frames - offset).min(BLOCK_SIZE);

            for ch in 0..channels.len().min(MAX_CHANNELS) {
                let channel = channels[ch];
                if !channel.gain.is_finite() || channel.gain == 0.0 {
                    continue;
                }
                let bus = match self.channel_routing[ch] {
                    RoutingMode::Neither => continue,
                    RoutingMode::SendA => &mut *send_a,
                    RoutingMode::SendB => &mut *send_b,
                    RoutingMode::SendAtoB => &mut *send_cascade,
                };
                let input = channel.samples.get(offset..).unwrap_or(&[]);
                if let Some(ramp) = channel.ramp {
                    let ramp = ramp.get(offset..).unwrap_or(&[]);
                    for i in 0..n.min(input.len()).min(ramp.len()) {
                        bus[i] += super::math::audio_input((input[i] * ramp[i]) * channel.gain);
                    }
                } else {
                    for i in 0..n.min(input.len()) {
                        bus[i] += super::math::audio_input(input[i] * channel.gain);
                    }
                }
            }

            self.process_buses(
                &send_a[..n], &send_b[..n], &send_cascade[..n],
                &mut out_l[offset..offset + n], &mut out_r[offset..offset + n],
            );
            offset += n;
        }
    }

    /// Processes mono sends that have already been weighted and accumulated
    /// separately for A, B, and A-to-B. All effects continue through silence
    /// so that closing a send does not cut their tails.
    pub fn process_buses(
        &mut self,
        send_a: &[f32],
        send_b: &[f32],
        send_cascade: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) {
        let frames = send_a.len().min(send_b.len()).min(send_cascade.len())
            .min(out_l.len()).min(out_r.len());
        if frames == 0 {
            return;
        }
        for i in 0..frames {
            (out_l[i], out_r[i]) = self.process_bus_sample(send_a[i], send_b[i], send_cascade[i]);
        }
    }

    /// Processes one frame, allowing firmware control updates to remain
    /// interleaved with audio frames.
    #[inline(always)]
    pub fn process_bus_sample(&mut self, send_a: f32, send_b: f32, send_cascade: f32) -> (f32, f32) {
        let cascade_gain = self.advance_cascade_gain();
        let ((a_l, a_r), (cascade_l, cascade_r)) = self.velvet
            .process_shared_mono(&mut self.cascade_history, send_a, send_cascade);
        // Reject a corrupt direct send before it can contaminate the separate
        // cascade contribution to the reverb.
        let send_b = super::math::audio_input(send_b);
        let (wet_b_l, wet_b_r) = self.plateau.process_sample(
            send_b + cascade_l * cascade_gain,
            send_b + cascade_r * cascade_gain,
        );
        (
            super::math::soft_clip((a_l + cascade_l) * self.return_gain_a + wet_b_l * self.return_gain_b),
            super::math::soft_clip((a_r + cascade_r) * self.return_gain_a + wet_b_r * self.return_gain_b),
        )
    }

    /// Processes a stereo send block through the dual effect engine.
    /// Used when the stock mixer has already summed channel sends into a stereo bus.
    /// Disabled effects receive silence and remain muted while their state advances.
    pub fn process_stereo_send(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
        enable_a: bool,
        enable_b: bool,
    ) {
        let frames = in_l.len().min(in_r.len()).min(out_l.len()).min(out_r.len());
        if frames == 0 {
            return;
        }
        for i in 0..frames {
            let cascade_gain = self.advance_cascade_gain();
            let in_l = super::math::audio_input(in_l[i]);
            let in_r = super::math::audio_input(in_r[i]);
            let (a_l, a_r) = self.velvet.process_sample(
                if enable_a { in_l } else { 0.0 },
                if enable_a { in_r } else { 0.0 },
            );
            // This path does not route the separate cascade history, but it
            // must continue advancing so a later bus switch cannot replay it.
            self.cascade_history.silence();
            let cascade_l = if enable_a {
                a_l * cascade_gain
            } else {
                0.0
            };
            let cascade_r = if enable_a {
                a_r * cascade_gain
            } else {
                0.0
            };
            let (b_l, b_r) = if enable_b {
                self.plateau
                    .process_sample(in_l + cascade_l, in_r + cascade_r)
            } else {
                self.plateau.process_sample(0.0, 0.0)
            };
            let val_a_l = if enable_a {
                a_l * self.return_gain_a
            } else {
                0.0
            };
            let val_a_r = if enable_a {
                a_r * self.return_gain_a
            } else {
                0.0
            };
            let val_b_l = if enable_b {
                b_l * self.return_gain_b
            } else {
                0.0
            };
            let val_b_r = if enable_b {
                b_r * self.return_gain_b
            } else {
                0.0
            };
            out_l[i] = super::math::soft_clip(val_a_l + val_b_l);
            out_r[i] = super::math::soft_clip(val_a_r + val_b_r);
        }
    }
}
