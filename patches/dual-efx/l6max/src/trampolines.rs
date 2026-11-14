use core::arch::global_asm;

global_asm!(include_str!("trampolines.s"), options(raw));

unsafe extern "C" {
    pub fn original_efx_dispatch(inst: u32, efx_type: u32, param0: u32, param1: u32);
}
