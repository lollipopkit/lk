//! Preemptive task switching.
//!
//! The timer interrupt already spills every register a called function may
//! clobber. Switching *tasks* needs more: the registers a function is entitled
//! to keep (rbx, rbp, r12-r15) belong to whoever was interrupted, so they have
//! to travel with that task rather than with the handler. Once all of them are
//! on the interrupted task's stack, a switch is one instruction — point RSP at
//! another task's stack and let the same restore sequence run.
//!
//! What is left here is the one thing a language cannot say: *return on a
//! different stack*. Everything else has moved to `program.lk` and
//! `drivers/tasks.lk` — the table, the frame a task starts life on, which slot
//! runs next, whose address space, whose kernel stack. This file supplies the
//! register spill either side of that decision, and the software interrupt a
//! task uses to ask for it.
//!
//! A task may not allocate. `lkrt` has one arena and no locks around it, so two
//! tasks in it at once would corrupt it; the same rule the interrupt handlers
//! already follow, for the same reason.

use core::arch::global_asm;

/// Gives up the rest of this task's slice.
///
/// A software interrupt rather than a direct call: the switch has to happen
/// with a complete interrupt frame on the stack, because that is what the
/// resume path expects to find. `int` builds one; a call does not.
///
/// This is what an `#[extern]` declaration in `program.lk` names — the LK
/// program asks the board for something the board alone can do.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_yield() {
    // The number is a literal here and named `VECTOR_YIELD` in `program.lk`,
    // which is the file that installs its gate. Two spellings of one number,
    // and this is the side that cannot avoid it: `int` takes its vector as an
    // immediate, so there is no operand to pass one in through.
    //
    // SAFETY: the vector has a gate — `program.lk` installs it before it asks
    // the board to enable interrupts, which is the only order that works.
    unsafe { core::arch::asm!("int 0x30", options(nomem, nostack)) };
}

// The timer's trampoline, extended to switch tasks.
//
// Every register is saved, not just the caller-saved ones: what is on this
// stack has to be a *whole* task, because the stack this returns on may not be
// the one it arrived on.
/// How many eight-byte words `SAVE_TASK` leaves on the stack.
///
/// Fifteen integer registers and sixteen XMM registers of sixteen bytes each.
/// Asked of the board rather than counted again in LK, because the macro below
/// is the thing that decides it: a register added there has to appear in the
/// frame a task *starts* on too, and two numbers in two languages are two
/// places to add it. A frame short by one word is not an error anything
/// reports — it is a resume that reads its RIP out of whatever the next slot
/// held.
#[unsafe(no_mangle)]
pub extern "C" fn lk_task_saved_words() -> i64 {
    15 + 256 / 8
}

global_asm!(
    ".section .text, \"ax\"",
    // A whole task's registers, not just the caller-saved ones: what is on
    // this stack may be resumed on a different one.
    ".macro SAVE_TASK",
    "   push rax",
    "   push rcx",
    "   push rdx",
    "   push rsi",
    "   push rdi",
    "   push r8",
    "   push r9",
    "   push r10",
    "   push r11",
    "   push rbx",
    "   push rbp",
    "   push r12",
    "   push r13",
    "   push r14",
    "   push r15",
    // And the SSE registers, which this used to leave to whoever was
    // interrupted. Every other interrupt path here saves them, for the reason
    // written beside those: a compiled LK handler may clobber any XMM register
    // under the System V ABI, LK numbers are `f64`, and the interrupted
    // computation may hold one. This path calls two of them a thousand times a
    // second — and it is also the one that switches tasks, so without this a
    // task's SSE state is not part of what travels with it.
    //
    // 256 rather than the device path's 264, and the difference is the whole
    // alignment question. A multiple of sixteen *preserves* whatever alignment
    // the pushes above produced, and that alignment already works: this path
    // calls compiled LK today. The device path adds the extra eight because its
    // nine pushes leave it needing them; deriving either number from first
    // principles is not required, and trying to was what made this look harder
    // than it is.
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
    ".endm",
    ".macro RESTORE_TASK",
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
    "   pop r15",
    "   pop r14",
    "   pop r13",
    "   pop r12",
    "   pop rbp",
    "   pop rbx",
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
    // The yield path is the timer path without the device work: nothing
    // arrived, so there is nothing to acknowledge and no tick to count.
    ".global __yield_trampoline",
    "__yield_trampoline:",
    "   SAVE_TASK",
    "   mov rdi, rsp",
    "   call lk_schedule_from_interrupt",
    "   mov rsp, rax",
    "   RESTORE_TASK",
    "   iretq",
    ".global __task_trampoline",
    "__task_trampoline:",
    "   SAVE_TASK",
    // The handler's own work — the LK tick, and the end-of-interrupt it sends
    // itself — happens before the switch, on the interrupted task's stack.
    // Called directly rather than through a forwarding function on this side:
    // there is nothing left for one to add now that acknowledging the chip is
    // the driver's.
    "   call lk_timer_isr",
    "   mov rdi, rsp",
    "   call lk_schedule_from_interrupt",
    "   mov rsp, rax",
    "   RESTORE_TASK",
    "   iretq",
);
