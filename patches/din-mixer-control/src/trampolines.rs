use core::arch::global_asm;
global_asm!(include_str!("trampolines.s"), options(raw));
