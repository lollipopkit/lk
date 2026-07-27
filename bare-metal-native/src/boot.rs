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
    // Park every core but the first. `MPIDR_EL1[7:0]` is the CPU id; without
    // this all four would run the same program and race on the same stack.
    "   mrs     x0, mpidr_el1",
    "   and     x0, x0, #0xff",
    "   cbnz    x0, 2f",
    // Bring the PL011 up before anything else.
    //
    // `program.lk`'s driver configures it too, with the same values — this is
    // not a substitute for it. It is here because the timer interrupt is armed
    // before the LK program starts running, and its handler transmits: the
    // device has to be able to transmit from reset, or a tick landing in the
    // window before `uart_init()` would write to a disabled UART.
    //   +0x24 IBRD, +0x28 FBRD, +0x2c LCR_H (8N1, FIFOs), +0x30 CR (UARTEN|TXE)
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
    // Let floating point and SIMD execute. `CPACR_EL1.FPEN` traps them at
    // reset, on the assumption that an OS wants to know before it has to save
    // those registers on a context switch. Compiled code uses them freely — LK
    // numbers are `f64` — so a program faults on its first arithmetic without
    // this.
    "   mrs     x0, cpacr_el1",
    "   orr     x0, x0, #(3 << 20)",
    "   msr     cpacr_el1, x0",
    "   isb",
    // Turn on the MMU.
    //
    // Not for virtual memory — the mapping below is the identity — but because
    // with the MMU off every access is Device-nGnRnE, and Device memory does
    // not support the exclusive instructions that back atomics. Any lock or
    // atomic counter (`lkrt`'s state, the allocator here) faults immediately
    // with an alignment abort. Marking RAM as Normal memory is what makes
    // ordinary compiled code legal to run.
    //
    // A four-entry level 1 table, one 1 GiB block each, covering 0-4 GiB:
    // peripherals below 1 GiB stay Device, RAM at 0x4000_0000 becomes Normal
    // write-back, and the rest stays Device so a stray access still faults
    // rather than silently succeeding in a cache.
    "   ldr     x0, =__page_table",
    "   movz    x1, #0x0401",                 // Device block at 0x0000_0000
    "   str     x1, [x0, #0]",
    "   movz    x1, #0x0705",                 // Normal block at 0x4000_0000
    "   movk    x1, #0x4000, lsl #16",
    "   str     x1, [x0, #8]",
    "   movz    x1, #0x0401",
    "   movk    x1, #0x8000, lsl #16",
    "   str     x1, [x0, #16]",
    "   movz    x1, #0x0401",
    "   movk    x1, #0xc000, lsl #16",
    "   str     x1, [x0, #24]",
    "   msr     ttbr0_el1, x0",
    // MAIR attr0 = Device-nGnRnE (0x00), attr1 = Normal write-back (0xff).
    "   movz    x1, #0xff00",
    "   msr     mair_el1, x1",
    // TCR: T0SZ=32 (4 GiB), 4 KiB granule, walks cacheable and inner
    // shareable, TTBR1 disabled (nothing is mapped high).
    "   ldr     x1, =0x0000000200803520",
    "   msr     tcr_el1, x1",
    "   dsb     ish",
    "   tlbi    vmalle1",
    "   dsb     ish",
    "   isb",
    // SCTLR: M (MMU), C (data cache), I (instruction cache). A (strict
    // alignment) stays clear — compiled code may access unaligned Normal
    // memory, which is legal once it is no longer Device.
    "   mrs     x1, sctlr_el1",
    "   orr     x1, x1, #(1 << 0)",
    "   orr     x1, x1, #(1 << 2)",
    "   orr     x1, x1, #(1 << 12)",
    "   msr     sctlr_el1, x1",
    "   isb",
    // Point the CPU at our vector table. Reset leaves `VBAR_EL1` at 0, where
    // there is no memory mapped, so any fault becomes a jump into nothing and
    // the board just stops — the failure with the least information possible.
    "   ldr     x0, =__vectors",
    "   msr     vbar_el1, x0",
    "   isb",
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

