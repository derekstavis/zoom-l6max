#![no_std]
#![no_main]
//! Broadcasts the whole channel mixer state as Control Change messages when a
//! controller asks for it. Live changes keep using the stock notifier.
#[path = "../../../api/patchrt.rs"]
#[macro_use]
pub mod patch_api;
use patch_api::function_entry_hook;
mod firmware;
use firmware as firmware_bindings;
#[path = "../../../api/l6max/hooks.rs"]
mod hook_catalog;
#[path = "../../common/sync_request.rs"]
mod sync_request;
mod parameters;
use firmware::*;
use parameters::*;

/// Current stored value (0-127) of `source` for mixer channel `audio`.
unsafe fn stored_value(source: Source, audio: u32) -> u32 {
    unsafe {
        match source {
            Source::EqGain(band) => mixer_get_eq_gain(audio, band),
            Source::EqFrequency(band) => mixer_get_eq_frequency(audio, band),
            Source::Aux(bus) => mixer_get_aux(audio, bus),
            Source::Efx => mixer_get_efx(audio, 0),
            Source::SubMix => mixer_get_submix(audio),
            Source::Pan => mixer_get_pan(audio),
            Source::Level => mixer_get_level(audio),
            Source::Mute => mixer_get_mute(audio),
        }
    }
}

/// Sends every setting of every channel (now including MUTE), the four
/// slot-based global toggles, and EFX TYPE/COMPRESSOR (each its own getter,
/// not slot-based), through the stock sender, which applies the user's CC
/// map, MIDI channel and output gating. Covers every one of the stock
/// firmware's 94 parameter identifiers.
unsafe fn broadcast_state() {
    unsafe {
        for channel in 0..CHANNELS {
            let audio = mixer_channel_index(channel as u32);
            if audio > MAX_MIXER_CHANNEL {
                continue;
            }
            for mode in 0..MODES {
                let raw = stored_value(SOURCES[mode], audio);
                if raw > 127 {
                    continue;
                }
                if let Some(id) = parameter_id(mode, channel) {
                    midi_send_cc(id as u32, raw);
                }
            }
        }
        for toggle in GLOBAL_TOGGLES {
            let raw = mixer_get_channel_flag(toggle.slot);
            if raw > 127 {
                continue;
            }
            midi_send_cc(toggle.id as u32, raw);
        }
        let efx_type = mixer_get_efx_type(0);
        if efx_type <= 127 {
            midi_send_cc(EFX_TYPE_ID as u32, efx_type);
        }
        let compressor = mixer_get_compressor();
        if compressor <= 127 {
            midi_send_cc(COMPRESSOR_ID as u32, compressor);
        }
    }
}

#[function_entry_hook(midi_in)]
pub unsafe fn midi_in_hook(message: *const u8) {
    if unsafe { sync_request::is_sync_message(message, midi_channel() as u8) } {
        unsafe { broadcast_state() };
        hook_done()
    } else {
        hook_continue(message)
    }
}
