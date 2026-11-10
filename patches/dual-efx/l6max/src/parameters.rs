//! Parameter modeling, scaling, and OLED string formatting for Dual-EFX.

pub use super::parameter_view::{EfxParamId, FormattedParam, ParameterSnapshot, format_snapshot};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ParameterBank {
    Velvet,
    Plateau,
}

/// Latest applied value for each inspector parameter. One audio task writes
/// individual entries; selection can read them without borrowing DSP state.
/// This does not reserve memory or synchronize callback lifetime.
#[repr(C)]
pub struct ParameterHandoff {
    values: [core::sync::atomic::AtomicU32; 16],
}

impl ParameterHandoff {
    pub const fn new() -> Self {
        Self {
            values: [const { core::sync::atomic::AtomicU32::new(u32::MAX) }; 16],
        }
    }

    pub fn record(&self, id: EfxParamId, processor: &super::routing::DualEfxProcessor) {
        use EfxParamId::*;
        use core::sync::atomic::Ordering;
        // Store native units so restoring percentages does not round twice.
        let value = match id {
            VndAmount => processor.velvet.current.amount,
            // Preserve the request even when the displayed density is rounded
            // to the number of taps that fit the current spread.
            VndDensity => processor.velvet.current.density,
            PlatSize => processor.plateau.params.size,
            PlatDecay => processor.plateau.params.decay,
            PlatDiffusion => processor.plateau.params.diffusion,
            GlobalCascade => processor.cascade_gain,
            _ => snapshot_parameter(id, processor).value,
        };
        self.values[id as usize].store(value.to_bits(), Ordering::Release);
        // Reducing refresh also clamps transition length in set_parameters.
        if id == VndRefresh {
            self.values[VndTransition as usize].store(
                processor.velvet.current.transition_ms.to_bits(),
                Ordering::Release,
            );
        }
    }

    /// Restore into freshly initialized, exclusively owned DSP state.
    /// Individually atomic values do not form a transaction with live edits.
    pub fn restore(&self, processor: &mut super::routing::DualEfxProcessor) {
        use core::sync::atomic::Ordering;
        let mut velvet = processor.velvet.current;
        let mut plateau = processor.plateau.params;
        let mut velvet_changed = false;
        let mut plateau_changed = false;
        for (index, word) in self.values.iter().enumerate() {
            let bits = word.load(Ordering::Acquire);
            if bits == u32::MAX {
                continue;
            }
            let value = f32::from_bits(bits);
            match index {
                0 => velvet.amount = value,
                1 => velvet.length_ms = value,
                2 => velvet.density = value,
                3 => velvet.decay_db = value,
                4 => velvet.refresh_seconds = value,
                5 => velvet.dynamic = value != 0.0,
                6 => plateau.predelay_ms = value,
                7 => plateau.size = value,
                8 => plateau.decay = value,
                9 => plateau.low_cut_hz = value,
                10 => plateau.high_cut_hz = value,
                11 => plateau.diffusion = value,
                12 => plateau.mod_depth = value,
                13 => plateau.freeze = value != 0.0,
                14 => processor.cascade_gain = value,
                15 => velvet.transition_ms = value,
                _ => unreachable!(),
            }
            velvet_changed |= index <= 5 || index == 15;
            plateau_changed |= (6..=13).contains(&index);
        }
        // Rebuild the saved filter before callback publication, rather than
        // crossfading from the newly constructed default filter.
        if velvet_changed {
            processor.velvet.init(velvet);
        }
        if plateau_changed {
            processor.plateau.set_parameters_fresh(plateau);
        }
    }
}

