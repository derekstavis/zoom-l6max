#![no_std]
#![no_main]
//! Broadcast L6 mixer state through its own configured MIDI map when requested.
use core::ptr::read_volatile as read;

mod firmware;
mod trampolines;
use firmware::*;
use trampolines as original;

const CHANNELS: u32 = 6;
const GROUPS: u32 = 9;
const SYNC_CC: u8 = 127;
const SYNC_MIN_VALUE: u8 = 64;

/// Reads the current position for an L6 mode. `audio` is the L6's mapped
/// channel index; `group` follows the nine six-channel CC map groups.
unsafe fn stored_value(audio: u32, group: u32) -> u32 {
    unsafe {
        match group {
            0 => call2(GET_EQ_GAIN, audio, 0),
            1 => call2(GET_EQ_FREQUENCY, audio, 1),
            2 => call2(GET_EQ_GAIN, audio, 1),
            3 => call2(GET_EQ_GAIN, audio, 2),
            4 => call2(GET_AUX, audio, 0),
            5 => call2(GET_AUX, audio, 1),
            6 => call2(GET_EFX, audio, 0),
            7 => call1(GET_PAN, audio),
            8 => call1(GET_LEVEL, audio),
            _ => 0x100,
        }
    }
}

unsafe fn broadcast_state() {
    unsafe {
        for channel in 0..CHANNELS {
            let audio = call1(CHANNEL_INDEX, channel);
            if audio >= 10 {
                continue;
            }
            for group in 0..GROUPS {
                let value = stored_value(audio, group);
                if value <= 127 {
                    call3(MIDI_SEND_MAPPED_CC, channel, group, value);
                }
            }
        }
    }
}

/// The stock handler's message bytes are at offsets +4, +5 and +6.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.midi_in_hook")]
pub unsafe extern "C" fn midi_in_hook(message: *const u8) {
    unsafe {
        let status = read(message.add(4));
        let controller = read(message.add(5));
        let value = read(message.add(6));
        let channel = call0(MIDI_CHANNEL) as u8;
        if status & 0xf0 == 0xb0
            && status & 0x0f == channel
            && controller == SYNC_CC
            && value >= SYNC_MIN_VALUE
        {
            broadcast_state();
            return;
        }
        core::mem::transmute::<usize, unsafe extern "C" fn(*const u8)>(original::midi_in() | 1)(
            message,
        );
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
