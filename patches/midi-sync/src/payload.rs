#![no_std]
#![no_main]
//! Broadcasts the whole channel mixer state as Control Change messages when a
//! controller asks for it. Live changes keep using the stock notifier.
use core::ptr::read_volatile as read;

mod firmware;
mod parameters;
mod trampolines;
use firmware::*;
use parameters::*;
use trampolines as original;

/// Current stored value (0-127) of `source` for mixer channel `audio`.
unsafe fn stored_value(source: Source, audio: u32) -> u32 {
    unsafe {
        match source {
            Source::EqGain(band) => call2(MIXER_GET_EQ_GAIN, audio, band),
            Source::EqFrequency(band) => call2(MIXER_GET_EQ_FREQUENCY, audio, band),
            Source::Aux(bus) => call2(MIXER_GET_AUX, audio, bus),
            Source::Efx => call2(MIXER_GET_EFX, audio, 0),
            Source::SubMix => call1(MIXER_GET_SUBMIX, audio),
            Source::Pan => call1(MIXER_GET_PAN, audio),
            Source::Level => call1(MIXER_GET_LEVEL, audio),
        }
    }
}

/// Sends every setting of every channel, then the global toggles, through the
/// stock sender, which applies the user's CC map, MIDI channel and output gating.
unsafe fn broadcast_state() {
    unsafe {
        for channel in 0..CHANNELS {
            let audio = call1(MIXER_CHANNEL_INDEX, channel as u32);
            if audio > MAX_MIXER_CHANNEL {
                continue;
            }
            for mode in 0..MODES {
                let raw = stored_value(SOURCES[mode], audio);
                if raw > 127 {
                    continue;
                }
                if let Some(id) = parameter_id(mode, channel) {
                    call2(MIDI_SEND_CC, id as u32, raw);
                }
            }
        }
        for toggle in GLOBAL_TOGGLES {
            let raw = call1(MIXER_GET_CHANNEL_FLAG, toggle.slot);
            if raw > 127 {
                continue;
            }
            call2(MIDI_SEND_CC, toggle.id as u32, raw);
        }
    }
}

/// Replaces the entry of the stock inbound MIDI message handler. `message`
/// holds the status byte at offset 4 and the data bytes at offsets 5 and 6.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.midi_in_hook")]
pub unsafe extern "C" fn midi_in_hook(message: *const u8) {
    unsafe {
        let status = read(message.add(4));
        let controller = read(message.add(5));
        let value = read(message.add(6));
        if status & 0xf0 == 0xb0
            && is_sync_request(status, controller, value, call0(MIDI_CHANNEL) as u8)
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
