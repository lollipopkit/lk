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
global_asm!(
    ".section .text, \"ax\"",
    // A whole task's registers, not just the caller-saved ones: what is on
    // this stack may be resumed on a different one.
    // NOTE: this saves fifteen integer registers and *no* SSE state, and it is
    // the only interrupt path here that does not.
    //
    // Every other one — the keyboard's and the mouse's before they moved into
    // `lkrt`, `lkrt`'s generic stubs, the syscall trampoline — saves all
    // sixteen XMM registers, for the reason written next to them: a compiled LK
    // handler may clobber any of them under the System V ABI, LK numbers are
    // `f64`, and the interrupted computation may hold one. This path calls two
    // compiled LK functions (`lk_timer_isr` and `lk_schedule_from_interrupt`)
    // with none of that saved, a thousand times a second, and it is also the
    // one that switches tasks — so a task's SSE state is not part of what
    // travels with it either.
    //
    // Nothing has gone wrong yet because today's tick and scheduler do integer
    // work only. That is a property of the handlers, not of the boundary, and
    // it is the same shape as the syscall trampoline before its XMM save was
    // added.
    //
    // TODO: save them here too. Not done in the same breath as noticing it,
    // because the stack alignment has to be *measured* rather than derived: the
    // arithmetic that explains why the device path's `sub rsp, 264` lands
    // aligned does not also explain why this path's fifteen pushes do, and a
    // `call` into compiled LK on a misaligned stack faults on the first
    // `movaps` rather than saying anything. Boot it, break in the handler, and
    // read RSP.
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
    ".endm",
    ".macro RESTORE_TASK",
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
