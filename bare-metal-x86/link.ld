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

    /DISCARD/ : { *(.eh_frame*) *(.comment) }
}
