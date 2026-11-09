#![no_std]
#![no_main]
#![allow(dead_code)]

use core::ptr::{read_volatile as read, write_volatile as write};

mod firmware;
mod math;
mod parameter_view;
mod parameters;
mod plateau;
#[cfg(experimental_plateau_32k)]
mod plateau_rate;
mod routing;
mod trampolines;
mod velvet;

use firmware::*;
use routing::{DualEfxProcessor, RoutingMode};
use trampolines as original;

/// The DualEfxProcessor instance.
/// Placed in high SDRAM (0x83e00000) by link.ld (.bss section).
static mut PROCESSOR: core::mem::MaybeUninit<DualEfxProcessor> = core::mem::MaybeUninit::uninit();
static mut MAGIC_TAG: u32 = 0;

/// Current parameter editing bank:
/// 0 = Velvet Noise Decorrelator (Effect A)
/// 1 = Plateau Plate Reverb (Effect B)
/// 2 = Auto (follow active efx_type: Room -> Plateau, Spring -> Velvet Noise)
static mut EDIT_BANK: u8 = 2;

/// Selected channel for per-channel routing setup (0..7)
static mut SELECTED_ROUTING_CHANNEL: u8 = 0;

/// Last observed parameters to prevent redundant recalculation
static mut LAST_PARAM0: u32 = 0xffff;
static mut LAST_PARAM1: u32 = 0xffff;
static mut LAST_EFX_TYPE: u32 = 0xffff;

const INIT_MAGIC: u32 = 0x44454658; // "DEFX"

unsafe fn get_processor() -> &'static mut DualEfxProcessor {
    unsafe {
        let storage = core::ptr::addr_of_mut!(PROCESSOR);
        if read(core::ptr::addr_of!(MAGIC_TAG)) != INIT_MAGIC {
            // Preserve the caller's interrupt mask; do not unmask a nested
            // critical section when initialization returns.
            let interrupt_mask: u32;
            core::arch::asm!("mrs {}, PRIMASK", "cpsid i", out(reg) interrupt_mask, options(nostack));
            // A second caller could have initialized before masking interrupts.
            if read(core::ptr::addr_of!(MAGIC_TAG)) != INIT_MAGIC {
                DualEfxProcessor::initialize_in_place(&mut *storage);
                write(core::ptr::addr_of_mut!(EDIT_BANK), 2);
                write(core::ptr::addr_of_mut!(SELECTED_ROUTING_CHANNEL), 0);
                write(core::ptr::addr_of_mut!(LAST_PARAM0), 0xffff);
                write(core::ptr::addr_of_mut!(LAST_PARAM1), 0xffff);
                write(core::ptr::addr_of_mut!(LAST_EFX_TYPE), 0xffff);
                write(core::ptr::addr_of_mut!(MAGIC_TAG), INIT_MAGIC);
            }
            core::arch::asm!("msr PRIMASK, {}", in(reg) interrupt_mask, options(nostack));
        }
        // The marker is not yet invalidated on every reset; this legacy hook
        // remains disabled until lifetime and exclusivity are established.
        (&mut *storage).assume_init_mut()
    }
}

