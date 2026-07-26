# LK as native x86-64 machine code, with no OS

The aarch64 demo next door (`bare-metal-native/`) proves LK's compiled output
runs on a board. This one exists because **x86 devices are not memory-mapped**:
they live in a separate 64 KiB address space reached only by the `in` and `out`
instructions. A kernel here cannot say anything at all until it can execute
those, so `program.lk` drives a 16550 UART through `port_in_u8` /
`port_out_u8` — and then goes on to find the display controller in PCI
configuration space and draw to its framebuffer.

```bash
rustup target add x86_64-unknown-none
cargo build -p lk-cli --features aot   # from the repo root
LK_BIN=../target/debug/lk ./run.sh
```

```
....display at pci slot 2
framebuffer 0xfd000000
pixels 00000040 007f7f40 00fefd40
.2
[lk returned to the board]
```

Every line of that came from LK code driving three devices by three different
mechanisms:

| device | mechanism | what the program does |
| --- | --- | --- |
| COM1, a 16550 UART | port I/O (`in`/`out`) | configures the divisor and line control, polls the status register, transmits |
| PCI configuration space | the 0xCF8/0xCFC port pair | walks bus 0, finds the display controller by class code, reads BAR0, enables memory cycles |
| the framebuffer | volatile MMIO | sets a mode over the Bochs VBE ports, then writes 64000 pixels |

`check_screen.py` screenshots the machine through QEMU's monitor and checks the
pixels. That is a separate claim from the `pixels …` line: reading the
framebuffer back proves the writes reached the device's memory, but an
unconfigured card accepts those too. Only what QEMU scans out shows that the
mode was actually set.

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

## The float ABI, which is where this went wrong

`x86_64-unknown-none` is a **soft-float** target: Rust code built for it passes
and returns `f64` in integer registers, on the assumption that a kernel does not
want to save SSE state. The Cranelift-emitted LK object uses the ordinary SysV
ABI, where floats travel in XMM. Left mismatched, a call like
`lkrt_f64_div_checked` reads its arguments from the wrong registers and the
program computes a **wrong number** — no link error, because the symbol names
agree. `lk compile object:` now warns about this; the fix is three things that
have to be decided together:

- `-C target-feature=-soft-float,+sse,+sse2` in `.cargo/config.toml`. Removing
  `soft-float` is not optional: adding `+sse` while leaving it set puts LLVM in
  a state where SSE is available but the ABI is still soft, and the result is a
  hang rather than an error.
- `boot.rs` clears `CR0.EM`, sets `CR0.MP`, and sets `CR4.OSFXSR` /
  `CR4.OSXMMEXCPT`. x86-64 guarantees the SSE *instructions* exist, but they
  raise #UD until the OS says it is prepared to save their state.
- the interrupt trampoline saves all sixteen XMM registers, because the
  interrupted computation may now be holding a float in one.

The `half = 44` line in the output exists to keep this honest: it is a `f64`
round trip across the boundary, so a regression prints `88` instead of failing.

## Interrupts

Each `.` is a timer interrupt **handled by an LK function**:

```lk
#[export("lk_timer_isr")]
fn on_tick() {
    uart_putc(46);
}
```

The board's share is an IDT, remapping the 8259 PIC away from the vectors the
CPU reserves for exceptions, acknowledging the interrupt, and spilling every
caller-saved register. What a tick *means* is the program's, and that part is
LK — including programming the PIT's divisor, which `program.lk` does with the
same `port_out_u8` its UART driver uses.

The handler and the main program share a device, so `program.lk` masks
interrupts around the lines it does not want spliced:

```lk
let irq = unsafe { cpu_irq_save() };
uart_write(/* … */);
unsafe { cpu_irq_restore(irq); };
```

Two things the handler must not do, both because an interrupt lands between any
two instructions of the interrupted program — including instructions inside the
runtime: allocate, or take a lock.

## Exceptions

Vectors 0-31 are the CPU's own faults. Without gates for them a fault becomes a
double fault becomes a triple fault, which on this machine is a **silent reset
loop** — the failure mode with the least information possible, and the one that
cost the most time getting this demo working. All 32 now report:

```
!! exception #PF page fault vector=000000000000000e error=0000000000000002 \
   rip=0000000000100729 cr2=0000000900000000
```

The CPU pushes an error code for some vectors and not others, and tells the
handler nothing about which one fired. So there are 32 stubs, each pushing a
dummy zero where there is no error code and then its own number; after that the
stack layout is identical and one common tail reads it. They are padded to a
fixed stride so their addresses are computable, rather than needing 32 labels.

`--features fault-probe` builds an image that faults on purpose. Without a
build that takes the path, a broken reporter looks exactly like a working one.

## Booting

x86-64 cannot enter long mode in one step: long mode requires paging, paging
requires page tables, and the tables have to be built by 32-bit code. So a
multiboot loader hands control to `_start` in 32-bit protected mode, and
`boot.rs` identity-maps the first **four** gigabytes with 2 MiB pages, enables PAE, sets
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

Four gigabytes rather than one because a PCI device's framebuffer is mapped
near the top of the 32-bit physical range — this machine puts it at
`0xfd000000`, and a driver cannot reach it through a map that stops at 1 GiB.

**QEMU's `-kernel` only accepts ELF32.** The image is 64-bit, so `run.sh`
converts the ELF class with `objcopy -O elf32-i386` after linking. Every
address in the image is below 4 GiB, so nothing is lost; this is what real
kernels do.

## Why `relocation-model=static`

`x86_64-unknown-none` defaults to PIE. The Cranelift-emitted object is not
position independent — a bare-metal image is loaded at a fixed address and has
no dynamic loader — so the link rejects its absolute relocations. Setting the
relocation model in `.cargo/config.toml` is what makes the two agree.
