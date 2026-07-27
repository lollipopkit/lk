/* Memory layout for a multiboot kernel on QEMU's `q35`/`pc` machines.
 *
 * 1 MiB is where a multiboot loader places a kernel: below it is the legacy
 * low-memory area (real-mode IVT, BIOS data, VGA, option ROMs), which is not
 * ours to overwrite.
 *
 * The multiboot header has to be in the first 8 KiB of the file and 4-byte
 * aligned, or the loader will not recognise the image at all — hence its own
 * section, placed first. */
ENTRY(_start)

SECTIONS
{
    . = 0x100000;

    .multiboot : {
        KEEP(*(.multiboot))
    }

    .text : {
        KEEP(*(.text.boot))
        *(.text .text.*)
    }

    /* Everything ring 3 may reach, on its own pages.
     *
     * Page-aligned at both ends because permission is granted per page: a user
     * region sharing a page with kernel code would hand ring 3 that code too,
     * which is exactly what the first version of this did with a 2 MiB page. */
    . = ALIGN(4096);
    __user_start = .;
    .user : { *(.user .user.*) }

    /* The stack the one-shot `user` command's ring-3 program runs on.
     *
     * Inside the user region, and it has to be: that program runs in the
     * *kernel's* address space, so its stack is only reachable because these
     * pages carry the U bit. The per-task ones below are a different case. */
    . = ALIGN(4096);
    __user_shell_stack = .;
    . = . + 4096;
    __user_end = .;

    /* The ring-3 tasks' stacks, deliberately *outside* the user region.
     *
     * Each task maps its own at `USER_STACK_VIRTUAL` in its own address space,
     * so it needs no U bit here — and being outside is what stops one task
     * handing the kernel a pointer into the other one's stack through
     * `write(ptr, len)`, which checks against exactly this range.
     *
     * Named here rather than declared as Rust statics so `program.lk` can ask
     * for them itself. A `static mut [u8; N]` and a linker reservation are the
     * same thing; only one of them has a name every language can say. */
    . = ALIGN(4096);
    __user_task_stack_a = .;
    . = . + 4096;
    __user_task_stack_b = .;
    . = . + 4096;

    .rodata : { *(.rodata .rodata.*) }
    .data   : { *(.data .data.*) }

    .bss : {
        __bss_start = .;
        *(.bss .bss.* COMMON)
        __bss_end = .;
    }

    /* The page tables the 32-bit boot code fills in. Page aligned because CR3
       and the table entries hold physical addresses with no low bits.

       Deliberately *outside* `.bss`: paging is enabled before `.bss` is
       zeroed, so tables living inside it would be wiped out from under the
       CPU, and the next TLB miss would triple-fault. */
    . = ALIGN(4096);
    __pml4 = .;
    . = . + 4096;
    __pdpt = .;
    . = . + 4096;
    /* Four page directories, one per gigabyte: a device's framebuffer sits
       near the top of the 32-bit physical range, far outside the first. */
    __pd = .;
    . = . + 4 * 4096;
    /* One 4 KiB-granular table for the first 2 MiB, so the user pages can be
       marked separately from the kernel ones sharing that range. */
    __pt0 = .;
    . = . + 4096;

    /* No user page tables reserved here any more. `program.lk` builds an
       address space out of pages from its own allocator, so how many there can
       be is bounded by memory rather than by a number written down here — and
       the number written down here was four, which was never justified by
       anything except that two had been. */

    /* The ring-0 stack the CPU switches to when an interrupt arrives from ring
       3 while *task 0* is running — the one task that pre-exists the table and
       so never published a stack of its own.
     *
     * Its own, not the interrupted task's: a user task's stack pointer is a
     * value the user chose, and an interrupt that pushed onto it would hand the
     * frame it is about to `iretq` from to the program it interrupted.
     *
     * Both ends named, so `program.lk` can ask for the top without also being
     * told how big it is. */
    . = ALIGN(16);
    __ring0_stack = .;
    . = . + 16384;
    __ring0_stack_top = .;

    /* The stack grows down from here. */
    . = ALIGN(16);
    . = . + 0x10000;
    __stack_top = .;

    /* The image must stop below the shared page.
     *
     * `program.lk` and the interrupt handlers agree on 0x300000 by writing the
     * number down, not by asking the linker — an interrupt handler cannot look
     * anything up. Growing the image past it would not fail to build and would
     * not fail to boot: it would quietly put the key handler's line buffer on
     * top of the kernel's own data. Since the interpreter went in, the image is
     * ten times the size it was, so this is a real bound rather than a
     * theoretical one. */
    ASSERT(__bss_end < 0x300000,
           "the image has grown into the shared page at 0x300000 (see SHARED_BASE in program.lk)")

    /DISCARD/ : { *(.eh_frame*) *(.comment) }
}