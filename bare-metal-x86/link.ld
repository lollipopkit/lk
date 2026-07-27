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