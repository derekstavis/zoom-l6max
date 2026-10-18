//! Revision-specific firmware bindings used by the MIDI state sync patch.
//! Function entry addresses are even; call helpers select Thumb mode.
use core::mem::transmute;

/// Stock mixer getters, shared with the knob value patch.
pub const MIXER_GET_AUX: usize = 0x8000b080;
pub const MIXER_GET_LEVEL: usize = 0x8000b0b0;
/// `fn(mapped_channel: u32) -> u8`, one byte at `mapped_channel*0x18 +
/// 0x8045ff84` — the same 24-byte-stride per-channel record `MIXER_GET_CHANNEL_FLAG`
/// indexes into, just a different field (offset 0 there, offset 0x15 here).
/// Paired with the setter at `MIXER_SET_MUTE`. Found by decompiling the MUTE
/// button dispatcher (`0x80055050`), which reads this, toggles, calls
/// `0x8000cd30` (identifier `channel | 0x50`, confirming this is MUTE) to
/// notify, then writes the new value back with `MIXER_SET_MUTE`.
pub const MIXER_GET_MUTE: usize = 0x8000b0c0;
pub const MIXER_GET_EFX: usize = 0x8000b0f0;
pub const MIXER_GET_EQ_FREQUENCY: usize = 0x8000b120;
pub const MIXER_GET_EQ_GAIN: usize = 0x8000b140;
pub const MIXER_GET_PAN: usize = 0x8000b1d0;
pub const MIXER_GET_SUBMIX: usize = 0x8000b2a0;
/// `fn(mapped_channel: u32, value: u8)`, writes `MIXER_GET_MUTE`'s own field.
/// Not called by this patch (sync only reads state), kept for documentation
/// parity with the getter and in case a future patch needs it.
#[allow(dead_code)]
pub const MIXER_SET_MUTE: usize = 0x8000bb90;
/// Generic per-slot toggle-flag getter: `fn(slot: u32) -> u8`, one byte at
/// `slot*0x18 + 0x8045ff99`. Backs four button-type globals (not per physical
/// channel) at fixed slots: CH 5 MONO x2 (slot 4), CH 6 MONO x2 (slot 6),
/// USB 1/2 (slot 8), USB 3/4 (slot 10) — see `GLOBAL_TOGGLES`. The same table
/// extends to slots 0-3 and odd slots 5/7/9/11; MUTE is a separate field in
/// the same record (`MIXER_GET_MUTE`), not one of these — what the remaining
/// unattributed slots mean remains unestablished.
pub const MIXER_GET_CHANNEL_FLAG: usize = 0x8000b190;
/// `fn() -> u8`, the compressor on/off flag (`DAT_804600b0`, a single global
/// byte — not per channel, no argument). Identifier `0x5D`.
pub const MIXER_GET_COMPRESSOR: usize = 0x8000b1c0;
/// `fn(bus: u32) -> u8`, the current EFX type index (0-4), one byte at
/// `bus*0x2e + 0x804600b2`. Always called with `bus = 0` by the stock
/// dispatcher (this device has one EFX bus). Identifier `0x5C`.
pub const MIXER_GET_EFX_TYPE: usize = 0x8000b270;
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
