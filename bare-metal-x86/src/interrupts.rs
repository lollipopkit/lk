//! Interrupts: an IDT, the legacy 8259 PIC, and the trampoline that reaches
//! the LK handler.
//!
//! The division of labour matches the aarch64 demo. The board decides *which
//! vector* an interrupt lands on and does the acknowledging; what a tick
//! *means* is the program's, and that part is LK.

use core::arch::global_asm;

/// One IDT entry. The handler address is split across three fields because the
/// layout predates 64-bit addresses and was extended twice.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Gate {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

#[repr(C, packed)]
struct Descriptor {
    limit: u16,
    base: u64,
}

/// 256 entries because the CPU indexes this table by vector number and will
/// read whatever is at the index it computes — a short table is a fault that
/// reads past the end.
static mut IDT: [Gate; 256] = [Gate {
    offset_low: 0,
    selector: 0,
    ist: 0,
    type_attr: 0,
    offset_mid: 0,
    offset_high: 0,
    reserved: 0,
}; 256];

/// Where the PIC's IRQ0 is remapped to. 0-31 are reserved for CPU exceptions,
/// and the PIC's power-on default overlaps them — which is why every kernel
/// remaps it before enabling interrupts.
const PIT_VECTOR: usize = 0x20;

/// The 8259 pair's command and data ports.
const PIC1_CMD: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_CMD: u16 = 0xa0;
const PIC2_DATA: u16 = 0xa1;

unsafe extern "C" {
    /// The assembly trampoline below.
    fn __pit_trampoline();
}

/// Fills in one gate.
///
/// # Safety
///
/// `handler` must be a function the CPU can enter with an interrupt frame on
/// the stack — one of the stubs in this file, not an ordinary Rust function.
unsafe fn set_gate(idt: *mut [Gate; 256], vector: usize, handler: u64) {
    unsafe {
        (*idt)[vector] = Gate {
            offset_low: handler as u16,
            // The 64-bit code selector the boot GDT defines.
            selector: 0x08,
            ist: 0,
            // Present, ring 0, 64-bit interrupt gate. "Interrupt" rather than
            // "trap" matters: it clears IF on entry, so the handler cannot be
            // re-entered by the same interrupt before it acknowledges.
            type_attr: 0x8e,
            offset_mid: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            reserved: 0,
        };
    }
}

/// Builds the IDT, remaps the PIC, unmasks the timer and enables interrupts.
pub fn init() {
    let handler = __pit_trampoline as *const () as usize as u64;
    // SAFETY: single-threaded boot path; nothing else touches the IDT, and
    // interrupts are still masked until the `sti` at the end.
    unsafe {
        let idt = &raw mut IDT;
        // Vectors 0-31 are the CPU's own exceptions. Without gates for them a
        // fault becomes a double fault becomes a triple fault, which on this
        // machine is a silent reset loop — the failure mode that tells you
        // nothing at all. One stub each, so the report can name the vector.
        let stubs = &raw const ISR_STUBS as usize;
        for vector in 0..32 {
            set_gate(idt, vector, (stubs + vector * STUB_STRIDE) as u64);
        }
        set_gate(idt, PIT_VECTOR, handler);
        let descriptor = Descriptor {
            limit: (core::mem::size_of_val(&*idt) - 1) as u16,
            base: idt as u64,
        };
        core::arch::asm!("lidt [{}]", in(reg) &descriptor, options(readonly, nostack, preserves_flags));

        // Remap the PIC. The initialisation sequence is four writes per chip,
        // in order, and the chip latches them as ICW1-ICW4.
        crate::port_out_u8(PIC1_CMD, 0x11); // ICW1: begin init, expect ICW4
        crate::port_out_u8(PIC2_CMD, 0x11);
        crate::port_out_u8(PIC1_DATA, PIT_VECTOR as u8); // ICW2: vector offsets
        crate::port_out_u8(PIC2_DATA, PIT_VECTOR as u8 + 8);
        crate::port_out_u8(PIC1_DATA, 0x04); // ICW3: slave on IRQ2
        crate::port_out_u8(PIC2_DATA, 0x02);
        crate::port_out_u8(PIC1_DATA, 0x01); // ICW4: 8086 mode
        crate::port_out_u8(PIC2_DATA, 0x01);
        // Mask everything but IRQ0. An unmasked line with no handler is a
        // vector into a zeroed IDT entry, which is a triple fault.
        crate::port_out_u8(PIC1_DATA, 0xfe);
        crate::port_out_u8(PIC2_DATA, 0xff);

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
    /// The interrupt handler, written in LK. `#[export("lk_timer_isr")]` in
    /// `program.lk` is what makes this name exist.
    fn lk_timer_isr();
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
    ".global __pit_trampoline",
    "__pit_trampoline:",
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
    "   call pit_dispatch",
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
    "   iretq",
);

/// Each stub is padded to this, so their addresses are computable rather than
/// needing 32 labels. A stub is at most nine bytes: two `push imm8` and a
/// `jmp rel32`.
const STUB_STRIDE: usize = 16;

unsafe extern "C" {
    /// The first of the 32 exception stubs.
    static ISR_STUBS: u8;
}

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
