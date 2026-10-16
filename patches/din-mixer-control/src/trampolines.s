// The displaced instruction is loaded only from locally supplied firmware.
// The patcher verifies the full source hash and the hook region first.
//
// Entered by a plain branch (not a call) from the middle of
// MidiUartRcvMsg, so every register must survive unchanged except for the
// one instruction this hook displaces (movw r0,#0x90b4), which is replayed
// verbatim before resuming.
.thumb
.section .text.din_in_hook, "ax"
.global din_in_hook
.thumb_func
din_in_hook:
    push {r0-r3, r4-r12, lr}
    bl din_mixer_forward
    pop {r0-r3, r4-r12, lr}
.incbin "main_firmware.bin", 0x3e8ee, 4
    ldr pc, =0x8003e8f3
.ltorg
