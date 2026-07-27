//! Interrupts: an IDT, the legacy 8259 PIC, and the trampoline that reaches
//! the LK handler.
//!
//! The division of labour matches the aarch64 demo. The board decides *which
//! vector* an interrupt lands on and does the acknowledging; what a tick
//! *means* is the program's, and that part is LK.

use core::arch::global_asm;

/// The 8259 pair's command and data ports.
const PIC1_CMD: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_CMD: u16 = 0xa0;
const PIC2_DATA: u16 = 0xa1;

unsafe extern "C" {
    /// The assembly trampolines. The timer's lives in `tasks`, because
    /// returning on a *different* stack is what a task switch is.
    fn __task_trampoline();
    fn __keyboard_trampoline();
    fn __mouse_trampoline();
    fn __syscall_trampoline();
    fn __yield_trampoline();
}

/// Remaps the PIC, unmasks the lines that have handlers, and enables interrupts.
///
/// Called *from `program.lk`*, not from `kernel_main`, and the order is the
/// reason: the program builds its own interrupt table now, and enabling
/// interrupts before it has been loaded would take the first tick through
/// whatever the old table said. Naming this from LK is how that order becomes
/// something the program states rather than something two files agree about.
///
/// What is left here is what the 8259 is: a chip with a four-write
/// initialisation sequence and a mask register. It moves next, together with
/// the end-of-interrupt its handlers send — those are one decision, and
/// splitting them across the boundary would put the command port in two files.
// TODO: move the PIC to `drivers/pic.lk` along with the EOI in the dispatch
// functions below, and delete this.
#[unsafe(no_mangle)]
pub extern "C" fn lk_interrupts_start(pic_base: i64) {
    // SAFETY: fixed ISA ports, and a flag instruction on a single-threaded
    // boot path whose interrupt table is already loaded.
    unsafe {
        // Remap the PIC. The initialisation sequence is four writes per chip,
        // in order, and the chip latches them as ICW1-ICW4.
        crate::port_out_u8(PIC1_CMD, 0x11); // ICW1: begin init, expect ICW4
        crate::port_out_u8(PIC2_CMD, 0x11);
        // ICW2: where each chip's eight lines land. The number comes from the
        // caller because the caller is what decides it: `program.lk` builds the
        // gates, so it is the one place that knows which vector means the timer.
        // Naming it here as well would be a second answer to a question with
        // one, and the failure of the two disagreeing is a device raising a
        // vector nothing filled in.
        crate::port_out_u8(PIC1_DATA, pic_base as u8);
        crate::port_out_u8(PIC2_DATA, pic_base as u8 + 8);
        crate::port_out_u8(PIC1_DATA, 0x04); // ICW3: slave on IRQ2
        crate::port_out_u8(PIC2_DATA, 0x02);
        crate::port_out_u8(PIC1_DATA, 0x01); // ICW4: 8086 mode
        crate::port_out_u8(PIC2_DATA, 0x01);
        // Unmask the timer, the keyboard and the cascade; mask the rest. An
        // unmasked line with no handler is a vector into a not-present gate,
        // which is a general protection fault inside an interrupt. The cascade
        // (IRQ2) is not a device — it is how the slave chip reaches the CPU at
        // all, so the mouse's IRQ12 is invisible without it.
        crate::port_out_u8(PIC1_DATA, 0xf8);
        crate::port_out_u8(PIC2_DATA, 0xef);

        core::arch::asm!("sti", options(nomem, nostack));
    }
}

/// Masks the timer and disables interrupts, in that order — disabling first
/// would leave a pending interrupt to be taken the moment anything unmasks.
pub fn stop() {
    // SAFETY: fixed ISA ports and a flag instruction.
    unsafe {
        crate::port_out_u8(PIC1_DATA, 0xff);
        core::arch::asm!("cli", options(nomem, nostack));
    }
}

unsafe extern "C" {
    /// The interrupt handlers, written in LK. `#[export("…")]` in `program.lk`
    /// is what makes these names exist.
    fn lk_timer_isr();
    fn lk_key_isr();
}

/// Called from the trampoline with every caller-saved register already spilled.
#[unsafe(no_mangle)]
pub extern "C" fn pit_dispatch() {
    // SAFETY: the LK function `#[export]`ed under that name, compiled to a
    // `void(void)` by the same build.
    unsafe { lk_timer_isr() };
    // End-of-interrupt. Without it the PIC never delivers IRQ0 again.
    // SAFETY: a fixed ISA port.
    unsafe { crate::port_out_u8(PIC1_CMD, 0x20) };
}

