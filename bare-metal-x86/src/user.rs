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

// The syscall vector is 0x80, spelled as an immediate in the ring-3 programs
// below and named `VECTOR_SYSCALL` in `program.lk`, which installs its gate —
// the only gate with DPL 3, which is what makes it the one vector ring 3 can
// raise and every other one a general protection fault.

/// What a user task can ask for.
///
/// One number per call, in `rax`. Deliberately small: every entry here is a
/// hole in the wall the ring boundary just built, and the way to keep the wall
/// meaningful is to have few holes and know what each one lets through.
const SYS_WRITE: u64 = 1;
const SYS_EXIT: u64 = 2;
/// Write a run of bytes: `rdi` is an address in the caller's memory, `rsi` its
/// length. The first call that takes a *pointer*, and therefore the first one
/// that has to decide whether to believe it.
const SYS_WRITE_STR: u64 = 3;

/// How long a string the kernel will accept in one call.
///
/// A bound, not a guess: without one, a user task can hand over a length that
/// keeps the kernel inside the syscall for as long as it likes — with
/// interrupts on, so the machine survives, but the caller's own timer slice
/// is spent in kernel code where nothing can preempt the loop's *effects*.
const MAX_WRITE: u64 = 4096;

unsafe extern "C" {
    /// The console, which belongs to the LK program.
    fn lk_console_byte(byte: i64);
    /// The bounds of everything ring 3 may reach, from the linker script.
    static __user_start: u8;
    static __user_end: u8;
}

