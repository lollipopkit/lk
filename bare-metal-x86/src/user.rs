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

unsafe extern "C" {
    /// The one-shot ring-3 program's stack, placed by the linker script.
    static __user_shell_stack: u8;
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
    // And the SSE registers, which this did not use to save.
    //
    // The handler is a compiled LK function now, and LK numbers are `f64`: the
    // System V ABI lets it clobber every XMM register, and the ring-3 caller
    // never agreed to that. Today's user programs are assembly that touches no
    // XMM at all, so nothing would have gone wrong yet — which is the whole
    // problem with leaving it out. The interrupt trampolines next door save
    // these for exactly this reason; the syscall path had a Rust handler that
    // did integer work, and now it does not.
    //
    // The CPU aligns RSP to 16 on the way in and seven pushes leave it eight
    // off, so the 264 both reserves the area and restores the alignment `call`
    // expects.
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
    // The ABI: number in `rax`, arguments in `rdi` and `rsi`, which the call
    // wants in `rdi`, `rsi` and `rdx`. One shuffle, in one place.
    "   mov rdx, rsi",
    "   mov rsi, rdi",
    "   mov rdi, rax",
    // The dispatcher is `program.lk`'s: what a user task may ask for is a list
    // of holes in the wall the ring boundary just built, and deciding what is
    // on that list is not the board's business.
    "   call lk_syscall_dispatch",
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
    "   pop rsi",
    "   pop rdx",
    "   pop rcx",
    "   iretq",
);

/// Where a user task's stack lives *in its own address space*.
///
/// A virtual address the kernel's space does not map to the same thing: in the
/// kernel's tables 0x4000_0000 is identity-mapped RAM that nothing uses, and in
/// the user's it is the stack. That difference is what makes them two address
/// spaces rather than one with extra permissions.


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
pub unsafe fn enter(entry: u64, stack: u64, code: u64, data: u64) -> ! {
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
pub extern "C" fn lk_enter_user(entry: i64, stack: i64, code: i64, data: i64) -> ! {
    // Every number comes from the program, and that is the point: what runs,
    // on which stack, and through which two descriptors. This side contributes
    // the one thing the program cannot say — `iretq`, which is the only way
    // into ring 3, because no instruction lowers privilege directly.
    //
    // SAFETY: the caller's claim, made by writing `unsafe` in the LK source.
    unsafe { enter(entry as u64, stack as u64, code as u64, data as u64) }
}
