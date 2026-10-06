//! Revision-specific firmware bindings used by the knob notification patch.
//! Function entry addresses are even; call helpers select Thumb mode.
use core::mem::transmute;
pub const GUI_DRAW_BACKGROUND: usize = 0x80005e70;
pub const GUI_CLEAR_LAYER_RECT: usize = 0x80005f18;
pub const GUI_PUBLISH: usize = 0x80005f50;
pub const GUI_DRAW_TEXT_WIDGET: usize = 0x80006ad8;
pub const GUI_SET_WIDGET_POLARITY: usize = 0x80007080;
pub const GUI_SET_WIDGET_Y: usize = 0x800070b0;
pub const MIXER_GET_AUX: usize = 0x8000b080;
pub const MIXER_GET_LEVEL: usize = 0x8000b0b0;
pub const MIXER_GET_EFX: usize = 0x8000b0f0;
pub const MIXER_GET_EQ_FREQUENCY: usize = 0x8000b120;
pub const MIXER_GET_EQ_GAIN: usize = 0x8000b140;
pub const MIXER_GET_PAN: usize = 0x8000b1d0;
pub const MIXER_GET_SUBMIX: usize = 0x8000b2a0;
pub const MIXER_APPLY_CHANNEL_PARAMETER: usize = 0x80053b28;
pub const MIXER_CHANNEL_INDEX: usize = 0x800564b0;
pub const GUI_SET_WIDGET_TEXT: usize = 0x800070d8;
pub const PV_PORT_MALLOC: usize = 0x80085478;
pub const V_PORT_FREE: usize = 0x8008e588;

pub unsafe fn call1(address: usize, a: u32) -> u32 {
    unsafe { transmute::<usize, unsafe extern "C" fn(u32) -> u32>(address | 1)(a) }
}
pub unsafe fn call2(address: usize, a: u32, b: u32) -> u32 {
    unsafe { transmute::<usize, unsafe extern "C" fn(u32, u32) -> u32>(address | 1)(a, b) }
}
pub unsafe fn clear_layer_rect(layer: u32, x0: u32, y0: u32, x1: u32, y1: u32) {
    unsafe {
        transmute::<usize, unsafe extern "C" fn(u32, u32, u32, u32, u32)>(GUI_CLEAR_LAYER_RECT | 1)(
            layer, x0, y0, x1, y1,
        );
    }
}

// Tables used by the original EQ coefficient routines, not reconstructed curves.
pub const EQ_GAIN_DB_TABLE: u32 = 0x800a5138;
pub const EQ_MID_FREQUENCY_HZ_TABLE: u32 = 0x800a5738;

pub const FIRMWARE_SEMAPHORE_CREATE: usize = 0x8005f600;
pub const X_QUEUE_SEMAPHORE_TAKE: usize = 0x80091048;
pub const X_QUEUE_GENERIC_SEND: usize = 0x80090ad0;
pub const V_PORT_ENTER_CRITICAL: usize = 0x8008e508;
pub const V_PORT_EXIT_CRITICAL: usize = 0x8008e558;
pub const GUI_DISPLAY_CALLBACK_REGISTER: usize = 0x80024ed0;
pub const GUI_NOTIFICATION_TICK: usize = 0x800064c0;
pub unsafe fn call3(address: usize, a: u32, b: u32, c: u32) -> u32 {
    unsafe { transmute::<usize, unsafe extern "C" fn(u32, u32, u32) -> u32>(address | 1)(a, b, c) }
}
pub unsafe fn call4(address: usize, a: u32, b: u32, c: u32, d: u32) -> u32 {
    unsafe {
        transmute::<usize, unsafe extern "C" fn(u32, u32, u32, u32) -> u32>(address | 1)(a, b, c, d)
    }
}
