// The displaced instructions are loaded only from locally supplied firmware.
// The patcher verifies the full source hash and the hook region first.
.thumb
.global original_midi_in
.thumb_func
original_midi_in:
.incbin "main_firmware.bin", 0x3f7c8, 4
ldr pc, =0x8003f7cd
.ltorg
