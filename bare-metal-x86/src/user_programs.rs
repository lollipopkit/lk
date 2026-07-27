//! The ring-3 programs, which are what the ring boundary is *for*.
//!
//! Not part of the mechanism. `user.rs` has the TSS descriptor's loading, the
//! syscall trampoline, and `iretq` — the things that make ring 3 reachable at
//! all. These are the programs that get run there, and they exist to be
//! *checked*: each one does something the boundary has to allow and then
//! something it has to refuse, so that a boundary which quietly permitted
//! everything would fail rather than look identical to one that works.
//!
//! They are assembly, and they have to be. A ring-3 program must not touch
//! anything a compiler might reasonably reach for — no spills into kernel
//! pages, no call into `lkrt`, no globals — and the only thing it may ask the
//! machine for is `int 0x80`. LK cannot express that: `int` takes its vector as
//! an immediate, so there is no operand to pass one through, and compiled LK
//! reaches for the runtime constantly. This is the one place in this kernel
//! where "written in assembly" is a statement about the *program*, not about
//! the machine.

use core::arch::global_asm;

// A program that runs in ring 3.
//
// Written in assembly rather than Rust for one reason: it must not touch
// anything the compiler might reasonably reach for — no stack spills into
// kernel pages, no calls into `lkrt`, no globals. What it does is the whole
// point of the exercise: print through the syscall, then *try* to write the
// framebuffer directly, which is the instruction the ring boundary has to
// stop.
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

// A ring-3 task that never yields.
//
// It prints a `3` through the syscall, spins for a while, and does it again,
// for ever. Nothing in it cooperates: if the shell keeps answering while this
// runs, the timer took the CPU away from ring 3 and gave it back — which is
// the claim. The earlier `user` command could not show that, because it had
// no way back at all.
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
