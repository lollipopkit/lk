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

/* The machine's memory, decided here and nowhere else.
 *
 * Here because this file already decides where the image goes, and because it
 * is the one place *both* languages can read: Rust takes the address of an
 * `extern static`, and LK asks `symbol_address`. It used to be a table in a doc
 * comment plus a literal in each of them — `0x00380000` was written down twice
 * and cross-checked at run time by `kernel_run` refusing any other address,
 * which is a way of noticing the two had drifted, not a way of stopping it.
 *
 * The relations are written as relations, so "the arena starts where the run
 * heap ends" is a statement rather than an arithmetic coincidence between two
 * constants nobody recomputed.
 *
 *   0x00100000  the image: code, data, page tables and stacks (bounded by ASSERT)
 *   0x00300000  the shared page, which the program and its handlers agree on
 *   0x00380000  64 KiB staging for a source file read off the disk
 *   0x00400000  the interpreter's heap
 *   0x00800000  the arena a hosted run allocates from, reset per run
 *   0x02000000  the LK page allocator's arena
 *
 * The sizes are symbols too. A linker symbol is an address, and an address is a
 * number: `__heap_size` is simply a symbol whose number happens to be a length.
 */
__shared_base = 0x00300000;
__source_base = 0x00380000;
__source_max = 0x00010000;
__heap_base = 0x00400000;
__heap_size = 0x00400000;
__run_heap_base = __heap_base + __heap_size;
__run_heap_size = 0x01800000;
__page_arena_base = __run_heap_base + __run_heap_size;

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

    /* The user region has to fit under 4 MiB, and that is not a preference.
     *
     * `boot.rs` grants the U bit by indexing `__pt0` with `__user_start >> 12`
     * — an *absolute* page number — and `__pt0`/`__pt1` together are 1024
     * entries, so they describe the first 4 MiB and nothing above it. A user
     * region past that indexes off the end: nothing is granted, the ring-3
     * page stays kernel-only, and the first instruction of the program takes a
     * page fault with `rip == cr2`.
     *
     * That is exactly what happened, and what it took to get there was `.text`
     * growing by one page. `__user_start` had been sitting at 0x1ff000 — one
     * page from the edge — so any change that made the compiler bigger pushed
     * the kernel's ring 3 off a cliff, with a page fault as the only symptom.
     * A link error says which line to read. */
    ASSERT(__user_end <= 0x400000,
        "the ring-3 region no longer fits under 4 MiB, where boot.rs's 4 KiB page tables end")

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
    /* Two 4 KiB-granular tables, covering the first 4 MiB, so the user pages
       can be marked separately from the kernel ones sharing that range.

       Adjacent, and that is load-bearing: `boot.rs` grants the U bit by
       indexing from `__pt0` with an absolute page number, so the two tables
       have to read as one 1024-entry array. There was one table, covering
       2 MiB, and the ring-3 region had drifted to 0x1ff000 — one page below
       the end of what it describes. */
    __pt0 = .;
    . = . + 4096;
    __pt1 = .;
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
     * theoretical one.
     *
     * `__stack_top`, not `__bss_end`. Everything reserved above — the page
     * tables, the ring-0 stack, and the stack itself — comes *after* `.bss`,
     * and at the time of writing that is 111 KiB the old test could not see.
     * It would have let the stack sit 111 KiB inside the shared page and still
     * called the layout fine, which is the same mistake the ring-3 grant made
     * with `__pt0`: an invariant checked against the wrong quantity does not
     * announce that it is the wrong one. */
    ASSERT(__stack_top <= __shared_base,
           "the image has grown into the shared page (see the memory map at the top of this file)")

    /* And the staging area must stop below the interpreter's heap.
     *
     * `__source_max` is a length, not an address, so the two are only related
     * by an addition nobody was performing. A file one byte too long would be
     * read straight into the heap the interpreter is about to allocate from. */
    ASSERT(__source_base + __source_max <= __heap_base,
           "the source staging area overlaps the interpreter heap")

    /DISCARD/ : { *(.eh_frame*) *(.comment) }
}