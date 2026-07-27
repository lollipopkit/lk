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
    . = ALIGN(4096);
    __user_end = .;

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

    /* A second address space, for a user task: its own PML4 and the two tables
       that differ from the kernel's. Everything else is shared by pointing at
       the kernel's own tables — the kernel has to be mapped in every space, or
       an interrupt during ring 3 would have nowhere to land. */
    /* One address space per user task, up to the task table's own capacity.
       Two was what proved the point — one space is indistinguishable from a
       permission — but two is also a number nothing justifies keeping, and a
       third user task being refused for want of 16 KiB is an arbitrary limit
       rather than a decided one. */
    __user_pml4 = .;
    . = . + 4 * 4096;
    __user_pdpt = .;
    . = . + 4 * 4096;
    __user_pd = .;
    . = . + 4 * 4096;
    __user_pt = .;
    . = . + 4 * 4096;

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