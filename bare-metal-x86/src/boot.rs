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
    ".long 0x1BADB002", // magic
    // Flags bit 0 asks the loader for memory information: how much there is,
    // and the map of which ranges are usable. Without it a kernel knows only
    // what it can guess.
    ".long 0x00000001",        // flags: MEMORY_INFO
    ".long -(0x1BADB002 + 1)", // checksum
);

global_asm!(
    ".section .text.boot, \"ax\"",
    ".code32",
    ".global _start",
    "_start:",
    // No interrupts until there is an IDT; the loader leaves the PIC armed.
    "   cli",
    "   mov esp, offset __stack_top",
    // The loader leaves its information structure's address in EBX, and
    // nothing preserves it across the long-mode transition — so stash it now.
    //
    // The address is a bare number because the LK program has no way to name a
    // linker symbol. It is the same shared page the program uses for the state
    // its interrupt handlers share, at the offset `program.lk` documents as
    // SHARED_MULTIBOOT. Changing one without the other is the hazard, hence
    // this paragraph.
    "   mov dword ptr [0x0030001c], ebx",
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
    // User-accessible at this level too: the CPU takes the *conjunction* of the
    // U bits along the walk, so a user page under a kernel-only directory is
    // still kernel-only.
    "   or eax, 7",
    "   mov ecx, 4",
    "2: mov [edi], eax",
    "   mov dword ptr [edi + 4], 0",
    "   add eax, 0x1000",
    "   add edi, 8",
    "   loop 2b",
    // The first 2 MiB gets 4 KiB granularity, so ring 3 can be given the pages
    // it needs and *only* those.
    //
    // Every page kernel-only to start with; then the ones between
    // `__user_start` and `__user_end` — the linker script's own section — get
    // the U bit. The directory entry above them needs it too, because the CPU
    // takes the conjunction of the U bits along the walk: a user page under a
    // kernel-only directory is still kernel-only.
    "   mov edi, offset __pt0",
    "   mov eax, 0x03", // present | writable, no user
    "   mov ecx, 512",
    "5: mov [edi], eax",
    "   mov dword ptr [edi + 4], 0",
    "   add eax, 0x1000",
    "   add edi, 8",
    "   loop 5b",
    "   mov esi, offset __user_start",
    "   shr esi, 12", // first user page
    "   mov edx, offset __user_end",
    "   add edx, 0xfff",
    "   shr edx, 12",
    "   sub edx, esi", // how many
    "   jz 7f",        // nothing to grant
    "   mov edi, offset __pt0",
    "   lea edi, [edi + esi*8]",
    "6: or dword ptr [edi], 4", // user-accessible
    "   add edi, 8",
    "   dec edx",
    "   jnz 6b",
    "7: mov eax, offset __pt0",
    "   or eax, 7", // present | writable | user
    "   mov edi, offset __pd",
    "   mov [edi], eax",
    "   mov dword ptr [edi + 4], 0",
    "   mov eax, offset __pdpt",
    "   or eax, 7",
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

// The GDT that gets this code as far as long mode, and no further.
//
// Long mode ignores a code segment's base and limit, but a descriptor still has
// to exist and say "64-bit code" (the L bit) — that is what the far jump above
// selects. So this table cannot be avoided: entering long mode takes a `lgdt`
// and a far jump, both before any compiled code exists to do them.
//
// Three entries, and deliberately three. It once had six — the ring-3 pair and
// a TSS descriptor as well — and that was a table describing the machine's
// *policy*, written in the one file least able to say why. `program.lk` builds
// the table the machine actually runs on (`install_descriptor_table`), with
// whatever segments it has decided to have; this one only has to be enough to
// reach the code that does that.
//
// Which makes the ring-3 test a proof rather than a demonstration: there is no
// ring-3 descriptor anywhere in this image except the one the program writes at
// run time. A user task that runs at all is a user task running on the
// program's table.
//
// `.rodata` would do now that nothing is filled in at boot, and `.data` is kept
// only because a descriptor table is a thing the machine may yet want to write.
global_asm!(
    ".section .data, \"aw\"",
    ".align 16",
    ".global __gdt",
    "__gdt:",
    "   .quad 0",                  // 0x00: null descriptor
    "   .quad 0x00AF9A000000FFFF", // 0x08: 64-bit code, ring 0
    "   .quad 0x00AF92000000FFFF", // 0x10: data, ring 0
    "__gdt_descriptor:",
    "   .word __gdt_descriptor - __gdt - 1",
    "   .quad __gdt",
);
