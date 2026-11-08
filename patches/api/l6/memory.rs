// L6 MAIN 1.10 memory bindings.

firmware_functions! {
    pub unsafe fn fw_malloc(size: usize) -> *mut u8, at 0x8006_de79;
    pub unsafe fn fw_free(pointer: *mut u8) at 0x8007_3f49;
}
