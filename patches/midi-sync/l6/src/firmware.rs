//! L6 1.10 firmware bindings for MIDI state sync.
use core::mem::transmute;

pub const GET_EQ_FREQUENCY: usize = 0x80005f90;
pub const GET_EQ_GAIN: usize = 0x80005fa8;
pub const GET_AUX: usize = 0x800060e8;
pub const GET_EFX: usize = 0x80005f60;
pub const GET_PAN: usize = 0x80006028;
pub const GET_LEVEL: usize = 0x80005f40;
pub const CHANNEL_INDEX: usize = 0x80040ae8;
pub const MIDI_CHANNEL: usize = 0x80005ed8;
pub const MIDI_SEND_MAPPED_CC: usize = 0x80008330;

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
