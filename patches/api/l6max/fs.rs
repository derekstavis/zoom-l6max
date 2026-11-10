// L6MAX MAIN 1.10 fs bindings.

firmware_functions! {
    pub unsafe fn fs_mount(drive: u32) -> i32, at 0x8007_6e31;
    pub unsafe fn fs_ready() -> u32, at 0x8000_ebc9;
    pub unsafe fn fs_open(handle: *mut u32, path: *const u16, mode: u32, flags: u32) -> i32, at 0x8007_71a1;
    pub unsafe fn fs_read(handle: u32, data: *mut u8, length: u32, actual: *mut u32) -> i32, at 0x8007_77c9;
    pub unsafe fn fs_close(handle: u32) -> i32, at 0x8007_33e1;
    pub unsafe fn fs_find_first(pattern: *const u16, flags: u32, info: *mut u8) -> i32, at 0x8007_4a79;
    pub unsafe fn fs_find_next(info: *mut u8) -> i32, at 0x8007_4fd1;
    pub unsafe fn fs_find_close(info: *mut u8) -> i32, at 0x8007_48c1;
}
