//! Revision-specific firmware bindings used by the MIDI state sync patch.
//! Function entry addresses are even; call helpers select Thumb mode.
use core::mem::transmute;

/// Stock mixer getters, shared with the knob value patch.
pub const MIXER_GET_AUX: usize = 0x8000b080;
pub const MIXER_GET_LEVEL: usize = 0x8000b0b0;
pub const MIXER_GET_EFX: usize = 0x8000b0f0;
pub const MIXER_GET_EQ_FREQUENCY: usize = 0x8000b120;
pub const MIXER_GET_EQ_GAIN: usize = 0x8000b140;
pub const MIXER_GET_PAN: usize = 0x8000b1d0;
pub const MIXER_GET_SUBMIX: usize = 0x8000b2a0;
/// Maps a physical channel (0-7) to the mixer channel the getters expect.
pub const MIXER_CHANNEL_INDEX: usize = 0x800564b0;
/// Configured MIDI channel (0-15) from the settings callback table.
pub const MIDI_CHANNEL: usize = 0x8003feb0;
/// Stock Control Change sender: (parameter id, 7-bit value). Applies the user's
/// CC map and the configured MIDI channel, and skips unassigned parameters.
pub const MIDI_SEND_CC: usize = 0x8003f9b0;

pub unsafe fn call0(address: usize) -> u32 {
    unsafe { transmute::<usize, unsafe extern "C" fn() -> u32>(address | 1)() }
}
pub unsafe fn call1(address: usize, a: u32) -> u32 {
    unsafe { transmute::<usize, unsafe extern "C" fn(u32) -> u32>(address | 1)(a) }
}
pub unsafe fn call2(address: usize, a: u32, b: u32) -> u32 {
    unsafe { transmute::<usize, unsafe extern "C" fn(u32, u32) -> u32>(address | 1)(a, b) }
}
