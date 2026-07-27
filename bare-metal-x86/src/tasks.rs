//! Preemptive task switching.
//!
//! The timer interrupt already spills every register a called function may
//! clobber. Switching *tasks* needs more: the registers a function is entitled
//! to keep (rbx, rbp, r12-r15) belong to whoever was interrupted, so they have
//! to travel with that task rather than with the handler. Once all of them are
//! on the interrupted task's stack, a switch is one instruction — point RSP at
//! another task's stack and let the same restore sequence run.
//!
//! What lives here is the mechanics: stacks, the frame a task starts life
//! with, and the register bookkeeping. *Which* task runs next is
//! `lk_schedule`, an `#[export]`ed LK function — policy is the program's.
//!
//! A task may not allocate. `lkrt` has one arena and no locks around it, so two
//! tasks in it at once would corrupt it; the same rule the interrupt handlers
//! already follow, for the same reason.

use core::arch::global_asm;

/// How many tasks the board can hold. A capacity, not a count: the stacks are
/// static because nothing here can grow a table while interrupts are reading
/// it, but which of them are in use is decided at run time by `lk_spawn`.
pub const TASK_CAPACITY: usize = 4;

const STACK_SIZE: usize = 32 * 1024;

/// A task stack.
///
/// The alignment is not decoration: compiled LK code spills SSE registers with
/// `movaps`, which faults on a stack that is not 16-byte aligned. A plain
/// `[u8; N]` has alignment 1, and the fault it produces is a #GP inside the
/// task, nowhere near the array.
#[repr(align(16))]
struct Stack([u8; STACK_SIZE]);

/// One stack per task past the first. Task 0 keeps the stack the boot path
/// gave it — it is the one already running when the first interrupt lands.
static mut TASK_STACKS: [Stack; TASK_CAPACITY - 1] = [const { Stack([0; STACK_SIZE]) }; TASK_CAPACITY - 1];

/// Each task's saved stack pointer, valid while it is *not* running.
static mut TASK_RSP: [u64; TASK_CAPACITY] = [0; TASK_CAPACITY];

/// How many slots are in use. One at boot: the task already running, whose
/// stack the boot path gave it.
static mut TASK_USED: usize = 1;

/// Which entry of `TASK_RSP` belongs to the task currently on the CPU.
static mut CURRENT: usize = 0;

unsafe extern "C" {
    /// The scheduler, written in LK. The task *bodies* are no longer named
    /// here: a program spawns them by address, so the board does not have to
    /// know what they are called.
    fn lk_schedule(current: i64) -> i64;
}

/// Builds the stack a task starts life on.
///
/// It is the exact picture the interrupt path leaves behind, because that is
/// what the restore sequence will read: fifteen saved registers, then the
/// frame the CPU itself pushes. Getting the order wrong here is not a compile
/// error — it is a jump to whatever the wrong slot held.
///
/// # Safety
///
/// `top` must be the high end of a writable, 16-byte-aligned stack that
/// nothing else uses.
unsafe fn prepare_stack(top: *mut u8, entry: u64) -> u64 {
    // The CPU's frame, pushed high to low: SS, RSP, RFLAGS, CS, RIP.
    let mut sp = top as u64;
    let mut push = |value: u64| {
        sp -= 8;
        // SAFETY: within the caller's stack, which is ours to write.
        unsafe { core::ptr::write_volatile(sp as *mut u64, value) };
    };
    push(0x10); // SS — the boot GDT's data selector
    // One word below the top, so the task begins with the stack in the phase a
    // function expects: the ABI assumes a `call` has just pushed a return
    // address, and `iretq` pushes nothing. Without the offset the first
    // aligned SSE spill in the task faults, inside whatever it called.
    push(top as u64 - 8); // RSP the task resumes with
    push(0x202); // RFLAGS: interrupts enabled, bit 1 always set
    push(0x08); // CS — the boot GDT's 64-bit code selector
    push(entry); // RIP
    // The saved registers, in the order `IRQ_RESTORE` pops them — that is,
    // the reverse of the order `IRQ_SAVE` pushes.
    for _ in 0..15 {
        push(0);
    }
    sp
}

/// The vector a task uses to ask for a reschedule.
///
/// Past the PIC's remapped range, so it can only arrive from an `int`
/// instruction — there is no device behind it, and nothing to acknowledge.
pub const YIELD_VECTOR: usize = 0x30;

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
    // SAFETY: the vector has a gate, installed before interrupts were enabled.
    unsafe { core::arch::asm!("int 0x30", options(nomem, nostack)) };
}

/// Starts a task at `entry`, on a stack of its own. Returns its slot, or -1.
///
/// The address comes from `symbol_address` on the LK side, which is what makes
/// this a *table* rather than a list the board has to know the names in: the
/// program decides what runs, the board only supplies stacks and the switch.
///
/// The caller must have interrupts masked. The scheduler reads `TASK_USED` from
/// an interrupt, so publishing a slot before its stack is prepared would let a
/// timer tick resume a task that does not exist yet — which is a jump to zero.
///
/// # Safety
///
/// Called from LK with a code address; a value that is not one is a jump to
/// wherever it points. That is what `unsafe` in the LK source is claiming.
#[unsafe(no_mangle)]
pub extern "C" fn lk_spawn(entry: i64) -> i64 {
    if entry == 0 {
        return -1;
    }
    // SAFETY: the caller holds interrupts masked, so nothing else is reading
    // or writing these while this runs.
    unsafe {
        let used = *(&raw const TASK_USED);
        if used >= TASK_CAPACITY {
            return -1;
        }
        let stacks = &raw mut TASK_STACKS;
        let top = (*stacks)[used - 1].0.as_mut_ptr().add(STACK_SIZE);
        let sp = prepare_stack(top, entry as u64);
        let rsp = &raw mut TASK_RSP;
        (*rsp)[used] = sp;
        // Published last: the stack has to be complete before the scheduler
        // can pick the slot.
        *(&raw mut TASK_USED) = used + 1;
        used as i64
    }
}

/// Called from the trampoline with every register already on the interrupted
/// task's stack. Returns the stack to resume on.
///
/// # Safety
///
/// `rsp` must be the interrupted task's stack pointer, with a complete saved
/// frame at it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn schedule_from_interrupt(rsp: u64) -> u64 {
    // SAFETY: interrupts are masked inside an interrupt gate, so nothing else
    // is touching these while this runs.
    unsafe {
        let current = *(&raw const CURRENT);
        let table = &raw mut TASK_RSP;
        (*table)[current] = rsp;
        let next = lk_schedule(current as i64) as usize;
        // Clamped against what is *spawned*, not against the capacity: a
        // scheduler that names an empty slot would resume a stack that was
        // never prepared.
        let next = if next < *(&raw const TASK_USED) { next } else { current };
        *(&raw mut CURRENT) = next;
        (*table)[next]
    }
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
    "   call schedule_from_interrupt",
    "   mov rsp, rax",
    "   RESTORE_TASK",
    "   iretq",
    ".global __task_trampoline",
    "__task_trampoline:",
    "   SAVE_TASK",
    // The handler's own work (the LK tick, the end-of-interrupt) happens
    // before the switch, on the interrupted task's stack.
    "   call pit_dispatch",
    "   mov rdi, rsp",
    "   call schedule_from_interrupt",
    "   mov rsp, rax",
    "   RESTORE_TASK",
    "   iretq",
);
