// L6 MAIN 1.10 midi bindings.

firmware_functions! {
    pub unsafe fn midi_in(message: *const u8) at 0x8002_f491;
    pub unsafe fn midi_channel() -> u32, at 0x8000_5ed8;
    pub unsafe fn midi_send_mapped_cc(channel: u32, group: u32, value: u32) at 0x8000_8330;
}
