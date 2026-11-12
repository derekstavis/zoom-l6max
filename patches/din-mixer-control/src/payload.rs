#![cfg_attr(not(sd_composed), no_std)]
#![cfg_attr(not(sd_composed), no_main)]
//! Lets a MIDI device on the physical MIDI IN/OUT (DIN/TRS) jacks control the
//! mixer when Mixer Control via MIDI is on, as the manual describes but the
//! stock firmware never implements for that jack.
//!
//! Hooked mid-function, right after the DIN UART receive parser
//! (`MidiUartRcvMsg`) finishes assembling a complete message and before its
//! one existing consumer there (a factory-jig check, gated on a session
//! state that is never true in normal use). The builder-generated offset
//! veneer preserves every register across the call into this logic, then
//! replays the displaced instruction and resumes the original function.
#[cfg(not(sd_composed))]
#[path = "../../api/patchrt.rs"]
#[macro_use]
pub mod patch_api;
#[cfg(sd_composed)]
use crate::patch_api;
use patch_api::function_offset_hook;
#[cfg(not(sd_composed))]
mod firmware;
#[cfg(not(sd_composed))]
use firmware as firmware_bindings;
#[cfg(sd_composed)]
use crate::firmware_bindings;
#[path = "../../api/l6max/hooks.rs"]
#[cfg(not(sd_composed))]
mod hook_catalog;
#[cfg(not(sd_composed))]
use firmware::*;
#[cfg(sd_composed)]
use crate::firmware_bindings::*;

#[function_offset_hook(
    midi_uart_rcv_msg_task,
    at = 0x21e,
    displaced_len = 4,
    guard_sha256 = "c2b6e5432110b5d81a061afa4b39ab08a7c9240f645c0478ecf64fc058abba27"
)]
pub unsafe fn din_mixer_forward() {
    unsafe {
        let length = midi_message_length.read();
        if length == 3 && mixer_control_via_midi() != 0 {
            apply_mixer_cc(midi_message_buffer.address() as *const u8);
        }
    }
    hook_continue()
}
