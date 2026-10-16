#![no_std]
#![no_main]
//! Lets a MIDI device on the physical MIDI IN/OUT (DIN/TRS) jacks control the
//! mixer when Mixer Control via MIDI is on, as the manual describes but the
//! stock firmware never implements for that jack.
//!
//! Hooked mid-function, right after the DIN UART receive parser
//! (`MidiUartRcvMsg`) finishes assembling a complete message and before its
//! one existing consumer there (a factory-jig check, gated on a session
//! state that is never true in normal use). `trampolines.s` preserves every
//! register across the call into this logic, then replays the displaced
//! instruction and resumes the original function exactly where it left off.
mod firmware;
mod trampolines;
use core::ptr::read_volatile;
use firmware::*;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn din_mixer_forward() {
    unsafe {
        let length = read_volatile(MESSAGE_LENGTH as *const u16);
        if length == 3 && call0(MIXER_CONTROL_VIA_MIDI) != 0 {
            call1(APPLY_MIXER_CC, MESSAGE_BASE as u32);
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
