ENTRY(din_in_hook)
SECTIONS {
  . = PAYLOAD_BASE;
  .text : { KEEP(*(.text.din_in_hook)) *(.text*) *(.rodata*) }
  .state (NOLOAD) : { *(.data*) *(.bss*) }
  ASSERT(SIZEOF(.state) == 0, "patch must not reserve mutable RAM")
  /DISCARD/ : { *(.ARM.exidx*) *(.ARM.extab*) *(.comment*) }
}
