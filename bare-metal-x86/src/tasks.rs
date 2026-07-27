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

/// How many tasks the board runs. Fixed, because nothing here can grow a table
/// while interrupts are using it.
pub const TASK_COUNT: usize = 2;

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
static mut TASK_STACKS: [Stack; TASK_COUNT - 1] = [const { Stack([0; STACK_SIZE]) }; TASK_COUNT - 1];

/// Each task's saved stack pointer, valid while it is *not* running.
static mut TASK_RSP: [u64; TASK_COUNT] = [0; TASK_COUNT];

/// Which entry of `TASK_RSP` belongs to the task currently on the CPU.
static mut CURRENT: usize = 0;

unsafe extern "C" {
    /// The task bodies and the scheduler, written in LK.
    fn lk_task_b();
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
unsafe fn prepare_stack(top: *mut u8, entry: unsafe extern "C" fn()) -> u64 {
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
    push(entry as usize as u64); // RIP
    // The saved registers, in the order `IRQ_RESTORE` pops them — that is,
    // the reverse of the order `IRQ_SAVE` pushes.
    for _ in 0..15 {
        push(0);
    }
    sp
}

/// Prepares every task past the first. Call once, before interrupts.
pub fn init() {
    for index in 0..TASK_COUNT - 1 {
        // SAFETY: single-threaded boot path, before any interrupt can run.
        let top = unsafe {
            let stacks = &raw mut TASK_STACKS;
            (*stacks)[index].0.as_mut_ptr().add(STACK_SIZE)
        };
        // SAFETY: `top` is the high end of a stack nothing else uses.
        let sp = unsafe { prepare_stack(top, lk_task_b) };
        // SAFETY: as above.
        unsafe {
            let rsp = &raw mut TASK_RSP;
            (*rsp)[index + 1] = sp;
        }
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
        let next = if next < TASK_COUNT { next } else { current };
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
    ".global __task_trampoline",
    "__task_trampoline:",
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
    // The handler's own work (the LK tick, the end-of-interrupt) happens
    // before the switch, on the interrupted task's stack.
    "   call pit_dispatch",
    "   mov rdi, rsp",
    "   call schedule_from_interrupt",
    "   mov rsp, rax",
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
    "   iretq",
);
