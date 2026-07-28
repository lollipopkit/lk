//! Interrupt entry, once, for any vector.
//!
//! An interrupt is not a call. The code it lands in never agreed to lose its
//! caller-saved registers, so a compiled handler cannot be the thing a gate
//! points at — something has to spill them first and leave with `iretq`. That
//! something is assembly in every language, which is why it is here rather than
//! in LK.
//!
//! What *was* per-kernel is that every vector needed its own hand-written stub
//! in the board's Rust: adding a device meant editing a file the driver has
//! nothing to do with. So this is 256 stubs and a table. A kernel points a gate
//! at `lkrt_isr_stubs + vector * stride` and writes its handler's address into
//! `lkrt_isr_handlers[vector]`, and both of those are things LK can say —
//! `symbol_address` and a volatile store.
//!
//! What is deliberately *not* here: the two shapes that are not ordinary device
//! interrupts. A timer that switches tasks has to return on a *different* stack,
//! and a syscall has to return a value in `rax`; both need their own tail, and
//! both belong to the kernel that defines them.

/// Where each vector's handler is, or 0.
///
/// Written by the kernel, read by the tail below. A `u64` per vector rather
/// than a function pointer type, because what a kernel installs here is the
/// result of `symbol_address` — an integer, on the LK side.
///
/// Zero means nothing is installed, and the tail checks: a vector that arrives
/// with no handler is a spurious interrupt, and answering it with a call to
/// address zero turns a diagnosable event into a fault inside a fault.
#[cfg(all(not(feature = "std"), target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub static mut lkrt_isr_handlers: [u64; 256] = [0; 256];

// The stubs, and the tail they share.
//
// Each stub exists only to say which vector it is — the CPU does not tell the
// handler — and then join the common path. `push imm32` rather than `push imm8`
// because the immediate is sign-extended: vector 200 pushed as a byte arrives
// as -56, which is the kind of mistake that shows up only on the vectors nobody
// tested.
//
// The register discipline is the interesting part. Every caller-saved integer
// register, and all sixteen XMM registers, because a compiled LK handler may
// use any of them and LK numbers are `f64`. Missing one corrupts a value in the
// interrupted program rather than crashing, at a moment nothing can predict.
//
// Alignment: the CPU aligns RSP to 16 before pushing its own frame, and the
// stub's vector push plus nine register pushes plus 256 bytes of XMM area come
// to 336 — the same total the hand-written trampolines reach with nine pushes
// and 264. `call` gets the alignment it expects because the arithmetic works
// out, not because it happens to.
#[cfg(all(not(feature = "std"), target_arch = "x86_64"))]
core::arch::global_asm!(
    ".section .text, \"ax\"",
    ".align 16",
    ".globl lkrt_isr_stubs",
    "lkrt_isr_stubs:",
    ".set vector, 0",
    ".rept 256",
    "   .align 16",
    "   .byte 0x68", // push imm32
    "   .long vector",
    "   jmp __lkrt_isr_common",
    "   .set vector, vector + 1",
    ".endr",
    // Past the padding of the last stub, not past its last instruction: a
    // caller divides `end - start` by 256 to get the stride, and without this
    // the array ends ten bytes into its final slot.
    ".align 16",
    ".globl lkrt_isr_stubs_end",
    "lkrt_isr_stubs_end:",
    "__lkrt_isr_common:",
    "   push rax",
    "   push rcx",
    "   push rdx",
    "   push rsi",
    "   push rdi",
    "   push r8",
    "   push r9",
    "   push r10",
    "   push r11",
    "   sub rsp, 256",
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
    // The vector the stub pushed, under the XMM area and the nine registers.
    "   mov rdi, [rsp + 328]",
    "   lea rax, [rip + lkrt_isr_handlers]",
    "   mov rax, [rax + rdi * 8]",
    "   test rax, rax",
    "   jz 2f",
    // The handler is called with its own vector, so one LK function can serve
    // several gates and still know which one arrived.
    "   call rax",
    "2:",
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
    "   add rsp, 256",
    "   pop r11",
    "   pop r10",
    "   pop r9",
    "   pop r8",
    "   pop rdi",
    "   pop rsi",
    "   pop rdx",
    "   pop rcx",
    "   pop rax",
    // The vector the stub pushed.
    "   add rsp, 8",
    "   iretq",
);