/// As [`pit_dispatch`], for the mouse — with the slave chip's end-of-interrupt
/// as well as the master's. Acknowledging only the master leaves the slave
/// believing the interrupt is still in service, and it never raises another:
/// the mouse moves once and then stops, with nothing to say why.
#[unsafe(no_mangle)]
pub extern "C" fn mouse_dispatch() {
    unsafe extern "C" {
        fn lk_mouse();
    }
    // SAFETY: the LK handler is an `#[export]`ed function with no arguments.
    unsafe { lk_mouse() };
    // SAFETY: fixed ISA ports.
    unsafe {
        crate::port_out_u8(PIC2_CMD, 0x20);
        crate::port_out_u8(PIC1_CMD, 0x20);
    }
}

/// As [`pit_dispatch`], for the keyboard.
///
/// The scancode is deliberately *not* read here: the controller's data port is
/// the driver's business, and the driver is LK. What the board owes the device
/// is the acknowledgement.
#[unsafe(no_mangle)]
pub extern "C" fn keyboard_dispatch() {
    // SAFETY: as `pit_dispatch`.
    unsafe { lk_key_isr() };
    // SAFETY: a fixed ISA port.
    unsafe { crate::port_out_u8(PIC1_CMD, 0x20) };
}

// An interrupt lands between any two instructions of the interrupted program,
// so every register the called code may clobber has to be saved: the System V
// caller-saved integer registers, and all sixteen XMM registers because LK
// numbers are `f64` and the interrupted computation may hold one. Missing a
// register corrupts a value rather than crashing, which is the hardest kind of
// bug to find.
//
// The CPU aligns RSP to 16 bytes when it takes an interrupt in 64-bit mode.
// Nine 8-byte pushes leave it misaligned, so the `sub` below both reserves the
// XMM area and restores the alignment `call` expects.
global_asm!(
    ".section .text, \"ax\"",
    // The spill/restore is identical for every IRQ, so it lives in a macro
    // rather than being copied per vector — a register missing from one copy
    // corrupts a value only when that particular interrupt lands.
    ".macro IRQ_SAVE",
    "   push rax",
    "   push rcx",
    "   push rdx",
    "   push rsi",
    "   push rdi",
    "   push r8",
    "   push r9",
    "   push r10",
    "   push r11",
    "   sub rsp, 264",
    "   movups [rsp + 0], xmm0",
    "   movups [rsp + 16], xmm1",
    "   movups [rsp + 32], xmm2",
    "   movups [rsp + 48], xmm3",
    "   movups [rsp + 64], xmm4",
    "   movups [rsp + 80], xmm5",
    "   movups [rsp + 96], xmm6",
    "   movups [rsp + 112], xmm7",
    "   movups [rsp + 128], xmm8",
    "   movups [rsp + 144], xmm9",
    "   movups [rsp + 160], xmm10",
    "   movups [rsp + 176], xmm11",
    "   movups [rsp + 192], xmm12",
    "   movups [rsp + 208], xmm13",
    "   movups [rsp + 224], xmm14",
    "   movups [rsp + 240], xmm15",
    ".endm",
    ".macro IRQ_RESTORE",
    "   movups xmm0, [rsp + 0]",
    "   movups xmm1, [rsp + 16]",
    "   movups xmm2, [rsp + 32]",
    "   movups xmm3, [rsp + 48]",
    "   movups xmm4, [rsp + 64]",
    "   movups xmm5, [rsp + 80]",
    "   movups xmm6, [rsp + 96]",
    "   movups xmm7, [rsp + 112]",
    "   movups xmm8, [rsp + 128]",
    "   movups xmm9, [rsp + 144]",
    "   movups xmm10, [rsp + 160]",
    "   movups xmm11, [rsp + 176]",
    "   movups xmm12, [rsp + 192]",
    "   movups xmm13, [rsp + 208]",
    "   movups xmm14, [rsp + 224]",
    "   movups xmm15, [rsp + 240]",
    "   add rsp, 264",
    "   pop r11",
    "   pop r10",
    "   pop r9",
    "   pop r8",
    "   pop rdi",
    "   pop rsi",
    "   pop rdx",
    "   pop rcx",
    "   pop rax",
    ".endm",
    ".global __mouse_trampoline",
    "__mouse_trampoline:",
    // The same full save as the keyboard's, and for the same reason: an
    // interrupt is not a call. The code it lands in never agreed to lose its
    // caller-saved registers, and the handler is compiled LK — it uses them,
    // and the SSE ones. The first version of this did `call` and `iretq` with
    // nothing in between, which corrupts whatever it interrupted at a moment
    // nothing can predict.
    "   IRQ_SAVE",
    "   call mouse_dispatch",
    "   IRQ_RESTORE",
    "   iretq",
    ".global __keyboard_trampoline",
    "__keyboard_trampoline:",
    "   IRQ_SAVE",
    "   call keyboard_dispatch",
    "   IRQ_RESTORE",
    "   iretq",
);


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
