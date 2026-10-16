//! Revision-specific firmware bindings for the DIN mixer control patch.
//! Function entry addresses are even; call helpers select Thumb mode.
use core::mem::transmute;

/// `_DAT_805e5d4a`: the just-completed message's total length, as the DIN
/// UART receive parser (`MidiUartRcvMsg`) itself tracks it.
pub const MESSAGE_LENGTH: usize = 0x805e5d4a;
/// `&_DAT_80898938`: the DIN UART receive parser's own message scratch
/// buffer, 4 bytes before its first content byte (status at `MESSAGE_BASE+4`,
/// data at `+5` and `+6`) — the same pointer convention already built for the
/// one other consumer of a complete message at this point (the factory-jig
/// Note On check), and the same layout `APPLY_MIXER_CC` expects.
pub const MESSAGE_BASE: usize = 0x80898938;
/// `mixerControlViaMidi` getter, shared with the DIN OUT echo gate in the
/// panel mute handler.
pub const MIXER_CONTROL_VIA_MIDI: usize = 0x8003ff10;
/// Entry of the USB Mixer Control port's inbound message handler: applies a
/// 3-byte Control Change (status at offset 4, data at 5 and 6 of the pointed
/// buffer) to the mixer using the user's CC map and configured MIDI channel.
/// This is also `midi-sync`'s own hook point; combining both patches is
/// unaffected, since that patch always falls through to this same code for
/// anything but its own CC 127 broadcast request.
pub const APPLY_MIXER_CC: usize = 0x8003f7c8;

pub unsafe fn call0(address: usize) -> u32 {
    unsafe { transmute::<usize, unsafe extern "C" fn() -> u32>(address | 1)() }
}
pub unsafe fn call1(address: usize, a: u32) -> u32 {
    unsafe { transmute::<usize, unsafe extern "C" fn(u32) -> u32>(address | 1)(a) }
}
