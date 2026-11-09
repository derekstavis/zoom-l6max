// L6MAX MAIN 1.10 midi bindings.

firmware_functions! {
    pub unsafe fn midi_in(message: *const u8) at 0x8003_f7c9;
    pub unsafe fn mixer_control_via_midi() -> u32, at 0x8003_ff10;
    pub unsafe fn apply_mixer_cc(message: *const u8) at 0x8003_f7c8;
    pub unsafe fn midi_channel() -> u32, at 0x8003_feb0;
    pub unsafe fn midi_send_cc(parameter: u32, value: u32) at 0x8003_f9b0;
}

// L6max MAIN 1.10 raw-CC helper. The stock MIDI sender's internal tail starts
// after its user CC-map lookup and expects the stock 16-byte saved-register
// frame plus 8-byte local area. Recreate that frame, resolve the configured
// output channel, then tail-enter the stock sender path so its routing and
// epilogue remain in use.
core::arch::global_asm!(
    ".syntax unified\n\
     .thumb\n\
     .section .text.patchrt_midi_send_cc_raw, \"ax\"\n\
     .global __l6_patchrt_midi_send_cc_raw\n\
     .thumb_func\n\
     __l6_patchrt_midi_send_cc_raw:\n\
     push {r4, r5, r6, lr}\n\
     sub sp, #8\n\
     mov r4, r1\n\
     mov r6, r0\n\
     ldr r3, =0x8003feb1\n\
     blx r3\n\
     mov r5, r0\n\
     mov r0, r6\n\
     ldr r3, =0x8003f9c9\n\
     bx r3\n\
     .ltorg\n",
    options(raw)
);

unsafe extern "C" {
    fn __l6_patchrt_midi_send_cc_raw(controller: u32, value: u32);
}

/// Send a raw CC number without applying the user's CC map, while retaining
/// the firmware's configured MIDI channel and output routing.
///
/// # Safety
/// Only valid on L6max MAIN 1.10 and from a task context allowed to use the
/// stock MIDI output path.
pub unsafe fn midi_send_cc_raw(controller: u32, value: u32) {
    unsafe { __l6_patchrt_midi_send_cc_raw(controller, value) }
}

firmware_symbols! {
    /// Entry of `MidiUartRcvMsg`; static insertion hooks name offsets from it.
    pub midi_uart_rcv_msg_task at 0x8003_e6d0;
}

firmware_variables! {
    pub message_length: u16, at 0x805e_5d4a;
    pub message_buffer: u8, at 0x8089_8938;
}
