//! Ring 3, and the one door back in.
//!
//! Everything else in this kernel runs at ring 0, where a wrong address is a
//! fault and a right one is whatever the hardware does. That is fine while all
//! the code is the kernel's own. It stops being fine the moment the kernel runs
//! a *program* — which it now does — because "the program cannot touch the
//! framebuffer" has so far been a matter of the program not trying.
//!
//! This makes it a property of the machine. A task entered through `iretq` with
//! ring-3 selectors cannot execute `in`/`out`, cannot write the kernel's pages,
//! and cannot reach any memory the page tables do not mark user-accessible. What
//! it *can* do is `int 0x80`, which is the whole interface: one vector, one
//! handler, and a number in a register saying what is wanted.
//!
//! Two things have to exist before that is possible, and both are the kind of
//! structure whose absence shows up as a triple fault rather than an error:
//!
//! - a **TSS**, because the CPU needs somewhere to put the stack pointer when an
//!   interrupt arrives while ring 3 is running. Without `rsp0` it pushes the
//!   interrupt frame onto the *user* stack, which the user can then rewrite.
//! - a **ring-3 code and data descriptor**, because privilege is a property of
//!   the segment the CPU is executing from.

use core::arch::global_asm;

/// The vector a user task asks the kernel through.
///
/// 0x80 by tradition, and past the PIC's remapped range so no device can raise
/// it. Its gate is the only one with DPL 3: every other vector is the kernel's,
/// and a ring-3 `int` at one of them is a general protection fault rather than a
/// way in.
pub const SYSCALL_VECTOR: usize = 0x80;

/// What a user task can ask for.
///
/// One number per call, in `rax`. Deliberately small: every entry here is a
/// hole in the wall the ring boundary just built, and the way to keep the wall
/// meaningful is to have few holes and know what each one lets through.
const SYS_WRITE: u64 = 1;
const SYS_EXIT: u64 = 2;

unsafe extern "C" {
    /// The console, which belongs to the LK program.
    fn lk_console_byte(byte: i64);
}

/// Set when a user task asks to exit, so the kernel knows the ring-3 excursion
/// finished rather than faulted.
static USER_EXITED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn user_exited() -> bool {
    USER_EXITED.load(core::sync::atomic::Ordering::Relaxed)
}

/// The syscall handler, called from the trampoline with the number in `rax` and
/// one argument in `rdi`.
///
/// Returning a value rather than writing registers: the trampoline puts the
/// answer back in `rax`, which keeps the register discipline in one place.
#[unsafe(no_mangle)]
pub extern "C" fn syscall_dispatch(number: u64, arg: u64) -> u64 {
    match number {
        // Deliberately a *byte*, not a pointer: a pointer from ring 3 is an
        // address the kernel would have to check before following, and there is
        // no page-level user/kernel split here yet to check it against. One
        // byte per call is slow and honest; a buffer would be fast and wrong.
        SYS_WRITE => {
            // SAFETY: an `#[export]`ed LK function taking one integer.
            unsafe { lk_console_byte((arg & 0xff) as i64) };
            0
        }
        SYS_EXIT => {
            USER_EXITED.store(true, core::sync::atomic::Ordering::Relaxed);
            0
        }
        // An unknown number is not a crash: a kernel that dies on a bad syscall
        // is one any program can take down.
        _ => u64::MAX,
    }
}

global_asm!(
    ".global __syscall_trampoline",
    "__syscall_trampoline:",
    // The interrupt already switched to the ring-0 stack from the TSS. What is
    // saved here is what the *caller* expects to keep: everything except the
    // return value.
    "   push rcx",
    "   push rdx",
    "   push rsi",
    "   push r8",
    "   push r9",
    "   push r10",
    "   push r11",
    "   mov rsi, rdi",
    "   mov rdi, rax",
    "   call syscall_dispatch",
    "   pop r11",
    "   pop r10",
    "   pop r9",
    "   pop r8",
    "   pop rsi",
    "   pop rdx",
    "   pop rcx",
    "   iretq",
);

/// The Task State Segment.
///
/// Almost all of it is dead weight in long mode — the register-save fields a
/// 32-bit TSS had are gone, and hardware task switching with them. What is left
/// that matters is `rsp0`: the stack the CPU switches to when an interrupt or a
/// syscall takes the machine from ring 3 back to ring 0.
#[repr(C, packed)]
struct TaskStateSegment {
    _reserved0: u32,
    rsp: [u64; 3],
    _reserved1: u64,
    ist: [u64; 7],
    _reserved2: u64,
    _reserved3: u16,
    io_map_base: u16,
}

