ENTRY(knob_changed)
SECTIONS {
  . = PAYLOAD_BASE;
  .text : { KEEP(*(.text.knob_changed)) KEEP(*(.text.*_hook)) KEEP(*(.text.sd_dynamic_initialize)) *(.text*) *(.rodata*) }
  .patch_hooks : { KEEP(*(.patch_hooks)) }
  .state (NOLOAD) : { *(.data*) *(.bss*) }
  ASSERT(SIZEOF(.state) == 0, "patch must not reserve mutable RAM")
  /DISCARD/ : { *(.ARM.exidx*) *(.ARM.extab*) *(.comment*) }
}
