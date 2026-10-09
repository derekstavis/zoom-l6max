use core::arch::global_asm;
global_asm!(include_str!("trampolines.s"), options(raw));
unsafe extern "C" {
    fn original_midi_in();
}
/// Entry of the stock inbound MIDI message handler, past its displaced prologue.
pub fn midi_in() -> usize {
    original_midi_in as *const () as usize
}
