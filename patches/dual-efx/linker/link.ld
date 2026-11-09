ENTRY(efx_dispatch_hook)
SECTIONS {
  . = PAYLOAD_BASE;
  .text : {
    KEEP(*(.text.efx_dispatch_hook))
    KEEP(*(.text.*_hook))
    KEEP(*(.text.knob_changed))
    *(.text*)
    *(.rodata*)
  }
  . = 0x83e00000;
  .state (NOLOAD) : {
    *(.data*)
    *(.bss*)
    *(COMMON)
  }
  /DISCARD/ : { *(.ARM.exidx*) *(.ARM.extab*) *(.comment*) }
}
