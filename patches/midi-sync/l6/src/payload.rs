#![no_std]
#![no_main]
//! Broadcast L6 mixer state through its own configured MIDI map when requested.
#[path = "../../../api/patchrt.rs"]
#[macro_use]
pub mod patch_api;
use patch_api::function_entry_hook;
mod firmware;
use firmware as firmware_bindings;
#[path = "../../../api/l6/hooks.rs"]
mod hook_catalog;
#[path = "../../common/sync_request.rs"]
mod sync_request;
use firmware::*;

const CHANNELS: u32 = 6;
const GROUPS: u32 = 9;

/// Reads the current position for an L6 mode. `audio` is the L6's mapped
/// channel index; `group` follows the nine six-channel CC map groups.
unsafe fn stored_value(audio: u32, group: u32) -> u32 {
    unsafe {
        match group {
            0 => mixer_get_eq_gain(audio, 0),
            1 => mixer_get_eq_frequency(audio, 1),
            2 => mixer_get_eq_gain(audio, 1),
            3 => mixer_get_eq_gain(audio, 2),
            4 => mixer_get_aux(audio, 0),
            5 => mixer_get_aux(audio, 1),
            6 => mixer_get_efx(audio, 0),
            7 => mixer_get_pan(audio),
            8 => mixer_get_level(audio),
            _ => 0x100,
        }
    }
}

unsafe fn broadcast_state() {
    unsafe {
        for channel in 0..CHANNELS {
            let audio = mixer_channel_index(channel);
            if audio >= 10 {
                continue;
            }
            for group in 0..GROUPS {
                let value = stored_value(audio, group);
                if value <= 127 {
                    midi_send_mapped_cc(channel, group, value);
                }
            }
        }
    }
}

/// The stock handler's message bytes are at offsets +4, +5 and +6.
#[function_entry_hook(midi_in)]
pub unsafe fn midi_in_hook(message: *const u8) {
    if unsafe { sync_request::is_sync_message(message, midi_channel() as u8) } {
        unsafe { broadcast_state() };
        hook_done()
    } else {
        hook_continue(message)
    }
}
