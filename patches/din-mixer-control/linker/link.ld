ENTRY(din_mixer_forward)
SECTIONS {
  . = PAYLOAD_BASE;
  .text : { KEEP(*(.text.patch_hook)) KEEP(*(.text.generated_hook_*)) *(.text*) *(.rodata*) }
  .patch_hooks : { KEEP(*(.patch_hooks)) }
  .state (NOLOAD) : { *(.data*) *(.bss*) }
  ASSERT(SIZEOF(.state) == 0, "patch must not reserve mutable RAM")
  /DISCARD/ : { *(.ARM.exidx*) *(.ARM.extab*) *(.comment*) }
}
