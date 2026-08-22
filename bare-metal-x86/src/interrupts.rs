//! What is left of interrupts on the board's side: the trampolines, and the
//! exception reporter.
//!
//! Everything that is a *decision* has moved to LK. The table lives in
//! `drivers/idt.lk`, the vector map and the install order in `program.lk`, the
//! 8259 and its end-of-interrupt in `drivers/pic.lk`. What could not move is
//! here, and the line is sharp: an interrupt is not a call. The code it lands
//! in never agreed to lose its caller-saved registers, so a compiled handler
//! has to be entered through a stub that spills every one of them and leaves
//! with `iretq`. There is no language in which that is not assembly.
//!
//! The exception reporter stays for a different reason, and the one first given
//! for it was wrong. It was "formatting a report in LK would allocate", which
//! the clock task disproves — that one draws its digits by dividing, and
//! allocates nothing.
//!
//! The real reason is that **every dependency a fault reporter has is a way for
//! the report not to happen**. This one has two: a sixteen-byte buffer on its
//! own stack, and `out` to a port. It does not read the shared page, call
//! through a table, or touch an allocator — so a fault that damaged any of
//! those still gets reported. Moving it would trade that for making the list of
//! named vectors editable in LK, which is a small thing to want and a large
//! thing to pay for.
//!
//! The same argument says where the line is: if this ever needs to do something
//! a fault does not already guarantee is possible, it is doing too much.

use core::arch::global_asm;

// The keyboard's and the mouse's trampolines used to be here, one hand-written
// copy each of a spill/restore that is identical for every device interrupt.
// They are `lkrt`'s now — 256 stubs and a handler table, so a kernel points a
// gate at one and writes an address into the other. Adding a device stopped
// being an edit to this file.
//
// What could not go is the timer's, next door in `tasks`: returning on a
// *different* stack is what a task switch is, and no shared tail can do that.

/// Where every CPU exception ends up.
///
/// It reports and halts rather than trying to recover: nothing here knows how
/// to resume a faulted program, and a fault that prints its cause is the whole
/// difference between a debuggable board and a board that stops.
#[unsafe(no_mangle)]
pub extern "C" fn exception_report(vector: u64, error: u64, rip: u64, cr2: u64) -> ! {
    // Interrupts off first: the timer handler transmits on this same device.
    // SAFETY: a flag instruction.
    unsafe { core::arch::asm!("cli", options(nomem, nostack)) };
    crate::serial_write("\n!! exception ");
    crate::serial_write(vector_name(vector));
    crate::serial_write(" vector=");
    crate::write_hex(vector);
    crate::serial_write(" error=");
    crate::write_hex(error);
    crate::serial_write(" rip=");
    crate::write_hex(rip);
    // CR2 holds the faulting address for a page fault and stale data
    // otherwise; printing it unconditionally is still better than a second
    // build to find out.
    crate::serial_write(" cr2=");
    crate::write_hex(cr2);
    crate::serial_write("\n");
    // End the machine rather than parking, so a fault under a test harness
    // fails in seconds instead of hitting its timeout. QEMU's `isa-debug-exit`
    // is at 0xf4; a machine without it ignores the write and falls through to
    // the halt below.
    // SAFETY: a fixed ISA port.
    unsafe { crate::port_out_u8(0xf4, 1) };
    loop {
        // SAFETY: parks the core rather than spinning.
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)) };
    }
}

/// The names worth recognising at a glance. The rest report as their number.
fn vector_name(vector: u64) -> &'static str {
    match vector {
        0 => "#DE divide error",
        3 => "#BP breakpoint",
        6 => "#UD invalid opcode",
        8 => "#DF double fault",
        11 => "#NP segment not present",
        12 => "#SS stack fault",
        13 => "#GP general protection",
        14 => "#PF page fault",
        16 => "#MF x87 fault",
        17 => "#AC alignment check",
        19 => "#XM SIMD fault",
        _ => "exception",
    }
}

// The 32 exception stubs, and the tail they share.
//
// The CPU pushes an error code for some vectors and not others, and tells the
// handler nothing about which vector fired. So each stub pushes a dummy zero
// where there is no error code, then its own number — after which the stack
// layout is the same for all 32 and one common tail can read it.
//
// `.byte 0x6a, n` is `push imm8`: writing the opcode directly avoids the
// assembler treating a `.set` symbol as an address.
global_asm!(
    ".section .text, \"ax\"",
    ".global ISR_STUBS",
    ".set ERRMASK, (1<<8)|(1<<10)|(1<<11)|(1<<12)|(1<<13)|(1<<14)|(1<<17)|(1<<21)|(1<<29)|(1<<30)",
    ".align 16",
    "ISR_STUBS:",
    ".set vec, 0",
    ".rept 32",
    "   .align 16",
    "   .if ((ERRMASK >> vec) & 1) == 0",
    "   .byte 0x6a, 0",
    "   .endif",
    "   .byte 0x6a, vec",
    "   jmp __exception_common",
    "   .set vec, vec + 1",
    ".endr",
    // Past the *padding* of the last stub, not past its last instruction.
    //
    // The `.align 16` above is at the top of each iteration, so without this
    // one the array ends nine bytes into its final slot and `end - start` is
    // 505 rather than 512. The caller divides by 32 to get the stride, gets 15,
    // and every stub after the first is entered at an address inside the one
    // before it. That is what happened: a deliberate page fault reported itself
    // as vector 2, with the faulting address in the error-code field.
    ".align 16",
    // One past the last stub. `program.lk` needs the stride to compute a
    // stub's address, and this is what lets it *derive* one — `(end - start)
    // / 32` — instead of naming 16 a second time. The `.align 16` above is
    // what decides the stride, and a copy of it on the other side of the
    // language boundary would be a number nothing checks.
    ".global ISR_STUBS_END",
    "ISR_STUBS_END:",
    "__exception_common:",
    // [rsp] = vector, +8 = error code, +16 = faulting RIP.
    "   mov rdi, [rsp]",
    "   mov rsi, [rsp + 8]",
    "   mov rdx, [rsp + 16]",
    "   mov rcx, cr2",
    // The frame leaves RSP 8 off what the ABI wants at a call. This never
    // returns, so realigning by clobbering RSP is free.
    "   and rsp, -16",
    "   call exception_report",
);
