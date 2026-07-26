//! The reset path: multiboot entry in 32-bit protected mode, ending in 64-bit
//! long mode with a stack and identity-mapped memory.
//!
//! A multiboot loader (QEMU's `-kernel`, GRUB) hands control over in 32-bit
//! protected mode with paging off and no stack. Everything compiled here is
//! 64-bit, so the boot code's whole job is the transition — which on x86-64
//! cannot be done in one step: long mode requires paging, paging requires page
//! tables, and the tables have to be built by 32-bit code.

use core::arch::global_asm;

// The multiboot 1 header. `flags = 0` asks for nothing beyond being loaded;
// the checksum must make the three fields sum to zero.
global_asm!(
    // `"a"`: without the allocatable flag the linker places the section at
    // address 0 and leaves it out of the loaded image, so the loader never
    // finds the header and falls back to the Linux/PVH path.
    ".section .multiboot, \"a\"",
    ".align 4",
    ".long 0x1BADB002",  // magic
    ".long 0",           // flags
    ".long -0x1BADB002", // checksum
);

global_asm!(
    ".section .text.boot, \"ax\"",
    ".code32",
    ".global _start",
    "_start:",
    // No interrupts until there is an IDT; the loader leaves the PIC armed.
    "   cli",
    "   mov esp, offset __stack_top",
    // Identity-map the first four gigabytes with 2 MiB pages.
    //
    // Four rather than one because a PCI device's framebuffer is mapped near
    // the top of the 32-bit physical range — a driver cannot reach it through
    // a map that stops at 1 GiB.
    //
    // 2 MiB rather than 1 GiB pages because `PDPE1GB` is a CPUID feature and
    // this has to work on whatever the machine turns out to be; 2048 entries
    // is a short enough loop not to care.
    //   PD[i]   = i * 2 MiB | PRESENT | WRITABLE | PAGE_SIZE
    //   PDPT[g] = PD_g | PRESENT | WRITABLE
    //   PML4[0] = PDPT | PRESENT | WRITABLE
    "   mov edi, offset __pd",
    "   mov eax, 0x83",
    "   mov ecx, 2048",
    "1: mov [edi], eax",
    "   mov dword ptr [edi + 4], 0",
    "   add eax, 0x200000",
    "   add edi, 8",
    "   loop 1b",
    "   mov edi, offset __pdpt",
    "   mov eax, offset __pd",
    "   or eax, 3",
    "   mov ecx, 4",
    "2: mov [edi], eax",
    "   mov dword ptr [edi + 4], 0",
    "   add eax, 0x1000",
    "   add edi, 8",
    "   loop 2b",
    "   mov eax, offset __pdpt",
    "   or eax, 3",
    "   mov edi, offset __pml4",
    "   mov [edi], eax",
    "   mov dword ptr [edi + 4], 0",
    "   mov eax, offset __pml4",
    "   mov cr3, eax",
    // PAE. Long mode has no non-PAE paging mode.
    "   mov eax, cr4",
    "   or eax, 1 << 5",
    "   mov cr4, eax",
    // EFER.LME — "long mode enable". It does not take effect until paging is
    // switched on, which is the next step.
    "   mov ecx, 0xC0000080",
    "   rdmsr",
    "   or eax, 1 << 8",
    "   wrmsr",
    // Paging on. From here the CPU is in compatibility mode; the far jump
    // through a 64-bit code segment is what actually enters long mode.
    "   mov eax, cr0",
    "   or eax, 1 << 31",
    "   mov cr0, eax",
    "   lgdt [__gdt_descriptor]",
    "   jmp 0x08, offset __long_mode",
    ".code64",
    "__long_mode:",
    // The data segment registers still hold 32-bit selectors. In long mode
    // they are mostly ignored, but leaving stale ones set is the kind of thing
    // that fails later and elsewhere.
    "   mov ax, 0x10",
    "   mov ds, ax",
    "   mov es, ax",
    "   mov ss, ax",
    "   mov fs, ax",
    "   mov gs, ax",
    // Enable SSE. x86-64 guarantees the *instructions* exist, but they raise
    // #UD until the OS says it is prepared to save their state: CR0.EM clear
    // and CR0.MP set (there is a real FPU, do not emulate), CR4.OSFXSR and
    // CR4.OSXMMEXCPT set. LK numbers are `f64`, so without this the first
    // floating-point instruction faults — and so does the interrupt
    // trampoline, which saves the XMM registers.
    "   mov rax, cr0",
    "   and rax, ~(1 << 2)",
    "   or rax, 1 << 1",
    "   mov cr0, rax",
    "   mov rax, cr4",
    "   or rax, (1 << 9) | (1 << 10)",
    "   mov cr4, rax",
    "   mov rsp, offset __stack_top",
    // Zero `.bss`. Rust assumes it, and the loader guarantees nothing about
    // memory it did not load from the file.
    "   mov rdi, offset __bss_start",
    "   mov rcx, offset __bss_end",
    "   sub rcx, rdi",
    "   xor eax, eax",
    "   rep stosb",
    "   call kernel_main",
    // `kernel_main` does not return; if it somehow does, park.
    "3: hlt",
    "   jmp 3b",
);

// A minimal GDT. Long mode ignores the base and limit of a code segment, but a
// descriptor still has to exist and say "64-bit code" (the L bit) — that is
// what the far jump above selects.
global_asm!(
    ".section .rodata, \"a\"",
    ".align 16",
    "__gdt:",
    "   .quad 0",                  // null descriptor
    "   .quad 0x00AF9A000000FFFF", // 0x08: 64-bit code, ring 0
    "   .quad 0x00AF92000000FFFF", // 0x10: data, ring 0
    "__gdt_descriptor:",
    "   .word __gdt_descriptor - __gdt - 1",
    "   .quad __gdt",
);