/// Applies one inspector encoder turn to the DSP state and returns the value
/// to present. The caller must provide exclusive access to the processor;
/// firmware event delivery and synchronization are not implemented here.
pub fn edit_encoder(
    processor: &mut super::routing::DualEfxProcessor,
    bank: ParameterBank,
    encoder: usize,
    delta: i32,
) -> Option<EfxParamId> {
    use super::math::clamp;
    use EfxParamId::*;
    if delta == 0 || encoder >= 8 {
        return None;
    }
    let step = delta as f32;
    let id = match bank {
        ParameterBank::Velvet => {
            let mut p = processor.velvet.current;
            let id = match encoder {
                0 => {
                    p.amount += step * 0.02;
                    VndAmount
                }
                1 => {
                    p.length_ms += step;
                    VndTimeSpread
                }
                2 => {
                    p.density = clamp(p.density + step * 50.0, 100.0, 4000.0);
                    VndDensity
                }
                3 => {
                    p.decay_db += step * 2.0;
                    VndDecay
                }
                4 => {
                    p.refresh_seconds = clamp(p.refresh_seconds + step * 0.5, 0.1, 60.0);
                    VndRefresh
                }
                5 => {
                    p.dynamic = delta > 0;
                    VndMode
                }
                6 => {
                    p.transition_ms += step * 25.0;
                    VndTransition
                }
                _ => {
                    processor.set_cascade_gain_smooth(clamp(
                        processor.cascade_gain + step * 0.1,
                        0.0,
                        1.0,
                    ));
                    return Some(GlobalCascade);
                }
            };
            processor.velvet.set_parameters(p);
            id
        }
        ParameterBank::Plateau => {
            let mut p = processor.plateau.params;
            let id = match encoder {
                0 => {
                    p.predelay_ms = clamp(
                        p.predelay_ms + step * 2.0,
                        0.0,
                        super::plateau::MAX_PREDELAY_MS,
                    );
                    PlatPreDelay
                }
                1 => {
                    p.size += step * 0.02;
                    PlatSize
                }
                2 => {
                    p.decay += step * 0.02;
                    PlatDecay
                }
                3 => {
                    p.low_cut_hz = clamp(p.low_cut_hz + step * 20.0, 20.0, 1000.0);
                    PlatLowCut
                }
                4 => {
                    p.high_cut_hz = clamp(p.high_cut_hz + step * 250.0, 1000.0, 16000.0);
                    PlatHighCut
                }
                5 => {
                    p.diffusion += step * 0.05;
                    PlatDiffusion
                }
                6 => {
                    p.mod_depth += step * 0.25;
                    PlatModDepth
                }
                _ => {
                    p.freeze = delta > 0;
                    PlatFreeze
                }
            };
            match encoder {
                0 => processor.plateau.set_predelay_live(p.predelay_ms),
                1 => processor.plateau.set_size_live(p.size),
                2 | 6 | 7 => processor.plateau.set_live_parameters(p),
                5 => processor.plateau.set_diffusion_live(p.diffusion),
                _ => processor.plateau.set_parameters(p),
            }
            id
        }
    };
    Some(id)
}

/// Copies the applied parameter value for later presentation on another task.
pub fn snapshot_parameter(
    id: EfxParamId,
    processor: &super::routing::DualEfxProcessor,
) -> ParameterSnapshot {
    use EfxParamId::*;
    let velvet = processor.velvet.current;
    let plateau = processor.plateau.params;
    let value = match id {
        VndAmount => velvet.amount * 100.0,
        VndTimeSpread => velvet.length_ms,
        // The target count is rounded and bounded at both ends. The audible
        // bank may still be transitioning; retain the request separately.
        VndDensity => {
            processor.velvet.tap_count(velvet.density) as f32
                * (super::velvet::SAMPLE_RATE / processor.velvet.current_tail_samples as f32)
        }
        VndDecay => velvet.decay_db,
        VndRefresh => velvet.refresh_seconds,
        VndTransition => velvet.transition_ms,
        VndMode => {
            if velvet.dynamic {
                1.0
            } else {
                0.0
            }
        }
        PlatPreDelay => plateau.predelay_ms,
        PlatSize => plateau.size * 100.0,
        PlatDecay => plateau.decay * 100.0,
        PlatLowCut => plateau.low_cut_hz,
        PlatHighCut => plateau.high_cut_hz,
        PlatDiffusion => plateau.diffusion * 100.0,
        PlatModDepth => plateau.mod_depth,
        PlatFreeze => {
            if plateau.freeze {
                1.0
            } else {
                0.0
            }
        }
        GlobalCascade => processor.cascade_gain * 100.0,
    };
    ParameterSnapshot { id, value }
}

/// Formats the current DSP value for the L6max OLED notification popup.
pub fn format_parameter(
    id: EfxParamId,
    processor: &super::routing::DualEfxProcessor,
) -> FormattedParam {
    format_snapshot(snapshot_parameter(id, processor))
}
