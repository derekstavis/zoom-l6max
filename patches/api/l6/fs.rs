// L6 MAIN 1.10 fs bindings.

firmware_functions! {
    pub unsafe fn fs_mount(drive: u32) -> i32, at 0x8005_fc99;
    pub unsafe fn fs_ready() -> u32, at 0x8000_ac81;
    pub unsafe fn fs_open(handle: *mut u32, path: *const u16, mode: u32, flags: u32) -> i32, at 0x8005_ffe9;
    pub unsafe fn fs_read(handle: u32, data: *mut u8, length: u32, actual: *mut u32) -> i32, at 0x8006_0621;
    pub unsafe fn fs_close(handle: u32) -> i32, at 0x8005_c1f9;
    pub unsafe fn fs_find_first(pattern: *const u16, flags: u32, info: *mut u8) -> i32, at 0x8005_d8e9;
    pub unsafe fn fs_find_next(info: *mut u8) -> i32, at 0x8005_de41;
    pub unsafe fn fs_find_close(info: *mut u8) -> i32, at 0x8005_d729;
}
