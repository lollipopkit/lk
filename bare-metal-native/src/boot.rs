//! The reset path.
//!
//! QEMU's `-kernel` loads the ELF and jumps to its entry with nothing set up:
//! no stack, no zeroed `.bss`, and all four cores released at once. This does
//! the minimum to make Rust callable, and no more — anything further is the
//! board's business rather than the language's.

use core::arch::global_asm;

global_asm!(
    ".section .text.boot",
    ".global _boot",
    "_boot:",
    // Bring up the PL011 before anything tries to print. Reset leaves it
    // disabled, so a write to the data register goes nowhere — which looks
    // exactly like the boot path never running.
    //   +0x30 CR: bit 0 UARTEN, bit 8 TXE
    //   +0x24 IBRD, +0x28 FBRD, +0x2c LCR_H (8N1, FIFO enabled)
    "   mov     x9, #0x09000000",
    "   mov     w10, #0",
    "   str     w10, [x9, #0x30]",
    "   mov     w10, #1",
    "   str     w10, [x9, #0x24]",
    "   mov     w10, #40",
    "   str     w10, [x9, #0x28]",
    "   mov     w10, #0x70",
    "   str     w10, [x9, #0x2c]",
    "   mov     w10, #0x301",
    "   str     w10, [x9, #0x30]",
    // Park every core but the first. `MPIDR_EL1[7:0]` is the CPU id; without
    // this all four would run the same program and race on the same stack.
    "   mrs     x0, mpidr_el1",
    "   and     x0, x0, #0xff",
    "   cbnz    x0, 2f",
    // A stack, before anything compiled runs.
    "   ldr     x0, =__stack_top",
    "   mov     sp, x0",
    // Zero `.bss`. Rust assumes it, and RAM comes up with whatever was there.
    "   ldr     x0, =__bss_start",
    "   ldr     x1, =__bss_end",
    "1: cmp     x0, x1",
    "   b.hs    3f",
    "   str     xzr, [x0], #8",
    "   b       1b",
    "3: bl      kernel_main",
    // `kernel_main` does not return; if it somehow does, park rather than
    // execute whatever follows in memory.
    "2: wfe",
    "   b       2b",
);
