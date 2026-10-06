ENTRY(knob_changed)
SECTIONS {
  . = 0x80108000;
  .text : { KEEP(*(.text.knob_changed)) KEEP(*(.text.*_hook)) *(.text*) *(.rodata*) }
  .state (NOLOAD) : { *(.data*) *(.bss*) }
  ASSERT(SIZEOF(.state) == 0, "patch must not reserve mutable RAM")
  /DISCARD/ : { *(.ARM.exidx*) *(.ARM.extab*) *(.comment*) }
}
