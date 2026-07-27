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
.display at pci slot 2
framebuffer 0xfd000000
pixels 00001428 00ffc040
....lkos
keys 4 last 115
```

...and on the screen, a shell:

```
LK ON BARE METAL
TYPE HELP

>help
help clear echo keys exit
>echo lk
lk
>_
```

Backspace stops at the left margin rather than wrapping to the previous line:
nothing records where that line ended, and inventing it would be a guess.

## The shell

The split is the one an interrupt forces. The key handler decodes a scancode,
draws it, and appends the byte to a line in the shared page — it cannot
allocate, so the line is bytes at a fixed address rather than a string. When
Enter arrives it raises a flag and stops there. The main loop, which is allowed
to allocate and print, takes the line and runs it.

Commands are matched byte-wise against literal spellings:

```lk
if (line_starts_with([101, 99, 104, 111])) {   // "echo"
    emit(base, line_tail(5));
```

That is not a workaround for missing strings — it is the same code whether it
runs before there is a heap or after, which is the property a kernel wants.

`run_command` returns whether to keep going, so `exit` ends the loop rather
than setting a flag someone has to remember to check.

## Memory

```
>mem
upper 129920 kb
pages 31710/31712
>page
00400000
>page
00401000
```

The numbers come from the machine, not from this file. The multiboot header
asks the loader for memory information; `boot.rs` stashes the pointer it
leaves in EBX (nothing preserves a register across the long-mode transition);
`drivers/multiboot.lk` walks the map — a *chain*, not an array, since each
entry's `size` field says how far the next one is — and `drivers/pages.lk`
hands out 4 KiB pages from the largest usable range. Booting with `-m 64`
against `-m 256` changes every number.

The allocator is a bump: nothing in this kernel frees, and a free list would be
machinery for a case that does not exist. What it does have to be is honest
about the range it was given, which is why the caller passes one in rather than
this file guessing. It is also handed a base trimmed to 4 MiB, because the
image, its stack and its page tables sit below 2 MiB and the loader's map does
not know that.

The pointer's address appears in `boot.rs` and in `program.lk` as
`SHARED_MULTIBOOT`. A bare number, because an LK program has no way to name a
linker symbol — changing one without the other is the hazard.

## Types across modules

An imported function's signature is visible to the type checker, so
`for i in 0..entry_count()` needs no annotation and a call with the wrong
number of arguments is caught where it is written. What the checker reads is
only what the imported file *states* — its annotations — not what inference
would derive from its bodies.

Argument types are checked too, against *annotated* parameters. An unannotated
one also ends up with a type — inference derives one from the body — but that
is not a claim the program made, so calls are not judged against it. Annotate a
parameter and its callers are checked; leave it off and they are not.

An integer literal reaches a machine-integer parameter without a cast:
`port(0x3f8)` for `fn port(number: u16)`. Machine integers do not convert
implicitly — that is what makes `u8 + Int` an error rather than a silent
widening — but a literal has no type of its own to preserve.

Every line of that came from LK code driving four devices by three different
mechanisms:

| device | mechanism | what the program does |
| --- | --- | --- |
| COM1, a 16550 UART | port I/O (`in`/`out`) | configures the divisor and line control, polls the status register, transmits |
| PCI configuration space | the 0xCF8/0xCFC port pair | walks bus 0, finds the display controller by class code, reads BAR0, enables memory cycles |
| the framebuffer | volatile MMIO | sets a mode over the Bochs VBE ports, clears it, and draws text |
| the PS/2 keyboard | port I/O, from an interrupt handler | reads the scancode, decodes it, echoes the character to the serial line and draws it at the cursor |

`check_screen.py` screenshots the machine through QEMU's monitor and checks the
pixels. That is a separate claim from the `pixels …` line: reading the
framebuffer back proves the writes reached the device's memory, but an
unconfigured card accepts those too. Only what QEMU scans out shows that the
mode was actually set.

## The drivers are modules

```
drivers/serial.lk        a 16550 UART
drivers/pci.lk           configuration space
drivers/vbe.lk           the Bochs VBE display interface
drivers/framebuffer.lk   pixels, given a base and a stride
drivers/keyboard.lk      the PS/2 controller
drivers/pit.lk           the interval timer
drivers/shared.lk        a word an interrupt handler and the main flow share
program.lk               which devices to bring up, and what a keystroke means
```

Each driver is self-contained and knows nothing about the program. What stays
in `program.lk` is the part that is not reusable — the order things are brought
up in, the scancode table (a *layout*, not a property of the controller), and
the two interrupt handlers, because a driver decodes a scancode but only the
program knows what to do with it.

`lk compile object:` bundles file imports at compile time, the same way the
executable path does. A module may import another module; the bundler walks the whole graph.

Two limits are worth knowing. Both fail at compile time with a message rather
than at runtime, and both have the same cause: **bundling flattens the modules
into one program, while the VM runs them as separate modules with separate
heaps and copies containers across the boundary.**

**A bundled module may not write through a container parameter**, keep one, or
call a method on one. Under a flattened build the callee holds the caller's
container; under the VM it holds a copy, so `fn put(xs, i, v) { xs[i] = v; }`
changes the caller's list in one and not the other. A module that might do this
is not bundled at all — the program falls back, and an object build, which has
no fallback, reports it. Reading, indexing, iterating and `len` are unaffected,
which is what the drivers here do.

**A bundled module's top level may hold scalar constants but not containers.** Same reasoning, applied to what the module
exposes rather than what it is handed:

```lk
// module: const NAMES = ["a", "b"];  fn get() -> List<String> { return NAMES; }
let xs = get();
xs.push("z");
count()   // VM: 2 (the module kept its own copy).  Flattened: 3.
```

Refusing the shape is what stops a native build computing something the VM
would not. A container that is built inside a function is fine — it is fresh
per call, so there is nothing to share.

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

`program.lk`'s arithmetic crosses this boundary constantly, so a regression
shows up as wrong numbers rather than as a failure.

**This rests on something rustc says it does not support.** Building emits:

```
warning: target feature `soft-float` cannot be disabled with `-Ctarget-feature`:
         use a soft-float target instead
```

The supported answer is a target whose spec has hard float, which for bare
metal means a custom target JSON — and that needs `-Zbuild-std`, so it needs
nightly. The alternative is to take `f64` off the LK/`lkrt` boundary entirely
by passing bit patterns in integers; that works, but it costs a pair of
register moves on every float call *on the hosted path too*, which is the one
with a performance gate. Neither trade is worth making today.

What makes this acceptable rather than merely convenient is the failure mode:
if rustc turns that warning into an error, the build stops. It does not go
back to computing wrong numbers silently.

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

### Sharing state with a handler

The keyboard handler counts keystrokes and the main flow reads that count. A
plain global would not do: the compiler may keep one in a register across the
polling loop, and an interrupt writing memory would never be seen. So the
count lives at a fixed address reached through `volatile_read_u32` /
`volatile_write_u32` — the same reason a device register needs `volatile`,
applied to ordinary memory that changes behind the code's back.

The address is free RAM: the image, its heap, its stack and the page tables all
live below 2 MiB, so 0x0030_0000 is untouched. That is crude, and deliberately
so — a kernel with no memory manager yet has exactly this much to work with,
and pretending otherwise would hide what the program is actually doing.

`check_keyboard.py` types at the machine through QEMU's monitor. `sendkey`
puts a real scancode into the emulated controller, so the test covers IRQ1, the
LK handler, the scancode table and the echo — the one part memory inspection
cannot show.

## Text

`drivers/text.lk` renders a 5x7 font into 6x8 cells, and scrolls.

Scrolling moves the picture pixel by pixel through the same volatile accessors
as everything else — the framebuffer is device memory, so there is no `memmove`
to reach for. At this size that is around 60000 read/write pairs per line,
comfortably inside a timer period; and since an interrupt gate has already
masked interrupts, a long handler delays the next tick rather than re-entering.
 The font is a table of
integers, one row of pixels each, written for this program — the repository
carries no third-party font data — and it lives in `program.lk` rather than in
the renderer: a container at a module's top level cannot cross a bundled
import, and which glyph a character maps to is a keyboard-layout question
rather than a rendering one.

One thing in `program.lk` looks like decoration and is not:

```lk
let ascii = (SCANCODE_ASCII[code]) as Int;
```

A global that an `#[export]`ed function reads is treated as dynamic: nothing
proves the entry function ran before an interrupt did, so the compiler cannot
assume the table is initialised, and its elements arrive boxed. The cast is
where the program states what it knows.

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