static mut TSS: TaskStateSegment = TaskStateSegment {
    _reserved0: 0,
    rsp: [0; 3],
    _reserved1: 0,
    ist: [0; 7],
    _reserved2: 0,
    _reserved3: 0,
    // Past the end of the segment: an I/O permission bitmap that starts beyond
    // the TSS limit means "no ports are permitted", which is what ring 3 should
    // be able to do — nothing. Leaving this zero would point the CPU at the
    // start of the TSS and let a user task read the bitmap out of its own
    // fields, which is a permission map made of whatever happened to be there.
    io_map_base: core::mem::size_of::<TaskStateSegment>() as u16,
};

/// The kernel stack an interrupt from ring 3 lands on.
///
/// Its own stack, not the interrupted task's: a user task's stack pointer is a
/// value the user chose, and an interrupt that pushed onto it would be handing
/// the frame it is about to `iretq` from to the program it interrupted.
#[repr(align(16))]
struct KernelStack([u8; 16 * 1024]);
static mut RING0_STACK: KernelStack = KernelStack([0; 16 * 1024]);

unsafe extern "C" {
    static mut __gdt_tss: u64;
}

/// Fills in the TSS descriptor and loads it. Call once, before entering ring 3.
pub fn init() {
    // SAFETY: single-threaded boot path; nothing else touches the GDT or the
    // TSS while this runs.
    unsafe {
        let stack_top = (&raw mut RING0_STACK).cast::<u8>().add(size_of::<KernelStack>());
        let tss = &raw mut TSS;
        (*tss).rsp[0] = stack_top as u64;

        // A system descriptor in long mode is sixteen bytes: the familiar
        // 32-bit layout, plus the base's upper word in the half after it. The
        // fields are scattered across it for reasons that are purely
        // historical, which is why this is written out rather than computed.
        let base = tss as u64;
        let limit = (size_of::<TaskStateSegment>() - 1) as u64;
        let low = limit & 0xFFFF
            | (base & 0xFF_FFFF) << 16
            | 0x89u64 << 40                       // present, type 9 = available 64-bit TSS
            | ((limit >> 16) & 0xF) << 48
            | ((base >> 24) & 0xFF) << 56;
        let high = base >> 32;
        let slot = &raw mut __gdt_tss;
        slot.write(low);
        slot.add(1).write(high);

        core::arch::asm!("ltr {0:x}", in(reg) TSS_SELECTOR, options(nostack, preserves_flags));
    }
}

/// Points the CPU at the ring-0 stack it should switch to on the next
/// interrupt from ring 3.
///
/// Per task, not once: two user tasks sharing one kernel stack would have the
/// second one's interrupt frame land on top of the first one's, and the first
/// would resume into whatever was left. The scheduler calls this on every
/// switch, which is the only place that knows whose stack is next.
pub fn set_kernel_stack(top: u64) {
    // SAFETY: a plain word in a static the CPU reads only on a ring change,
    // which cannot happen while this runs (interrupts are masked inside the
    // gate this is called from).
    unsafe {
        (&raw mut TSS).cast::<u8>().add(4).cast::<u64>().write_unaligned(top);
    }
}

/// Where the TSS descriptor sits in the GDT.
const TSS_SELECTOR: u16 = 0x28;
/// Ring-3 code and data, with the requested privilege level in the low bits.
const USER_CODE_SELECTOR: u64 = 0x18 | 3;
const USER_DATA_SELECTOR: u64 = 0x20 | 3;

/// Enters ring 3 at `entry`, on `stack`, and does not come back.
///
/// `iretq` is the only way in: there is no instruction that lowers privilege
/// directly, so the kernel builds the frame an interrupt *would* have left
/// behind — as if ring 3 had been interrupted and is now being resumed — and
/// returns from an interrupt that never happened.
///
/// # Safety
///
/// `entry` must be code the user segment can reach and `stack` a mapped,
/// writable, 16-byte-aligned stack. Both are user-visible from here on.
pub unsafe fn enter(entry: u64, stack: u64) -> ! {
    // SAFETY: the caller's claim, plus a frame this function builds itself.
    unsafe {
        core::arch::asm!(
            // The data selectors first: they are loaded by `iretq` for SS, but
            // the others are the kernel's until something sets them.
            "mov ds, {data:x}",
            "mov es, {data:x}",
            "mov fs, {data:x}",
            "mov gs, {data:x}",
            // The frame `iretq` pops, pushed high to low: SS, RSP, RFLAGS, CS, RIP.
            "push {data}",
            "push {stack}",
            // Interrupts enabled (bit 9) and bit 1, which is always set. A ring-3
            // task with interrupts masked would own the machine.
            "push 0x202",
            "push {code}",
            "push {entry}",
            "iretq",
            data = in(reg) USER_DATA_SELECTOR,
            stack = in(reg) stack,
            code = in(reg) USER_CODE_SELECTOR,
            entry = in(reg) entry,
            options(noreturn),
        )
    }
}

