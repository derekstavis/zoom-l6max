ENTRY(sd_reset_registry)
SECTIONS {
  . = PAYLOAD_BASE;
  .text : { KEEP(*(.text.sd_reset_registry)) KEEP(*(.text.dispatch_midi_in)) *(.text*) *(.rodata*) }
  .patch_hooks : { KEEP(*(.patch_hooks)) }
  .state (NOLOAD) : { *(.data*) *(.bss*) }
  ASSERT(SIZEOF(.state) == 0, "loader must not reserve mutable RAM")
  /DISCARD/ : { *(.ARM.exidx*) *(.ARM.extab*) *(.comment*) }
}
