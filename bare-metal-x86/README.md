# LK as native x86-64 machine code, with no OS

The aarch64 demo next door (`bare-metal-native/`) proves LK's compiled output
runs on a board. This one exists because **x86 devices are not memory-mapped**:
they live in a separate 64 KiB address space reached only by the `in` and `out`
instructions. A kernel here cannot say anything at all until it can execute
those, so `program.lk` drives a 16550 UART through `port_in_u8` /
`port_out_u8`.

```bash
rustup target add x86_64-unknown-none
cargo build -p lk-cli --features aot   # from the repo root
LK_BIN=../target/debug/lk ./run.sh
```

```
native LK drives COM1: sum(fib(0..9)) = 88
88
[lk returned to the board]
```

The first line is written by LK's own driver, compiled to x86-64 `in`/`out`
instructions. The `88` after it is the script's result value, echoed by `lkrt`
through the sink `kernel_main` installs.

## Port I/O

```lk
unsafe { port_out_u8(COM1 + REG_LCR, LCR_8N1 as u8); };
let status = unsafe { port_in_u8(COM1 + REG_LSR) };
```

`port_in_uN` / `port_out_uN` for N in 8, 16, 32 — `in`/`out` have no 64-bit
form. They need `unsafe` for the same reason `volatile_*` does: nothing in the
compiler knows what device answers at a port, or what reading it does.

They are separate intrinsics rather than a reuse of `volatile_*` because there
is **no pointer to take** — port space is not addressable by a load or a store.
They are also architecture-gated, which `volatile_*` is not: other ISAs have no
such instructions, so a program using them is x86 code, and the runtime raises
elsewhere rather than pretending.

Like the MMIO intrinsics, they lower to opaque `lkrt` calls and are marked
`WritesHost` in the ABI table *including the reads* — reading a device port can
change its state (a UART's receive register empties when read), so the
optimiser must not collapse two reads of one port.

## Booting

x86-64 cannot enter long mode in one step: long mode requires paging, paging
requires page tables, and the tables have to be built by 32-bit code. So a
multiboot loader hands control to `_start` in 32-bit protected mode, and
`boot.rs` identity-maps the first gigabyte with 2 MiB pages, enables PAE, sets
`EFER.LME`, turns paging on, and far-jumps through a 64-bit code descriptor.

Two things there are worth knowing because both fail silently:

**The multiboot header's section needs the `"a"` flag.** Without it the linker
places the section at address 0 and leaves it out of the loaded image; the
loader then does not recognise the file as multiboot and falls back to the
Linux/PVH path with a confusing message about ELF notes.

**The page tables must live outside `.bss`.** Paging is enabled before `.bss`
is zeroed, so tables inside it get wiped out from under the CPU and the next
TLB miss triple-faults — a reset loop with no output, whose cause is nowhere
near where it appears.

**QEMU's `-kernel` only accepts ELF32.** The image is 64-bit, so `run.sh`
converts the ELF class with `objcopy -O elf32-i386` after linking. Every
address in the image is below 4 GiB, so nothing is lost; this is what real
kernels do.

## Why `relocation-model=static`

`x86_64-unknown-none` defaults to PIE. The Cranelift-emitted object is not
position independent — a bare-metal image is loaded at a fixed address and has
no dynamic loader — so the link rejects its absolute relocations. Setting the
relocation model in `.cargo/config.toml` is what makes the two agree.
