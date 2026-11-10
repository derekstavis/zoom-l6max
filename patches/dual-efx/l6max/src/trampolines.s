// Displaced instructions and trampoline references for dual-efx patch.
.thumb
.global original_efx_dispatch
.thumb_func
original_efx_dispatch:
    ldr pc, =0x80003c59
.ltorg

.global displaced_efx_bytes
displaced_efx_bytes:
    .incbin "main_firmware.bin", 0x277c, 4
.ltorg
