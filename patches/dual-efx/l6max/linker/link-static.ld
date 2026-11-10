ENTRY(install_effect_callback)
SECTIONS {
  . = PAYLOAD_BASE;
  .text : {
    KEEP(*(.text.patch_hook))
    KEEP(*(.text.*_hook))
    *(.text*)
    *(.rodata*)
    *(.data*)
  }
  .patch_hooks : { KEEP(*(.patch_hooks)) }
  .effect_workspace_state 0x80d00000 (NOLOAD) : {
    . = ALIGN(8);
    __effect_workspace_state_start = .;
    KEEP(*(.effect_workspace_state))
    __effect_workspace_state_end = .;
  }
  ASSERT(SIZEOF(.effect_workspace_state) <= 0x100,
         "Dual EFX static state exceeds its reserved workspace prefix")
  .module_bss (NOLOAD) : {
    . = ALIGN(8);
    __module_bss_start = .;
    *(.bss*)
    *(COMMON)
    __module_memory_end = .;
  }
  /DISCARD/ : { *(.ARM.exidx*) *(.ARM.extab*) *(.comment*) }
}
