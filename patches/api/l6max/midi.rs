// L6MAX MAIN 1.10 midi bindings.

firmware_functions! {
    pub unsafe fn midi_in(message: *const u8) at 0x8003_f7c9;
    pub unsafe fn mixer_control_via_midi() -> u32, at 0x8003_ff10;
    pub unsafe fn apply_mixer_cc(message: *const u8) at 0x8003_f7c8;
    pub unsafe fn midi_channel() -> u32, at 0x8003_feb0;
    pub unsafe fn midi_send_cc(parameter: u32, value: u32) at 0x8003_f9b0;
}

firmware_symbols! {
    /// Entry of `MidiUartRcvMsg`; static insertion hooks name offsets from it.
    pub midi_uart_rcv_msg_task at 0x8003_e6d0;
}

firmware_variables! {
    pub message_length: u16, at 0x805e_5d4a;
    pub message_buffer: u8, at 0x8089_8938;
}
