ENTRY(install_effect_callback)
SECTIONS {
  . = PAYLOAD_BASE;
  .text : {
    KEEP(*(.text.patch_hook))
    KEEP(*(.text.*_hook))
    KEEP(*(.text.patch_module_init))
    *(.text*)
    *(.rodata*)
    *(.data*)
  }
  .patch_hooks : { KEEP(*(.patch_hooks)) }
  .module_bss (NOLOAD) : {
    . = ALIGN(8);
    __module_bss_start = .;
    *(.bss*)
    *(COMMON)
    __module_memory_end = .;
  }
  /DISCARD/ : { *(.ARM.exidx*) *(.ARM.extab*) *(.comment*) }
}