/// A program that runs in ring 3.
///
/// Written in assembly rather than Rust for one reason: it must not touch
/// anything the compiler might reasonably reach for — no stack spills into
/// kernel pages, no calls into `lkrt`, no globals. What it does is the whole
/// point of the exercise: print through the syscall, then *try* to write the
/// framebuffer directly, which is the instruction the ring boundary has to
/// stop.
global_asm!(
    ".global __user_program",
    "__user_program:",
    // "USER" through the syscall, one byte per call.
    "   mov rax, 1",
    "   mov rdi, 85",       // 'U'
    "   int 0x80",
    "   mov rax, 1",
    "   mov rdi, 83",       // 'S'
    "   int 0x80",
    "   mov rax, 1",
    "   mov rdi, 69",       // 'E'
    "   int 0x80",
    "   mov rax, 1",
    "   mov rdi, 82",       // 'R'
    "   int 0x80",
    // Say the excursion finished *before* trying anything forbidden, so the
    // check can tell "ring 3 ran" from "ring 3 was stopped".
    "   mov rax, 2",
    "   int 0x80",
    // And now the forbidden thing: a write to the kernel's own data. At ring 3
    // this is a page fault, reported by the exception handler and never
    // executed. A kernel where this *worked* would have a ring boundary in
    // name only.
    "   mov rax, 0x300000",
    "   mov qword ptr [rax], 0",
    // Unreachable: the fault above does not return.
    "2:  jmp 2b",
);

/// A ring-3 task that never yields.
///
/// It prints a `3` through the syscall, spins for a while, and does it again,
/// for ever. Nothing in it cooperates: if the shell keeps answering while this
/// runs, the timer took the CPU away from ring 3 and gave it back — which is
/// the claim. The earlier `user` command could not show that, because it had
/// no way back at all.
global_asm!(
    ".global __user_task",
    "__user_task:",
    "3: mov rax, 1",
    "   mov rdi, 51",       // '3'
    "   int 0x80",
    "   mov rcx, 40000000",
    "4: dec rcx",
    "   jnz 4b",
    "   jmp 3b",
);

unsafe extern "C" {
    /// The ring-3 program's first instruction.
    pub fn __user_program();
    /// The preemptible ring-3 task's.
    pub fn __user_task();
}

/// A stack for the ring-3 task, distinct from the one-shot program's: they are
/// different tasks and must not share a stack.
#[repr(align(16))]
struct UserTaskStack([u8; 8 * 1024]);
static mut USER_TASK_STACK: UserTaskStack = UserTaskStack([0; 8 * 1024]);

/// The task's entry and stack, for the program to spawn it with.
#[unsafe(no_mangle)]
pub extern "C" fn lk_user_task_entry() -> i64 {
    __user_task as *const () as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_user_task_stack() -> i64 {
    // SAFETY: a static array's own end, one word down for the ABI's phase.
    unsafe {
        (&raw mut USER_TASK_STACK)
            .cast::<u8>()
            .add(size_of::<UserTaskStack>())
            .sub(8) as i64
    }
}

/// The stack the user program runs on.
#[repr(align(16))]
struct UserStack([u8; 8 * 1024]);
static mut USER_STACK: UserStack = UserStack([0; 8 * 1024]);

/// The top of that stack, one word down — the phase a function expects.
pub fn user_stack_top() -> u64 {
    // SAFETY: a static array's own end.
    unsafe { (&raw mut USER_STACK).cast::<u8>().add(size_of::<UserStack>()).sub(8) as u64 }
}

/// The frame a *user* task starts life on, for `tasks::prepare_stack`.
///
/// Same shape as a kernel task's — fifteen saved registers under the frame the
/// CPU itself pushes — with the selectors that make it ring 3. Which is the
/// whole difference between a user task and a kernel one: not what it runs, but
/// which four numbers are in that frame.
pub fn user_frame_selectors() -> (u64, u64) {
    (USER_CODE_SELECTOR, USER_DATA_SELECTOR)
}

/// Runs the ring-3 program, and does not come back.
///
/// Called from LK through `#[extern]`. There is no return: the only ways out of
/// ring 3 here are a syscall (which returns *into* ring 3) and a fault, and the
/// fault reporter halts. Making that explicit in the signature is what stops a
/// caller from writing code after it that would never run.
///
/// A second task would be the way to make this survivable — enter ring 3 on its
/// own stack, and let the timer take the CPU back. That needs the scheduler to
/// know about privilege, which it does not yet.
#[unsafe(no_mangle)]
pub extern "C" fn lk_enter_user() -> ! {
    // SAFETY: `__user_program` is code in this image, and the stack is a static
    // array of its own.
    unsafe { enter(__user_program as *const () as u64, user_stack_top()) }
}
