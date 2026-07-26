/* Memory layout for QEMU's aarch64 `virt` machine.
 *
 * RAM starts at 0x4000_0000. 0x4008_0000 is where the Arm Linux boot protocol
 * places a kernel, and QEMU's `-kernel` follows it — starting there means the
 * image loads where the loader already expects it.
 *
 * There is no bootloader beneath this: `_boot` runs on a bare core with no
 * stack, which is why it sets one before calling anything compiled. */
ENTRY(_boot)

SECTIONS
{
    . = 0x40080000;

    .text : {
        /* The boot stub must come first: execution begins at the lowest
           address of the image, and the reset path has no vector table to
           redirect it. */
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

    /* The stack grows down from here. 64 KiB is generous for this demo and
       cheap on a machine with 128 MiB of RAM. */
    . = ALIGN(16);
    . = . + 0x10000;
    __stack_top = .;

    /DISCARD/ : { *(.eh_frame*) *(.ARM.exidx*) }
}
