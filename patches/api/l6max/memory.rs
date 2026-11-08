// L6MAX MAIN 1.10 memory bindings.

firmware_functions! {
    pub unsafe fn fw_malloc(size: usize) -> *mut u8, at 0x8008_5479;
    pub unsafe fn fw_free(pointer: *mut u8) at 0x8008_e589;
    pub unsafe fn pv_port_malloc(size: u32) -> u32, at 0x8008_5478;
    pub unsafe fn v_port_free(pointer: u32) at 0x8008_e588;
}
