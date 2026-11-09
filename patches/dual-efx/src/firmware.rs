//! Firmware bindings and memory addresses for LiveTrak L6max 1.10.

use core::mem::transmute;

pub const MIXER_GET_EFX: usize = 0x8000b0f0;
pub const MIXER_GET_EFX_TYPE: usize = 0x8000b270;
pub const MIXER_GET_EFX_PARAM: usize = 0x8000b240;
pub const MIXER_APPLY_CHANNEL_PARAMETER: usize = 0x80053b28;
pub const MIXER_CHANNEL_INDEX: usize = 0x800564b0;

pub const PV_PORT_MALLOC: usize = 0x80085478;
pub const V_PORT_FREE: usize = 0x8008e588;

pub const GUI_DRAW_BACKGROUND: usize = 0x80005e70;
pub const GUI_CLEAR_LAYER_RECT: usize = 0x80005f18;
pub const GUI_PUBLISH: usize = 0x80005f50;
pub const GUI_DRAW_TEXT_WIDGET: usize = 0x80006ad8;
pub const GUI_SET_WIDGET_POLARITY: usize = 0x80007080;
pub const GUI_SET_WIDGET_Y: usize = 0x800070b0;
pub const GUI_SET_WIDGET_TEXT: usize = 0x800070d8;

pub const AUDIO_EFX_IN_BUFFER: usize = 0x804683a4;
pub const AUDIO_EFX_OUT_BUFFER: usize = 0x8045fe98;

pub unsafe fn call0(address: usize) -> u32 {
    unsafe { transmute::<usize, unsafe extern "C" fn() -> u32>(address | 1)() }
}

pub unsafe fn call1(address: usize, a: u32) -> u32 {
    unsafe { transmute::<usize, unsafe extern "C" fn(u32) -> u32>(address | 1)(a) }
}

pub unsafe fn call2(address: usize, a: u32, b: u32) -> u32 {
    unsafe { transmute::<usize, unsafe extern "C" fn(u32, u32) -> u32>(address | 1)(a, b) }
}

pub unsafe fn call3(address: usize, a: u32, b: u32, c: u32) -> u32 {
    unsafe { transmute::<usize, unsafe extern "C" fn(u32, u32, u32) -> u32>(address | 1)(a, b, c) }
}

pub unsafe fn call4(address: usize, a: u32, b: u32, c: u32, d: u32) -> u32 {
    unsafe {
        transmute::<usize, unsafe extern "C" fn(u32, u32, u32, u32) -> u32>(address | 1)(a, b, c, d)
    }
}