/// Dispatches effect audio processing.
/// Hooked at 0x8000277c in MAIN firmware (displaces bl 0x80003c58).
///
/// Registers:
/// - r0: instance/mixer id (0)
/// - r1: efx_type (1=Room, 2=Spring, 3=Delay, 4=Chorus, 5=AINR)
/// - r2: param0 (0..127)
/// - r3: param1 (0..127)
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.efx_dispatch_hook")]
pub unsafe extern "C" fn efx_dispatch_hook(
    inst: u32,
    efx_type: u32,
    param0: u32,
    param1: u32,
) {
    unsafe {
        let proc = get_processor();

        // Stock effect type mapping:
        // efx_type == 1: Room Reverb -> Replaced with Plateau
        // efx_type == 2: Spring Reverb -> Replaced with Velvet Noise Decorrelator
        // If efx_type is Delay (3), Chorus (4), or AINR (5), forward to stock DSP!
        if efx_type == 1 || efx_type == 2 {
            let last_p0 = read(core::ptr::addr_of!(LAST_PARAM0));
            let last_p1 = read(core::ptr::addr_of!(LAST_PARAM1));
            let last_type = read(core::ptr::addr_of!(LAST_EFX_TYPE));

            // Only update parameters if param0/param1/efx_type changed:
            // This prevents recalculating filter/delay coefficients on every audio block (3000 Hz)
            // and preserves extended parameter edits made via encoders.
            if param0 != last_p0 || param1 != last_p1 || efx_type != last_type {
                write(core::ptr::addr_of_mut!(LAST_PARAM0), param0);
                write(core::ptr::addr_of_mut!(LAST_PARAM1), param1);
                write(core::ptr::addr_of_mut!(LAST_EFX_TYPE), efx_type);

                if efx_type == 1 {
                    // Room Reverb: param0 = PreDelay, param1 = Decay
                    let mut params = proc.plateau.params;
                    params.predelay_ms = (param0 as f32 / 127.0) * 150.0;
                    params.decay = 0.1 + (param1 as f32 / 127.0) * 0.899;
                    proc.plateau.set_parameters(params);
                } else {
                    // Spring Reverb: param0 = VND Time Spread, param1 = VND Decay
                    let mut params = proc.velvet.current;
                    params.length_ms = 1.0 + (param0 as f32 / 127.0) * 99.0;
                    params.decay_db = -120.0 + (param1 as f32 / 127.0) * 120.0;
                    proc.velvet.set_parameters(params);
                }
            }

            // Read send levels from mixer channels (0..7 for L6max)
            let mut any_send = false;
            for ch in 0..8 {
                let send_val = call2(MIXER_GET_EFX, ch as u32, 0);
                if send_val > 0 {
                    any_send = true;
                    // If routing hasn't been explicitly configured, default according to efx_type:
                    // efx_type == 1 -> SendB (Plateau)
                    // efx_type == 2 -> SendA (Velvet Noise)
                    if proc.channel_routing[ch] == RoutingMode::Neither {
                        proc.channel_routing[ch] = if efx_type == 1 {
                            RoutingMode::SendB
                        } else {
                            RoutingMode::SendA
                        };
                    }
                }
            }

            if !any_send {
                return;
            }

            // Audio buffers: 16 stereo frames (typical 32-sample block)
            // Stock buffer pointers:
            // Input send buffer: AUDIO_EFX_IN_BUFFER (0x804683a4)
            // Output return buffer: AUDIO_EFX_OUT_BUFFER (0x8045fe98)
            let in_buf = AUDIO_EFX_IN_BUFFER as *const f32;
            let out_buf = AUDIO_EFX_OUT_BUFFER as *mut f32;

            // Block size is 16 frames (32 samples interleaved)
            const FRAMES: usize = 16;
            let mut in_l = [0.0f32; FRAMES];
            let mut in_r = [0.0f32; FRAMES];
            let mut out_l = [0.0f32; FRAMES];
            let mut out_r = [0.0f32; FRAMES];

            for i in 0..FRAMES {
                in_l[i] = read(in_buf.add(i * 2));
                in_r[i] = read(in_buf.add(i * 2 + 1));
            }

            // If efx_type == 1, enable Plateau (Effect B). If 2, enable Velvet Noise (Effect A).
            // When dual mode or cascade is configured, both run concurrently!
            let enable_a = efx_type == 2 || proc.cascade_gain > 1e-4;
            let enable_b = efx_type == 1 || proc.cascade_gain > 1e-4;
            proc.process_stereo_send(&in_l, &in_r, &mut out_l, &mut out_r, enable_a, enable_b);

            for i in 0..FRAMES {
                write(out_buf.add(i * 2), out_l[i]);
                write(out_buf.add(i * 2 + 1), out_r[i]);
            }
        } else {
            // Forward stock Delay, Chorus, or AINR to original implementation
            original::original_efx_dispatch(inst, efx_type, param0, param1);
        }
    }
}

