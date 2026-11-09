// L6max MAIN 1.10 recorder callback bindings.

firmware_functions! {
    /// Queued Recorder callbacks receive the recorder control context. The
    /// current firmware callbacks may ignore it, but forwarding it preserves
    /// the callback ABI for entry hooks.
    pub unsafe fn recorder_play_start_callback(context: *mut u32) -> u32, at 0x8004_6d80;
    pub unsafe fn recorder_play_stop_callback(context: *mut u32) -> u32, at 0x8004_6e09;
    pub unsafe fn recorder_start_callback(context: *mut u32) -> u32, at 0x8004_7268;
    pub unsafe fn recorder_stop_callback(context: *mut u32) -> u32, at 0x8004_7338;

}

firmware_variables! {
    /// Recorder state byte read by the stock Rec/Play callbacks.
    pub recorder_mode: u8, at 0x8020_a9ec;
}
