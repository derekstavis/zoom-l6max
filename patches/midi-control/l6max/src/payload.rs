#![no_std]
#![no_main]
//! Recorder transport control and state feedback over the configured MIDI port.
#[path = "../../../api/patchrt.rs"]
#[macro_use]
pub mod patch_api;
use patch_api::function_entry_hook;
mod firmware;
use firmware as firmware_bindings;
#[path = "../../../api/l6max/hooks.rs"]
mod hook_catalog;
#[path = "../../../midi-sync/common/sync_request.rs"]
mod sync_request;
#[path = "../../../midi-sync/l6max/src/parameters.rs"]
mod parameters;
use firmware::*;
use parameters::*;

const CC_REC: u8 = 107;
const CC_PLAY_STOP: u8 = 108;
const CC_PREVIOUS: u8 = 111;
const CC_PAGE: u8 = 112;
const CC_STATE_REQUEST: u8 = 127;
const CC_RECORDER_STATE: u32 = 115;
const CC_BLUE_PAGE: u32 = 116;
const EVENT_REC: u32 = 0x2b;
const EVENT_PLAY_STOP: u32 = 0x2c;
const EVENT_PREVIOUS: u32 = 0x2f;
const EVENT_PAGE_FIRST: u32 = 0x15;
const PAGE_COUNT: u8 = 10;

unsafe fn post_button(event: u32, kind: u32) {
    unsafe {
        let _ = panel_ui_event_queue_send(0, kind, event, 0, 0);
    }
}

unsafe fn tap_button(event: u32) {
    unsafe {
        post_button(event, 0);
        post_button(event, 1);
    }
}

unsafe fn report_state() {
    let state = unsafe { rec_mode.read() };
    unsafe { midi_send_cc_raw(CC_RECORDER_STATE, u32::from(state)) };
}

unsafe fn stored_value(source: Source, channel: u32) -> u32 {
    unsafe {
        match source {
            Source::EqGain(band) => mixer_get_eq_gain(channel, band),
            Source::EqFrequency(band) => mixer_get_eq_frequency(channel, band),
            Source::Aux(bus) => mixer_get_aux(channel, bus),
            Source::Efx => mixer_get_efx(channel, 0),
            Source::SubMix => mixer_get_submix(channel),
            Source::Pan => mixer_get_pan(channel),
            Source::Level => mixer_get_level(channel),
            Source::Mute => mixer_get_mute(channel),
        }
    }
}

unsafe fn broadcast_state() {
    unsafe {
        for physical in 0..CHANNELS {
            let audio = mixer_channel_index(physical as u32);
            if audio > MAX_MIXER_CHANNEL {
                continue;
            }
            for mode in 0..MODES {
                let raw = stored_value(SOURCES[mode], audio);
                if raw <= 127 {
                    if let Some(id) = parameter_id(mode, physical) {
                        midi_send_cc(u32::from(id), raw);
                    }
                }
            }
        }
        for toggle in GLOBAL_TOGGLES {
            let raw = mixer_get_channel_flag(toggle.slot);
            if raw <= 127 {
                midi_send_cc(u32::from(toggle.id), raw);
            }
        }
        let efx_type = mixer_get_efx_type(0);
        if efx_type <= 127 {
            midi_send_cc(u32::from(EFX_TYPE_ID), efx_type);
        }
        let compressor = mixer_get_compressor();
        if compressor <= 127 {
            midi_send_cc(u32::from(COMPRESSOR_ID), compressor);
        }
        report_state();
    }
}

#[function_entry_hook(midi_in)]
pub unsafe fn midi_control_in(message: *const u8) {
    if message.is_null() {
        hook_continue(message)
    } else {
        let status = unsafe { core::ptr::read_volatile(message.add(4)) };
        let controller = unsafe { core::ptr::read_volatile(message.add(5)) };
        let value = unsafe { core::ptr::read_volatile(message.add(6)) };
        let channel = (status & 0x0f) as u32;
        let configured = unsafe { midi_channel() };

        if controller == CC_STATE_REQUEST
            && sync_request::is_sync_request(status, controller, value, configured as u8)
        {
            unsafe { broadcast_state() };
            hook_done()
        } else if status & 0xf0 == 0xb0
            && channel == configured
            && controller == CC_STATE_REQUEST
        {
            hook_done()
        } else if status & 0xf0 == 0xb0
            && channel == configured
            && matches!(controller, CC_REC | CC_PLAY_STOP | CC_PREVIOUS)
        {
            if value >= 64 {
                let event = match controller {
                    CC_REC => EVENT_REC,
                    CC_PLAY_STOP => EVENT_PLAY_STOP,
                    _ => EVENT_PREVIOUS,
                };
                unsafe { tap_button(event) };
            }
            hook_done()
        } else if status & 0xf0 == 0xb0 && channel == configured && controller == CC_PAGE {
            if value < PAGE_COUNT {
                unsafe { tap_button(EVENT_PAGE_FIRST + u32::from(value)) };
            }
            hook_done()
        } else {
            hook_continue(message)
        }
    }
}

macro_rules! report_after_stock {
    ($handler:ident, $site:ident) => {
        #[function_entry_hook($site)]
        pub unsafe fn $handler(context: *mut u32) -> u32 {
            let result = unsafe { crate::call_original!($site, context) };
            unsafe { report_state() };
            hook_done(result)
        }
    };
}

report_after_stock!(report_play_start, rec_play_start_callback);
report_after_stock!(report_play_stop, rec_play_stop_callback);
report_after_stock!(report_record_start, rec_start_callback);
report_after_stock!(report_record_stop, rec_stop_callback);

#[function_entry_hook(panel_handle_main_button_event)]
pub unsafe fn report_page_button(event: *const u32, context: *mut u32) {
    if !event.is_null() {
        let source = unsafe { core::ptr::read_volatile(event) };
        let kind = unsafe { core::ptr::read_volatile(event.add(1)) };
        let button = unsafe { core::ptr::read_volatile(event.add(2)) };
        if source == 0 && kind == 0 && (EVENT_PAGE_FIRST..EVENT_PAGE_FIRST + 10).contains(&button) {
            unsafe { midi_send_cc_raw(CC_BLUE_PAGE, button - EVENT_PAGE_FIRST) };
        }
    }
    hook_continue(event, context)
}
