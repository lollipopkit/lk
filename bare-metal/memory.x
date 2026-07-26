/* QEMU's ARM MPS2 AN386 (Cortex-M4): two 4MB ZBT SRAM banks, one mapped as the
   code region and one as SRAM.

   This board is deliberately roomy. LK's full front end (tokenizer, parser,
   type checker, VM compiler) costs ~670KB of flash, so it does not fit a
   256KB-class MCU; see README.md for the measured breakdown and for the
   artifact-only path that halves it. */
MEMORY
{
  FLASH : ORIGIN = 0x00000000, LENGTH = 4M
  RAM   : ORIGIN = 0x20000000, LENGTH = 4M
}