/// Replaces the stock knob changed hook to provide extended parameter editing
/// when in an effect edit context, while preserving standard channel send levels
/// when in the normal mixer screen.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.knob_changed")]
pub unsafe extern "C" fn knob_changed(
    channel: u32,
    mode: u32,
    absolute: u32,
    delta: i32,
    value: u32,
) {
    unsafe {
        // Normal recorder window descriptor is 0x80200408.
        let active_window = read(0x80202610 as *const u32);
        let in_recorder_window = active_window == 0x80200408;

        // When in standard mixer/recorder window, encoders 0..7 control
        // Channel 1..8 Send Levels (for Mode 6) and other channel parameters normally.
        if in_recorder_window {
            core::mem::transmute::<usize, unsafe extern "C" fn(u32, u32, u32, i32, u32)>(
                MIXER_APPLY_CHANNEL_PARAMETER | 1,
            )(channel, mode, absolute, delta, value);
            return;
        }

        // When in an extended screen context / dialog:
        if channel >= 8 || mode >= 10 || delta == 0 {
            return;
        }

        // Mode 6 is EFX
        if mode == 6 {
            let proc = get_processor();
            let efx_type = call1(MIXER_GET_EFX_TYPE, 0);
            let configured_bank = read(core::ptr::addr_of!(EDIT_BANK));
            let bank = match configured_bank {
                0 => 0,
                1 => 1,
                _ => if efx_type == 1 { 1 } else { 0 },
            };

            // Keep the caller's interrupt-mask state across this update.
            let interrupt_mask: u32;
            core::arch::asm!("mrs {}, PRIMASK", "cpsid i", out(reg) interrupt_mask, options(nostack));

            // Map the 8 encoders (channels 0..7) to extended parameters:
            match channel {
                0 => {
                    // Encoder 1: Effect A Amount or Effect B PreDelay
                    if bank == 0 {
                        let mut params = proc.velvet.current;
                        params.amount = math::clamp(
                            proc.velvet.current.amount + (delta as f32 * 0.02),
                            0.0,
                            1.0,
                        );
                        proc.velvet.set_parameters(params);
                    } else {
                        let mut params = proc.plateau.params;
                        params.predelay_ms = math::clamp(
                            params.predelay_ms + (delta as f32 * 2.0),
                            0.0,
                            plateau::MAX_PREDELAY_MS,
                        );
                        proc.plateau.set_parameters(params);
                    }
                }
                1 => {
                    // Encoder 2: Effect A Time Spread or Effect B Size
                    if bank == 0 {
                        let mut params = proc.velvet.current;
                        params.length_ms = math::clamp(
                            proc.velvet.current.length_ms + (delta as f32 * 1.0),
                            1.0,
                            100.0,
                        );
                        proc.velvet.set_parameters(params);
                    } else {
                        let mut params = proc.plateau.params;
                        params.size = math::clamp(
                            params.size + (delta as f32 * 0.02),
                            0.01,
                            plateau::MAX_SIZE,
                        );
                        proc.plateau.set_parameters(params);
                    }
                }
                2 => {
                    // Encoder 3: Effect A Density or Effect B Decay
                    if bank == 0 {
                        let mut params = proc.velvet.current;
                        params.density = math::clamp(
                            proc.velvet.current.density + (delta as f32 * 50.0),
                            100.0,
                            4000.0,
                        );
                        proc.velvet.set_parameters(params);
                    } else {
                        let mut params = proc.plateau.params;
                        params.decay = math::clamp(
                            params.decay + (delta as f32 * 0.02),
                            0.1,
                            0.999,
                        );
                        proc.plateau.set_parameters(params);
                    }
                }
                3 => {
                    // Encoder 4: Effect A Decay or Effect B Low-Cut
                    if bank == 0 {
                        let mut params = proc.velvet.current;
                        params.decay_db = math::clamp(
                            proc.velvet.current.decay_db + (delta as f32 * 2.0),
                            -120.0,
                            0.0,
                        );
                        proc.velvet.set_parameters(params);
                    } else {
                        let mut params = proc.plateau.params;
                        params.low_cut_hz = math::clamp(
                            params.low_cut_hz + (delta as f32 * 20.0),
                            20.0,
                            1000.0,
                        );
                        proc.plateau.set_parameters(params);
                    }
                }
                4 => {
                    // Encoder 5: Effect A Refresh or Effect B High-Cut
                    if bank == 0 {
                        let mut params = proc.velvet.current;
                        params.refresh_seconds = math::clamp(
                            proc.velvet.current.refresh_seconds + (delta as f32 * 0.5),
                            0.1,
                            60.0,
                        );
                        proc.velvet.set_parameters(params);
                    } else {
                        let mut params = proc.plateau.params;
                        params.high_cut_hz = math::clamp(
                            params.high_cut_hz + (delta as f32 * 250.0),
                            1000.0,
                            16000.0,
                        );
                        proc.plateau.set_parameters(params);
                    }
                }
                5 => {
                    // Encoder 6: Effect A Dynamic Crossfade or Effect B Diffusion
                    if bank == 0 {
                        if delta > 0 {
                            proc.velvet.current.dynamic = true;
                        } else if delta < 0 {
                            proc.velvet.current.dynamic = false;
                        }
                    } else {
                        let mut params = proc.plateau.params;
                        params.diffusion = math::clamp(
                            params.diffusion + (delta as f32 * 0.05),
                            0.0,
                            1.0,
                        );
                        proc.plateau.set_parameters(params);
                    }
                }
                6 => {
                    // Encoder 7: Velvet Noise Routing cycle or Plateau Modulation Depth
                    if bank == 0 {
                        let mut sel = read(core::ptr::addr_of!(SELECTED_ROUTING_CHANNEL)) as usize;
                        if sel >= 8 {
                            sel = 0;
                        }
                        let current = proc.channel_routing[sel];
                        let next = match current {
                            RoutingMode::Neither => RoutingMode::SendA,
                            RoutingMode::SendA => RoutingMode::SendB,
                            RoutingMode::SendB => RoutingMode::SendAtoB,
                            RoutingMode::SendAtoB => {
                                // Once completed full cycle for this channel, advance to next channel
                                let next_ch = (sel + 1) % 8;
                                write(core::ptr::addr_of_mut!(SELECTED_ROUTING_CHANNEL), next_ch as u8);
                                RoutingMode::Neither
                            }
                        };
                        proc.channel_routing[sel] = next;
                    } else {
                        // Plateau Modulation Depth (0.0 to 10.0 samples)
                        let mut params = proc.plateau.params;
                        params.mod_depth = math::clamp(
                            params.mod_depth + (delta as f32 * 0.25),
                            0.0,
                            10.0,
                        );
                        proc.plateau.set_parameters(params);
                    }
                }
                7 => {
                    // Encoder 8: Global Cascade toggle (A -> B) or Freeze toggle
                    if bank == 0 {
                        proc.cascade_gain = if proc.cascade_gain > 0.5 { 0.0 } else { 1.0 };
                    } else {
                        let mut params = proc.plateau.params;
                        params.freeze = !params.freeze;
                        proc.plateau.set_parameters(params);
                    }
                }
                _ => {}
            }
            core::arch::asm!("msr PRIMASK, {}", in(reg) interrupt_mask, options(nostack));
        } else {
            // For other modes on dialog screens, apply normally
            core::mem::transmute::<usize, unsafe extern "C" fn(u32, u32, u32, i32, u32)>(
                MIXER_APPLY_CHANNEL_PARAMETER | 1,
            )(channel, mode, absolute, delta, value);
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