// The aarch64 exception vector table.
//
// Sixteen entries of 0x80 bytes: four exception kinds (synchronous, IRQ, FIQ,
// SError) for each of four sources (EL1 with SP0, EL1 with SPx, and 64- and
// 32-bit lower ELs). Everything except the IRQ we actually take lands in the
// reporter — the useful thing an unexpected exception can do is say what it
// was, rather than vanish.
global_asm!(
    ".section .text.vectors",
    ".align 11",
    ".global __vectors",
    "__vectors:",
    ".macro VEC_ENTRY, kind",
    "   mov     x0, \\kind",
    "   b       __fault_trampoline",
    ".align 7",
    ".endm",
    ".macro IRQ_ENTRY",
    "   b       __irq_trampoline",
    ".align 7",
    ".endm",
    ".align 7",
    "VEC_ENTRY 0",  // current EL, SP0: synchronous
    "VEC_ENTRY 1",  // current EL, SP0: IRQ
    "VEC_ENTRY 2",  // current EL, SP0: FIQ
    "VEC_ENTRY 3",  // current EL, SP0: SError
    "VEC_ENTRY 4",  // current EL, SPx: synchronous
    "IRQ_ENTRY",    // current EL, SPx: IRQ — the one the timer arrives on
    "VEC_ENTRY 6",
    "VEC_ENTRY 7",
    "VEC_ENTRY 8",  // lower EL, aarch64
    "VEC_ENTRY 9",
    "VEC_ENTRY 10",
    "VEC_ENTRY 11",
    "VEC_ENTRY 12", // lower EL, aarch32
    "VEC_ENTRY 13",
    "VEC_ENTRY 14",
    "VEC_ENTRY 15",
    "__fault_trampoline:",
    // The faulting state is already in system registers; hand it to Rust in
    // argument order. The stack is whatever the fault left behind, which is
    // good enough to report and halt on.
    "   mrs     x1, esr_el1",
    "   mrs     x2, far_el1",
    "   mrs     x3, elr_el1",
    "   b       fault_report",
    // The IRQ path, which unlike the fault path has to *return*.
    //
    // An interrupt can land between any two instructions of the interrupted
    // program, so every register the called code is allowed to clobber must be
    // saved: x0-x18 and x29/x30 on the integer side, v0-v7 and v16-v31 on the
    // floating-point side. Missing one corrupts a value in the interrupted
    // computation — a wrong answer rather than a crash, which is the hardest
    // kind of bug to find.
    "__irq_trampoline:",
    "   stp     x0, x1, [sp, #-16]!",
    "   stp     x2, x3, [sp, #-16]!",
    "   stp     x4, x5, [sp, #-16]!",
    "   stp     x6, x7, [sp, #-16]!",
    "   stp     x8, x9, [sp, #-16]!",
    "   stp     x10, x11, [sp, #-16]!",
    "   stp     x12, x13, [sp, #-16]!",
    "   stp     x14, x15, [sp, #-16]!",
    "   stp     x16, x17, [sp, #-16]!",
    "   stp     x18, x29, [sp, #-16]!",
    "   str     x30, [sp, #-16]!",
    "   stp     q0, q1, [sp, #-32]!",
    "   stp     q2, q3, [sp, #-32]!",
    "   stp     q4, q5, [sp, #-32]!",
    "   stp     q6, q7, [sp, #-32]!",
    "   stp     q16, q17, [sp, #-32]!",
    "   stp     q18, q19, [sp, #-32]!",
    "   stp     q20, q21, [sp, #-32]!",
    "   stp     q22, q23, [sp, #-32]!",
    "   stp     q24, q25, [sp, #-32]!",
    "   stp     q26, q27, [sp, #-32]!",
    "   stp     q28, q29, [sp, #-32]!",
    "   stp     q30, q31, [sp, #-32]!",
    "   bl      irq_dispatch",
    "   ldp     q30, q31, [sp], #32",
    "   ldp     q28, q29, [sp], #32",
    "   ldp     q26, q27, [sp], #32",
    "   ldp     q24, q25, [sp], #32",
    "   ldp     q22, q23, [sp], #32",
    "   ldp     q20, q21, [sp], #32",
    "   ldp     q18, q19, [sp], #32",
    "   ldp     q16, q17, [sp], #32",
    "   ldp     q6, q7, [sp], #32",
    "   ldp     q4, q5, [sp], #32",
    "   ldp     q2, q3, [sp], #32",
    "   ldp     q0, q1, [sp], #32",
    "   ldr     x30, [sp], #16",
    "   ldp     x18, x29, [sp], #16",
    "   ldp     x16, x17, [sp], #16",
    "   ldp     x14, x15, [sp], #16",
    "   ldp     x12, x13, [sp], #16",
    "   ldp     x10, x11, [sp], #16",
    "   ldp     x8, x9, [sp], #16",
    "   ldp     x6, x7, [sp], #16",
    "   ldp     x4, x5, [sp], #16",
    "   ldp     x2, x3, [sp], #16",
    "   ldp     x0, x1, [sp], #16",
    "   eret",
);
