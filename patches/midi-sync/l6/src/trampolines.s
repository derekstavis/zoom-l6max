// The four displaced bytes are embedded from the exact local L6 image.
.thumb
.global original_midi_in
.thumb_func
original_midi_in:
.incbin "main_firmware.bin", 0x2e490, 4
ldr pc, =0x8002f495
.ltorg
