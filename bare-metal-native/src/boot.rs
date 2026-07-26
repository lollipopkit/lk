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
// 32-bit lower ELs). Every one lands in the same reporter — this board has no
// interrupt handling yet, so the only useful thing any of them can do is say
// what went wrong.
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
    ".align 7",
    "VEC_ENTRY 0",  // current EL, SP0: synchronous
    "VEC_ENTRY 1",  // current EL, SP0: IRQ
    "VEC_ENTRY 2",  // current EL, SP0: FIQ
    "VEC_ENTRY 3",  // current EL, SP0: SError
    "VEC_ENTRY 4",  // current EL, SPx: synchronous
    "VEC_ENTRY 5",
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
);