// ---------------------------------------------------------- raising, not taking
//
// The other direction, and the same obstacle. `int` takes its vector as an
// *immediate*: there is no operand to pass one in through, so a kernel that
// wanted to raise vector `n` for a computed `n` could not say so at all. That is
// why the board's `kernel_yield` was a Rust function containing `int 0x30`, and
// why the vector was written down twice — once where the gate is installed and
// once where it is raised, in two languages, with nothing checking they agree.
//
// Two hundred and fifty-six stubs answer it the same way the entry side does. A
// stub is three bytes; the padding is what makes the stride derivable, and the
// caller divides `end - start` by 256 rather than being told.

/// One stub per vector: `int n` and return.
///
/// `.byte 0xcd` then the vector, rather than `int $n`, because the assembler
/// will happily encode `int 3` as the one-byte breakpoint `0xcc` — a different
/// instruction, on the one vector a debugger is most likely to be watching.
/// Writing the opcode out means all 256 slots are the same two instructions.
#[cfg(all(not(feature = "std"), target_arch = "x86_64"))]
core::arch::global_asm!(
    ".section .text, \"ax\"",
    ".align 8",
    ".globl lkrt_int_stubs",
    "lkrt_int_stubs:",
    ".set intvec, 0",
    ".rept 256",
    "   .align 8",
    "   .byte 0xcd",
    "   .byte intvec",
    "   ret",
    "   .set intvec, intvec + 1",
    ".endr",
    // Past the padding of the last stub, not past its last instruction: a caller
    // divides `end - start` by 256 to get the stride, and without this the array
    // ends three bytes into its final slot.
    ".align 8",
    ".globl lkrt_int_stubs_end",
    "lkrt_int_stubs_end:",
);

/// Raises `vector`, whatever it is.
///
/// The call lands in the stub, the stub raises the interrupt, and the handler's
/// `iretq` comes back to the `ret` — so from the caller this is an ordinary
/// function call that happens to have run a gate in the middle. A gate that
/// switches stacks (a task switch) simply does not come back until the caller is
/// resumed, at which point its frame is exactly as it left it.
///
/// # Safety
/// The vector must have a gate installed. Raising one that does not is a general
/// protection fault, which is the same thing that happens when a device does it.
#[cfg(all(not(feature = "std"), target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_cpu_raise_interrupt(vector: i64) {
    unsafe extern "C" {
        static lkrt_int_stubs: u8;
        static lkrt_int_stubs_end: u8;
    }
    if !(0..256).contains(&vector) {
        return;
    }
    let start = &raw const lkrt_int_stubs as usize;
    let stride = ((&raw const lkrt_int_stubs_end as usize) - start) / 256;
    let target = start + vector as usize * stride;
    // SAFETY: `target` is inside the stub array, which is `int`+`ret` and takes
    // no arguments.
    let stub: extern "C" fn() = unsafe { core::mem::transmute(target) };
    stub();
}

/// Anywhere else, this is refused rather than ignored.
///
/// Refused, and that is the point: under a process there is no interrupt table,
/// and a runtime that went ahead and raised a real `int 0x80` would be making a
/// Linux system call with whatever happened to be in the registers. Doing
/// nothing would be worse than either — it is the answer that lets a program
/// look like it worked, and it would put the two backends into disagreement,
/// since the interpreter refuses.
#[cfg(not(all(not(feature = "std"), target_arch = "x86_64")))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_cpu_raise_interrupt(_vector: i64) {
    crate::panic::raise_str(
        "cpu_raise_interrupt requires bare-metal execution on x86-64: no other target has this instruction",
    );
}