/// Is `[address, address + length)` memory the caller is allowed to hand over?
///
/// The only region ring 3 can reach is its own section, so that is the whole
/// test — and it is a test the kernel performs rather than a promise the caller
/// makes. Without it, `write(0x100010, 64)` would have the kernel read its own
/// code and print it, which is the shape of every "the kernel followed a
/// pointer it was given" bug there has ever been.
///
/// The arithmetic is checked too: a length near `u64::MAX` would wrap the end
/// back below the start and make any address look contained.
fn user_range_is_valid(address: u64, length: u64) -> bool {
    if length == 0 || length > MAX_WRITE {
        return false;
    }
    let Some(end) = address.checked_add(length) else {
        return false;
    };
    let start = (&raw const __user_start) as u64;
    let limit = (&raw const __user_end) as u64;
    address >= start && end <= limit
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
pub extern "C" fn syscall_dispatch(number: u64, arg: u64, arg2: u64) -> u64 {
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
        // The kernel checks, copies, and only then uses. Printing straight out
        // of user memory would be one instruction shorter and would leave the
        // window where the caller can change the bytes between the check and
        // the use — which on a machine with more than one CPU is not a window
        // but a race.
        SYS_WRITE_STR => {
            if !user_range_is_valid(arg, arg2) {
                return u64::MAX;
            }
            for offset in 0..arg2 {
                // SAFETY: the range was just checked to lie inside the user
                // section, which is mapped and present.
                let byte = unsafe { (arg as *const u8).add(offset as usize).read_volatile() };
                // SAFETY: an `#[export]`ed LK function taking one integer.
                unsafe { lk_console_byte(i64::from(byte)) };
            }
            arg2
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
    // The ABI: number in `rax`, arguments in `rdi` and `rsi`, which the C call
    // wants in `rdi`, `rsi` and `rdx`. One shuffle, in one place.
    "   mov rdx, rsi",
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

/// The kernel stack an interrupt from ring 3 lands on.
///
/// Its own stack, not the interrupted task's: a user task's stack pointer is a
/// value the user chose, and an interrupt that pushed onto it would be handing
/// the frame it is about to `iretq` from to the program it interrupted.
#[repr(align(16))]
struct KernelStack([u8; 16 * 1024]);
static mut RING0_STACK: KernelStack = KernelStack([0; 16 * 1024]);

unsafe extern "C" {
    /// The kernel's page directories, placed by the linker script. The user
    /// address spaces that used to sit beside them are gone: `program.lk`
    /// allocates its page tables from its own page allocator now, so how many
    /// address spaces there can be is bounded by memory rather than by four
    /// reservations in a linker script.
    static __pd: u64;
}

/// Where a user task's stack lives *in its own address space*.
///
/// A virtual address the kernel's space does not map to the same thing: in the
/// kernel's tables 0x4000_0000 is identity-mapped RAM that nothing uses, and in
/// the user's it is the stack. That difference is what makes them two address
/// spaces rather than one with extra permissions.
pub const USER_STACK_VIRTUAL: u64 = 0x4000_0000;

/// Where the kernel's four page directories are, one per gigabyte.
///
/// Answered rather than reached into, because they are the *board's*: the
/// 32-bit boot code fills them in before long mode, which is before any of the
/// program exists. A user address space points at them instead of copying them
/// — the kernel has to be mapped in every space, since an interrupt during ring
/// 3 lands in kernel code, and a copy would work today and drift on the day a
/// mapping is added to one and not the other.
#[unsafe(no_mangle)]
pub extern "C" fn lk_kernel_page_directories() -> i64 {
    // SAFETY: a linker-placed array of page directories; taking its address
    // reads nothing.
    unsafe { (&raw const __pd) as i64 }
}

/// The ring-0 stack the CPU switches to on the *first* interrupt from ring 3.
///
/// The board's, and only this one: it has to exist before there is an allocator
/// to ask, so it is a static in the image. `program.lk` reads it once while
/// building the TSS, and every switch after that replaces it with the kernel
/// stack of whichever task is next — which is the scheduler's business, not
/// this one's.
#[unsafe(no_mangle)]
pub extern "C" fn lk_boot_kernel_stack() -> i64 {
    // SAFETY: a static array's own end.
    unsafe { (&raw mut RING0_STACK).cast::<u8>().add(size_of::<KernelStack>()) as i64 }
}

unsafe extern "C" {
    /// The ring-3 selectors, asked of the program rather than named here.
    ///
    /// `program.lk` builds the descriptor table these index, so it is the one
    /// place that knows what sits at 0x18 and 0x20. A copy on this side would
    /// be a second answer to a question with one, and two that disagreed would
    /// mean an `iretq` into a segment other than the one intended — which, if
    /// it happened to be a ring-0 descriptor, is no ring boundary at all.
    fn lk_user_code_selector() -> i64;
    fn lk_user_data_selector() -> i64;
    /// The TSS's `rsp0`, which the program owns. Called on every task switch.
    pub(crate) fn lk_set_kernel_stack(top: i64);
}

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
    let (code, data) = user_frame_selectors();
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
            data = in(reg) data,
            stack = in(reg) stack,
            code = in(reg) code,
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
    ".section .user, \"ax\"",
    ".global __user_program",
    "__user_program:",
    // "USER" through the syscall, one byte per call.
    "   mov rax, 1",
    "   mov rdi, 85", // 'U'
    "   int 0x80",
    "   mov rax, 1",
    "   mov rdi, 83", // 'S'
    "   int 0x80",
    "   mov rax, 1",
    "   mov rdi, 69", // 'E'
    "   int 0x80",
    "   mov rax, 1",
    "   mov rdi, 82", // 'R'
    "   int 0x80",
    // A string, through the call that takes a pointer. The kernel checks the
    // range before following it — this one is inside the user section, so it
    // prints.
    "   mov rax, 3",
    "   lea rdi, [rip + __user_message]",
    "   mov rsi, 3",
    "   int 0x80",
    // And the same call with a *kernel* pointer. The kernel must answer with an
    // error rather than printing its own memory: a pointer from ring 3 is a
    // number, and believing it is how a kernel reads out its own secrets on
    // request. The reply lands in `rax`, which the program then hands back
    // through the byte-at-a-time call so the check can see it: 'N' for
    // refused, 'Y' for followed.
    "   mov rax, 3",
    "   mov rdi, 0x100010",
    "   mov rsi, 8",
    "   int 0x80",
    "   cmp rax, -1",
    "   je 5f",
    "   mov rdi, 89", // 'Y' — the kernel followed it
    "   jmp 6f",
    "5: mov rdi, 78", // 'N' — refused
    "6: mov rax, 1",
    "   int 0x80",
    // Say the excursion finished *before* trying anything forbidden, so the
    // check can tell "ring 3 ran" from "ring 3 was stopped".
    "   mov rax, 2",
    "   int 0x80",
    // And now the forbidden thing — a *read* of the kernel's own code, at an
    // address inside the same 2 MiB as this program.
    //
    // Deliberately a read, and deliberately near: while the first 2 MiB was one
    // user-accessible page, this succeeded and told nobody. With 4 KiB pages it
    // faults, which is the difference between "ring 3 cannot reach the shared
    // page two megabytes away" and "ring 3 cannot reach the kernel".
    "   mov rax, 0x100010",
    "   mov rax, qword ptr [rax]",
    // Unreachable: the fault above does not return.
    "2:  jmp 2b",
    "__user_message:",
    "   .ascii \"str\"",
);

/// A ring-3 task that never yields.
///
/// It prints a `3` through the syscall, spins for a while, and does it again,
/// for ever. Nothing in it cooperates: if the shell keeps answering while this
/// runs, the timer took the CPU away from ring 3 and gave it back — which is
/// the claim. The earlier `user` command could not show that, because it had
/// no way back at all.
global_asm!(
    ".section .user, \"ax\"",
    // Two tasks, one body, one difference: the letter each writes into its own
    // stack. Both stacks are at the same *virtual* address, so if they shared
    // an address space the second write would land on the first's and both
    // would print the same letter for ever after. They print A and B.
    ".global __user_task_a",
    "__user_task_a:",
    "   mov rbx, 65", // 'A'
    "   jmp __user_task_body",
    ".global __user_task_b",
    "__user_task_b:",
    "   mov rbx, 66", // 'B'
    "__user_task_body:",
    // Write it into this task's own stack page, then read it back from there
    // every time round: a task that prints its letter is one whose memory
    // still says what it wrote.
    "   mov [rsp - 16], bl",
    "3: mov rax, 1",
    "   movzx rdi, byte ptr [rsp - 16]",
    "   int 0x80",
    "   mov rcx, 40000000",
    "4: dec rcx",
    "   jnz 4b",
    "   jmp 3b",
);

unsafe extern "C" {
    /// The ring-3 program's first instruction.
    pub fn __user_program();
    /// The two preemptible ring-3 tasks'.
    pub fn __user_task_a();
    pub fn __user_task_b();
}

/// A stack for the ring-3 task, distinct from the one-shot program's: they are
/// different tasks and must not share a stack.
#[repr(align(4096))]
struct UserTaskStack([u8; 8 * 1024]);
#[unsafe(link_section = ".user")]
static mut USER_TASK_STACK: UserTaskStack = UserTaskStack([0; 8 * 1024]);
#[unsafe(link_section = ".user")]
static mut USER_TASK_STACK_B: UserTaskStack = UserTaskStack([0; 8 * 1024]);

/// The task's entry and stack, for the program to spawn it with.
#[unsafe(no_mangle)]
pub extern "C" fn lk_user_task_entry() -> i64 {
    __user_task_a as *const () as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_user_task_entry_b() -> i64 {
    __user_task_b as *const () as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_user_task_stack_b() -> i64 {
    // SAFETY: a static array's own end.
    unsafe {
        (&raw mut USER_TASK_STACK_B)
            .cast::<u8>()
            .add(size_of::<UserTaskStack>())
            .sub(4096) as i64
    }
}

/// The physical page that backs the ring-3 task's stack.
///
/// One page, at the *end* of the static array so the stack grows down inside
/// it. What the task sees is `USER_STACK_VIRTUAL`, which is where this page is
/// mapped in the task's own space — the two numbers are the same memory and
/// different addresses, which is the whole point of the exercise.
#[unsafe(no_mangle)]
pub extern "C" fn lk_user_task_stack() -> i64 {
    // SAFETY: a static array's own end.
    unsafe {
        (&raw mut USER_TASK_STACK)
            .cast::<u8>()
            .add(size_of::<UserTaskStack>())
            .sub(4096) as i64
    }
}

/// The address that stack has in the task's own address space, one word down
/// for the ABI's phase.
#[unsafe(no_mangle)]
pub extern "C" fn lk_user_task_stack_virtual() -> i64 {
    (USER_STACK_VIRTUAL + 4096 - 8) as i64
}

/// The stack the user program runs on.
#[repr(align(16))]
struct UserStack([u8; 8 * 1024]);
#[unsafe(link_section = ".user")]
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
    // SAFETY: `#[export]`ed LK functions taking nothing and returning an
    // integer, compiled into this image by the same build.
    unsafe { (lk_user_code_selector() as u64, lk_user_data_selector() as u64) }
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
