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

## Tasks

Two tasks, preempted by the timer. The shell is one; the other spins a glyph in
the top-right corner and never yields — the CPU is taken away from it.

The split follows what an interrupt can and cannot decide. `src/tasks.rs` owns
the mechanics: a stack per task, the frame a task starts life on, and the
register bookkeeping. *Which* task runs next is `lk_schedule`, an `#[export]`ed
LK function — round robin today, but the shape is where a policy goes, and it
is the program's.

The timer's trampoline saves **every** register rather than only the
caller-saved ones. What is on the stack has to be a whole task, because the
stack the handler returns on may not be the one it arrived on.

Two things about a task's first stack, both of which fail as a fault somewhere
else entirely:

- **The stack must be 16-byte aligned.** Compiled LK code spills SSE registers
  with `movaps`; a `[u8; N]` has alignment 1, and the fault lands inside
  whatever the task called.
- **Its initial RSP is one word below the top.** The ABI assumes a `call` has
  just pushed a return address, and `iretq` pushes nothing — so without the
  offset the stack is in the wrong phase and the first aligned spill faults.

A task may not allocate, the same rule the interrupt handlers follow and for
the same reason: `lkrt` has one arena and no lock around it. The spinner picks
its glyph with an `if` chain rather than indexing a list, because building the
list would allocate.

### Starting one

The board does not know what tasks exist. `src/tasks.rs` owns stacks, the saved
stack pointers and the switch; *which* code runs on them is the program's:

```lk
let irq = lock();
let spinner = spawn_task(unsafe { symbol_address("lk_task_b") });
let clock = spawn_task(unsafe { symbol_address("lk_task_clock") });
shared_write(SHARED_TASK_COUNT, clock + 1);
unlock(irq);
```

`TASK_COUNT` became `TASK_CAPACITY`: the stacks are still static, because
nothing here can grow a table an interrupt is reading, but which slots are in
use is decided at run time. Two details decide whether this works at all:

- **Spawning runs with interrupts masked**, and the slot is published *last*.
  The scheduler reads the table from a timer interrupt; a slot visible before
  its stack is prepared is a jump to zero.
- **The scheduler is clamped against what is spawned**, not against the
  capacity — naming an empty slot resumes a stack that was never built.

The clock task is also where "a task may not allocate" stopped being a rule
about the spinner and became a rule about everything: drawing the seconds used
to build a list of digits, which allocates, so it now divides right-to-left and
draws as it goes. Same arithmetic, no intermediate list.

### Asking the kernel for something

`#[extern]` is the mirror of `#[export]`: the board implements the function,
and a native build calls that symbol.

```lk
#[extern("kernel_yield")]
fn task_yield() {
}
```

The body is what the *interpreter* runs — it has no way to reach the outside
implementation — so a fallback goes there. That also makes this the one
construct whose two back ends are not checked against each other: the thing
being called is not in the program.

`kernel_yield` is a software interrupt (`int 0x30`) rather than a plain call.
The switch needs a complete interrupt frame on the stack, because that is what
the resume path expects to find; `int` builds one and a `call` does not. The
vector is past the PIC's remapped range, so nothing but an `int` can raise it
— there is no device to acknowledge.

There is no privilege boundary here to cross: LK and the kernel are one binary
at ring 0. What this is, is the direction `#[export]` did not cover — the
program asking the board for something only the board can do.

### Sharing the screen between them

The spinner and the shell do not overwrite each other's pixels. Until this
round that was because their coordinates happened not to overlap — an
arrangement, not a guarantee. `drivers/window.lk` makes it one: every write
goes through a rectangle, is translated into it, and is *dropped* if it falls
outside.

Dropped rather than refused. A refusal has to be reported to someone, and the
caller is often an interrupt handler with nowhere to report to. A counter says
the same thing without needing anyone to be listening — and it is what tells
"did not draw outside" apart from "was not asked to", which is why the spinner
deliberately draws one glyph to the left of its window on every frame. `win`
reports the count; `check_tasks.py` screenshots those pixels and requires them
to still be background.

Both halves of that check are load-bearing, and both have been seen to fail:
replacing `window_put` with a direct `put_pixel` lights 35 of them.

### Shift, and what a modifier is

A modifier key is a key like any other: the controller has no notion of one, and
reports a press and a release for `Shift` exactly as it does for `A`. What makes
it a modifier is that the *program* keeps its state instead of translating it —
which is why `drivers/keyboard.lk` only names the three scancodes and the
handler does the rest.

Both edges matter for `Shift`, and only one for `CapsLock`: the release is what
ends a shift, while a lock that ended when you let go would be a shift key. And
they compose differently — caps lock affects letters only (a keyboard where it
turned `1` into `!` is one nobody could type on), so a letter asks "is exactly
one of them in effect" while everything else asks only about shift.

The shifted punctuation is a table of literals rather than arithmetic. There is
no relation between `1` and `!` beyond a convention, and writing the convention
down is the honest way to say so.

### Who gets the keyboard

Tab moves the focus between the two windows. The key handler does not know what
a keystroke *means* — only who it belongs to: it decodes the scancode and puts
the byte in the focused window's ring (`drivers/queue.lk`), and stops there.
Building a command line out of those bytes is the shell's own work, done in its
task where allocating and printing are allowed.

That split is what makes a second interactive window cost nothing extra. The
pane beside the spinner drains its own ring and draws into its own rectangle;
neither window can reach the other's pixels or the other's input.

The ring drops bytes when it is full rather than blocking. A handler has no one
to wait for, and the count of what it dropped is the only report it can make to
someone who may not be listening.

`check_shell.py` types `help`, presses Tab, types `help` again, and requires the
shell to have answered exactly **once**. Routing every key to the shell makes it
answer twice, which is what the check says when it fails.

### Pointing at one

`drivers/mouse.lk` drives the PS/2 mouse — the same controller as the keyboard,
which is most of what makes it awkward: commands for the mouse are prefixed with
0xD4, the controller's own configuration byte has to be edited before it will
interrupt, and IRQ12 is on the *slave* PIC, so it is invisible until IRQ2 (the
cascade, which is not a device) is unmasked too and its end-of-interrupt goes to
both chips.

Clicking a window gives it the keyboard, which is `check_focus.py`'s claim
reached the other way.

Three failures worth writing down, because each looked like a dead device:

- **The initialisation has to run with interrupts masked.** Every mouse command
  answers `0xFA`, and IRQ12 is unmasked by then — so the acknowledgement raises
  an interrupt, the handler reads the byte as packet data, and the
  initialisation waits for a byte that has already been taken.
- **The controller's output buffer has to be drained afterwards.** A byte left
  from before the stream started becomes byte 0 of the first packet, and being
  one byte out is not a small error: the flags land where a movement byte
  belongs, and the overflow bits they happen to contain make *every* movement
  read as zero. The cursor sits still while packets arrive.
- **The handler must not allocate.** Printing the packet to the serial line for
  debugging used `uart_put_int`, which builds a list — and the fault was a #GP
  inside the allocator. The rule was already written down for tasks; the mouse
  is where it was tested.

The cursor is drawn by the shell, not the handler: the handler assembles packets
and publishes a position, because drawing from an interrupt would put the screen
in the hands of whatever it interrupted. It saves the pixels it covers and puts
them back before moving — XOR would be one operation instead of three, and would
produce a cursor whose colour depends on what is under it, which over this
program's own amber-on-blue is sometimes the background.

### Moving one

A window can be dragged by its title bar — the top four pixels — and the
question that answers is not "how do you move a rectangle" but **what happens to
what it covered**.

There is no backing store for pixels, and there is not going to be one: at
320x200x32 a full screen is 256 KiB, and a kernel that keeps a copy per window
has given its memory to the window manager. What there is instead is the
terminal's own **character grid**: 53x25 bytes in the shared page saying what
belongs at each cell. The screen was the only record of what had been typed
until this, which is fine right up until something covers it.

So `move_window` blanks the rectangle a window is leaving and asks the terminal
to redraw the cells it touched. `check_drag.py` types `help`, drags the pane
onto the answer, checks the text is gone, drags it off, and checks that every
one of the 132 lit pixels came back.

Three things this turned up:

- **The pointer has to be taken off the screen before anything redraws.** The
  cursor's saved pixels describe the screen as it was when the cursor was drawn;
  redraw underneath it and those pixels become a small rectangle of the old
  picture, which the next cursor move stamps back onto the new one. A few
  characters never came back, and that was why.
- **`-1` does not survive the shared page.** The words are 32-bit and a read
  comes back *unsigned*, so `-1` returns as 4294967295 — which is not less than
  zero. The drag slot used -1 for "nothing", took 4294967295 for a window
  number, and computed a descriptor address of `0x18_0030_003c`. The page fault
  named the address; the only clue in it was that 24, the descriptor stride,
  appeared in the high half. Sentinels here are out-of-range *values*, not
  negative ones.
- **The shell's frame and the terminal's first column share a pixel.** A
  full-screen window with a one-pixel frame has its left edge exactly where cell
  column 0 begins, and whichever is drawn last wins. It is a property of the
  layout rather than of dragging, and it is the kind of thing to fix by giving
  the terminal an inset once there is a reason to.

### Which one is on top

Three windows now, and they may overlap: the shell (the desktop), the spinner,
and a clock. Clicking one brings it to the front; dragging one moves it over
whatever is beneath.

The stacking order is a **list of slots, bottom-up**, not a depth number per
window. Depths have to be renumbered when one changes and two windows can end up
sharing one; a list can only ever say one thing about who is above whom.

What overlap actually demands is that **every window can redraw itself from
state**, because being uncovered can happen at any moment and there are no saved
pixels to put back. Each window here is defined by something it can be recomputed
from:

| window | redrawn from |
| --- | --- |
| the shell | the terminal's character grid |
| the spinner | its step counter |
| the clock | the tick count |

The clock is the one that made the remaining problem obvious. A window drawing
its own contents has no idea what is above it — so a clock that redraws itself
once a second buries whatever was just raised, about a second after the click.
Every path that repaints one window therefore ends in `repaint_above`, which
puts back whatever sits on top of it. `check_stack.py` waits three seconds
before looking, because a raise that a tick undoes is not a raise.

### Windows say what draws them

`repaint_window` does not know what any window contains. It holds an address per
slot and calls it:

```lk
fn repaint_window(base: Int, slot: Int) {
    let painter = shared_read(SHARED_WINDOW_PAINT + slot * WORD);
    unsafe { call_address_2(painter, base, slot) };
}
```

That is a **driver table**, and it needed two things the language did not have:
`symbol_address("name")`, which is an `#[export]`ed function's address, and
`call_address_2(addr, a, b)`, which calls through one. Both are native-only —
the VM refuses rather than inventing an address, the same choice `port_in_u8`
makes, because a fake address is a program that runs interpreted and jumps into
nothing when compiled. The name must be a literal: a relocation is a name
resolved at link time, and a kernel has no symbol table to look one up in.

The alternative is the chain of `if slot == …` this replaced, sitting in the
middle of the window manager and edited every time a window is added. Kernels
have driver tables for exactly this reason: what a device does belongs to the
device, and dispatch should not have to learn its name.

The signature is the table's, not each window's: two integers in, one out. A
table of function pointers is a single signature by definition — that is what
makes it a table.

### Seeing which one has it

Each window draws its own one-pixel frame, focused or idle, and repaints when
the focus moves. The frame is *inside* the rectangle rather than around it, so
it goes through the same clipped write as everything else — the alternative, a
frame owned by something above the windows, needs a window manager, and there
is none here.

Who repaints is the question this raises, and the answer is the owner. The key
handler that moves the focus only writes a word; each window notices on its own
turn and redraws itself. Drawing from an interrupt would put a window's
appearance in the hands of whatever it happened to interrupt.

`check_focus.py` scrolls the screen thirty lines first, then screenshots before
and after Tab and requires the two frames to have swapped colours. Removing the
repaint — leaving the handler's word written but nothing acting on it — makes
it fail, which is what it is for. The scrolling is not incidental: the view
pans and wraps in those thirty lines, so a frame still in the right place
afterwards is one that was repainted at the new origin rather than one that
happened not to move.

### What it costs

`cpu_timestamp()` reads the core's cycle counter, which is what a kernel needs
before it can claim anything got faster. `time` measures the two things this
program does that are not cheap, both of which run with interrupts masked — so
what they cost is time the other task does not get:

```
>time
scroll 289404 frames 170354
```

The first measurement said `scroll 8126964`. Scrolling moved the picture a
pixel at a time through opaque runtime calls — they have to be opaque, or the
optimiser would collapse repeated reads of device memory — so the cost was
mostly the *number* of calls. A pixel is four bytes and the framebuffer is
contiguous, so a 64-bit access carries two of them; halving the calls halved
the cost to 4.18M, which is what the numbers said and what the model predicted.

The second order of magnitude came from not moving pixels at all. The
framebuffer is allocated twice as tall as the screen (`VIRTUAL_HEIGHT = 400`)
and scrolling moves *where the screen starts reading*: one write to the Bochs
VBE `Y_OFFSET` register. Every drawing routine still takes screen coordinates —
the pan is folded into the base address by `framebuffer_origin()`, because the
framebuffer is linear and `base + pan * width * 4` *is* the visible origin.
Only when the view reaches the bottom of the framebuffer does anything get
copied, and then once per screenful rather than once per line. **376k cycles,
21× the first measurement.** What was left of it was not the scrolling at all:
it was clearing the one row of pixels that had just come into view, so
`fill_rect` now writes that band two pixels at a time as well — 289k.

What that does not buy is free scrolling overall, and the honest number is the
sum. Panning moves *everything*, chrome included, so both window frames have to
be drawn again at the new origin, and that now happens once per scroll. It
measured 518k, which the same measurement said was a fault in the window layer
rather than in the panning: `window_put` re-read the descriptor and re-checked
the bounds for each of the 1040 pixels in the two frames, five volatile loads
from the shared page per pixel. Reading the rectangle once per *shape* and
passing it in registers (`put_within`) took it to 170k without weakening the
clipping — every pixel is still checked, and what falls outside is still
counted, just added up in a register and recorded once at the end instead of a
read-modify-write on the shared page in the inner loop.

Scrolling a line therefore costs about **460k cycles all told, against 8.13M
where this started** — 17×. The remaining cost is spread evenly enough between
the two halves that the next thing worth doing is not another constant factor:
it is not repainting chrome that did not need to move, which needs the drawing
to know what changed.

A word of warning that this round paid for: the pan lives in the shared page,
and the first address chosen for it, `0x00300044`, was *inside* the window
descriptor table at `0x00300040` — it was the shell window's `top` field. Every
frame was then drawn at twice the pan, which looks exactly like a panning bug
and is not one. The shared page has no allocator; the comments above each
constant are the whole defence, so a block of them states its length.

## A block device

`drivers/ata.lk` is the first driver here that has to **wait for hardware**. A
UART transmits when asked and a framebuffer accepts a write immediately; a disk
takes milliseconds and says so through a status register. Every function in it
therefore has a bounded spin and reports a timeout rather than hanging — on a
board with no operating system that is the difference between a failed read and
a dead computer.

```
>disk
64 LK-DISK-OK-01234
>disk w
LK-WROTE-SECTOR1
```

The first line is IDENTIFY's own sector count and the first sixteen bytes of
sector 0. Printed as bytes rather than as a checksum on purpose: the point of a
disk driver's first outing is that *these* bytes came off *that* medium, and a
checksum is equally consistent with a bug that reads the same wrong thing every
time.

Two details of the interface cost a debugging session each and are worth
stating:

- **Select the drive before reading status.** Before a selection the command
  block belongs to no drive and reads back 0 — indistinguishable from an empty
  bus. The presence check reported "no disk" for a disk that was plainly there.
- **A write is acknowledged long before it is on the medium**, which is what the
  `FLUSH CACHE` command after it is for. Read-back through the controller cannot
  show the difference. `check_disk.py` therefore makes its third assertion from
  *outside*, after QEMU has exited: the image file must hold what was written.

## The descriptor table, and how to tell whose it is

`program.lk` builds the GDT the machine runs on — null, ring-0 code and data,
ring-3 code and data, and the sixteen-byte TSS descriptor — loads it with
`lgdt`, reloads CS through a far return, and points the task register at the
TSS it just built.

The boot stub still has a table, and always will: entering long mode takes a
`lgdt` and a far jump through a 64-bit code descriptor, both before any compiled
code exists to do them. What changed is that it is now **three entries** — null,
ring-0 code, ring-0 data — and nothing else. Enough to reach the code that
builds the real one.

That shrink is what turns the ring-3 test from a demonstration into a proof.
There is no ring-3 descriptor anywhere in the image except the one the program
writes at run time; a user task that runs at all is a user task running on the
program's table. `check_user.py` passes unchanged, which is the claim.

The TSS is `drivers/tss.lk`, and it is one field wearing a hundred bytes of
history: `rsp0`, the stack the CPU switches to when an interrupt takes the
machine from ring 3 back to ring 0. The scheduler writes it on every switch —
two user tasks sharing one kernel stack would have the second one's interrupt
frame land on the first one's — so the board calls back into the program for
that one word.

The selectors travel the same way. `program.lk` defines the table, so it is the
one place that knows what is at 0x18 and 0x20; `src/user.rs` asks it rather than
naming them again. Two answers to that question that disagreed would mean an
`iretq` into a segment other than the one intended, and if that segment happened
to be a ring-0 descriptor there would be no ring boundary at all.

### The second wall, which was also the compiler's

The first was constants; this one was declarations. Publishing a top-level `fn`
is `LoadFunction r; SetGlobal r, slot`, and the register is dead the instant the
store lands — but it used to be a fresh register every time. `program.lk` with
its drivers bundled in declares **236 functions**, out of the 256 a `u8`
register field allows, so the top level had about twenty registers left for
everything else. Adding the syscall constants used them up, and the error named
a constant.

The fix is one register shared by every declaration, rather than a register
recycled after each. The difference is not stylistic. Recycling — handing it
back for anything to use next — is wrong here for a reason outside the bytecode
compiler entirely: **the AOT lowering tracks what a register means keyed by
`(block, register)`, with no notion of time.** A register that once held a
function value keeps that meaning, so a later `SetGlobal` from it reads as
declaration bookkeeping and gets elided — a global write silently dropped.

That was not hypothetical. The recycling version was written first, and it made
`program.lk` stop lowering natively with "returns disagree on the value type" —
the same stale-meaning problem surfacing as a signature conflict instead. A
register that only ever holds a function value being published cannot have any
of that happen to it, because its meaning never changes. There is a test for the
shape, not just the outcome, and a TODO on the lowering.

### The first wall, which was the compiler's

Adding the GDT and TSS constants made `program.lk` stop compiling:

```
Compiler global dst register 256 exceeds u8 encoding
```

Registers are `u8` in the instruction encoding, so a function has 256 of them
and the top level is a function. Every global-backed top-level binding kept one
register permanently, as a *cache* of the global slot it had just been written
to — worth having, and unconditional. `program.lk` plus the drivers bundled into
it declare 256 constants between them; none of the files is anywhere near
unusual, and the error named whichever constant was added last.

The cache now has an eviction rule, which a cache should have had. Past 128
registers a top-level binding is only a global: reads cost a `GetGlobal` and the
register goes back. Nothing about the meaning changes — the value was already in
the global slot, which is the one place a *function* could ever see it from.

Two details are load-bearing. The question is asked *before* the initializer is
lowered, because the register file runs out on the temporaries of the statement
after the last binding rather than on the binding itself, and a check that comes
afterwards still overflows — which is how the first attempt failed. And the
limit is half the file rather than all of it, so what is left is one statement's
working set. Below 128 nothing changes at all, which is why the benchmark
workloads (five top-level bindings) emit byte-identical code.

## Ring 3

Everything else here runs at ring 0, where a wrong address is a fault and a
right one is whatever the hardware does. That was fine while all the code was
the kernel's own — and stopped being fine the moment the kernel started running
*programs*, because "the program cannot touch the framebuffer" was a fact about
the program.

```
>user
ring3
USER
!! exception #PF page fault vector=…e error=…7 rip=…10062e cr2=0000000000300000
```

`USER` is printed a byte at a time by ring-3 code through `int 0x80`, the only
gate with DPL 3. What follows is the same program writing to `0x300000` — the
shared page every interrupt handler uses — and the CPU refusing, with an error
code whose bit 2 says the access came from ring 3.

Three structures had to exist first, and two of them fail as a triple fault
rather than an error when they do not:

- a **TSS**, because the CPU needs a ring-0 stack to switch to when an interrupt
  arrives during ring 3. Without `rsp0` it pushes the interrupt frame onto the
  *user* stack, which the user can then rewrite. Its `io_map_base` points past
  the segment on purpose: a bitmap that starts beyond the limit means "no ports
  at all", where zero would point at the TSS's own fields and make a permission
  map out of whatever was there.
- **ring-3 descriptors**, because privilege is a property of the segment.
- a **user-accessible page**. This one produced the first failure: the ring-3
  program could not execute at all, faulting on its own first instruction with
  error code 5 — a user-mode access to a page the tables do not mark user. The U
  bit has to be set at *every* level of the walk, because the CPU takes their
  conjunction.

What ring 3 is granted is **its own pages, and nothing else**. The first 2 MiB
has 4 KiB granularity — one page table instead of one big page — and the U bit
is set only on the pages between `__user_start` and `__user_end`, a section the
linker script page-aligns at both ends. Everything else in that range, which is
most of the kernel, stays kernel-only.

That granularity is what the check is aimed at. The forbidden access is a *read*
of `0x100010` — the kernel's first instruction, in the same 2 MiB as the user
program. While the range was one user-accessible page, that read succeeded and
told nobody; now it faults. "Ring 3 cannot reach the shared page two megabytes
away" is a much weaker claim than "ring 3 cannot reach the kernel", and only the
second one is worth making.

Two mistakes on the way, both instructive. An early version set the U bit on the
second directory entry as well, and the forbidden write simply succeeded — which
is why the check asserts the *error code*, not just the address: a fault there
from ring 0 would be a kernel bug with the same `cr2`. And the U bit has to be
set at every level of the walk, because the CPU takes their conjunction; a user
page under a kernel-only directory is still kernel-only.

### An address space of its own, built by the program

`program.lk` builds it, out of pages from its own allocator:

```lk
fn build_user_space(stack_physical: Int) -> Int {
    let pml4 = page_alloc(SHARED_PAGES);
    …
    table_set(pdpt, index_of(stack_virtual, SHIFT_PDPT), directory, shared);
```

Every index is *computed from the virtual address* rather than written down. The
old version had `pdpt.add(1)`, `pd.write(…)`, `pt.write(…)` — 1, 0 and 0, all
correct, and correct only because the stack happens to sit at the bottom of the
second gigabyte. Numbers like that go on looking right after they stop being
true.

Two things follow from the pages coming out of the allocator. The linker script
no longer reserves four sets of tables, so the ceiling on how many address
spaces there can be is "how much memory is left" rather than a number nobody
justified — the previous one was four, and four was only there because two had
been. And `check_shell.py`'s expected page addresses moved by eight pages, which
is the two spaces being allocated like anything else; that check asserts the
*addresses* precisely so that a counter pretending to be an allocator would not
pass it.

### The failure that was not one

Removing the linker reservations made `check_spawn.py` report both the spinner
and the clock windows frozen — three runs in a row, deterministic-looking, and
the reservation being the only difference. Restoring it passed. Doubling the
kernel stack instead also passed. The obvious story was a stack that had been
overflowing into 64 KiB of unused reservations and now overflowed into `__pt0`.

It was none of that. The same build passes 4/4 on an idle machine. Every one of
those failures happened with a `cargo build` running alongside, and
`check_spawn` is the one check here that is wall-clock: four screenshots 1.4 s
apart, asking whether a window that changes once a slice ever changed. A starved
guest makes them all land in one phase.

Recorded here and in the check itself because roughly forty minutes went into a
hypothesis that a second look would have killed — and because the next person
reading a red `check_spawn` should re-run it on a quiet host before believing
it.

### An address space of its own

The ring-3 task's stack is at `0x4000_0000` — *in its own address space*. In the
kernel's, that address is identity-mapped RAM that does not exist on this
machine. Two spaces, not one with extra permissions, and the difference between
a thread and a process is the one word per task that says which.

Almost all of it is shared, and shared by *pointing* rather than copying: the
task's PML4 names the kernel's own page directories for three of the four
gigabytes. The kernel has to be mapped in every space — an interrupt during
ring 3 lands in kernel code, and there would otherwise be nowhere for it to go —
and copying the entries would work today and drift the first time a mapping is
added to one and not the other. Only the second gigabyte is the task's own, and
it holds one page: its stack.

There are two of them, which is what makes it a claim rather than a permission:
both tasks keep a stack at *the same* virtual address, each writes one letter
into it, and each prints what it reads back — for ever, interleaved by the
timer. `ABABAB…`. One address space would mean the second write landed on the
first's page and both letters were the same from then on.

That it works at all is the other half of the evidence. QEMU's default machine
has 128 MiB, so `0x4000_0000` is backed by nothing in the kernel's identity map;
a task running there means the tables that give it meaning are the ones in
force.

CR3 changes before the stack pointer is handed back, not after: the value the
switch returns is read by the CPU *after* this returns, and it has to mean the
same thing in whichever space is current by then. It does, because the kernel is
mapped identically in both — which makes the order safe rather than lucky.

### Whose list of holes it is

The syscall dispatcher is `program.lk`'s. What a user task may ask for is a list
of holes in the wall the ring boundary just built, and deciding what goes on
that list is not the board's business — nor is deciding whether to believe a
pointer:

```lk
fn user_range_is_valid(address: Int, length: Int) -> Bool {
    if (length <= 0 || length > MAX_WRITE) { return false; }
    let end = address + length;
    if (end < address) { return false; }
    return address >= user_section_start() && end <= user_section_end();
}
```

An `Int` is signed, which does half the work for free: an address with its top
bit set arrives negative and fails the lower bound. The other end still needs
the wrap check — an address just under the maximum wraps `address + length` to a
negative number, which would then pass `end <= limit`. LK's addition wraps
rather than trapping, so the wrap is what that comparison looks for.

The board answers where the user section is, because the linker is the only
thing that knows and the board is the only side that can ask it.

**The syscall trampoline now saves the SSE registers**, which it did not before.
The handler is compiled LK, LK numbers are `f64`, and the System V ABI lets a
called function clobber every XMM register — which the ring-3 caller never
agreed to. Nothing would have gone wrong yet: the user programs here are
assembly that touches no XMM at all. That is exactly why it is worth writing
down rather than waiting for the first one that does.

### A pointer the kernel does not believe

The first syscall took a byte per call, which was slow and deliberate: a pointer
from ring 3 is a *number*, and following one without checking is the shape of
every "the kernel read out its own memory on request" bug there has ever been.

There is a checked one now. `write(ptr, len)` verifies the range lies inside the
user section — the only memory ring 3 can reach — before reading a byte of it,
and the check is the kernel's, not a promise the caller makes. The arithmetic is
checked too, because a length near `u64::MAX` wraps the end back below the start
and makes any address look contained.

The user program does both: it prints `str` through the checked call, then hands
the same call the kernel's own address and prints what came back. `N` means
refused. A `Y` there would mean the boundary is decoration.

The kernel copies each byte out before using it rather than printing from user
memory in place. One instruction shorter would leave a window between the check
and the use — on one CPU a small one, on two a race.

### And back again

`user` has no way back — its only exits are a syscall (which returns *into* ring
3) and a fault. A ring-3 *task* does: one is spawned at boot, prints `3` for
ever, and never yields. The shell answers a command while it runs, which is the
claim: the timer took the CPU away from ring 3 and gave it back.

What that needed was one line in the scheduler and a stack per task. The frame a
task starts on is the same shape either way — fifteen saved registers under the
frame the CPU pushes — and the only difference between a kernel task and a user
one is the four numbers in it. What is *not* the same is where an interrupt from
ring 3 lands: the CPU takes that from the TSS, so `rsp0` is set to the next
task's own kernel stack on every switch. Two user tasks sharing one would have
the second's interrupt frame land on the first's, and the first would resume
into whatever was left.

### What a third one would need

The tables are there for four address spaces and the task table holds six tasks,
so a third user process costs nothing structural. What it needs is a *claim*: two
tasks printing `A` and `B` prove they cannot see each other, and a third printing
`C` proves nothing further unless it is arranged to fail differently — sharing a
space with exactly one of the others, say, so the check can tell "isolated from
everyone" from "isolated from the last one spawned".

Adding the task is half an hour. Deciding what it would demonstrate is the part
worth doing first.

## Memory that comes back

`drivers/pages.lk` never reclaims, which was honest while nothing freed.
Something does now — a shell that reads files, a program the kernel runs,
anything that lives for less than the machine does — and a bump allocator meets
that with "out of memory" while holding a megabyte of dead blocks.

`drivers/heap.lk` is a free-list allocator written in LK. Every block, free or
in use, stays in one address-ordered chain, and the chain lives *in the blocks*
— there is no side table because there is nowhere to put one: an allocator
cannot allocate.

```
>heap
blocks 1/1 reused 1 used 0
```

Three numbers, three claims. `heap` takes three blocks, frees the middle one,
takes one that only fits the hole, then frees everything:

- **the block count comes back** to what it started at, which only happens if
  freeing joins holes on *both* sides. Joining forward only is the tempting
  shortcut — it is half the code — and it leaves a heap that fragments in one
  direction and never recovers;
- **the hole was reused** rather than bumped past;
- **the byte count balances** to zero.

Two decisions worth stating, because both are the kind that look like
oversights:

- **Address-ordered, singly linked, first fit.** Freeing is therefore O(n) in
  the number of blocks: the block *before* one being freed has to be found by
  walking. Size buckets and a doubly-linked list would fix that, at four times
  the code, and their win starts where a kernel has thousands of live blocks.
  This one is nowhere near, and the check above is what will say when it is.
- **The arena is taken from the page allocator at startup**, not fixed at an
  address here. An address written into a source file is one nothing else can
  be told about — the same collision the shared page's chain of constants
  prevents, one layer out.

## A filesystem, of sorts

`drivers/tarfs.lk` reads files by name off the disk. tar is chosen for what it
does **not** need: no allocation, no cache, no free list, no write path, no
in-memory index. An entry is a 512-byte header followed by its contents rounded
up to whole sectors, and the next entry follows it — so finding a file is
walking that chain on the medium, and reading one is reading sectors.

```
>cat hello.txt
HELLO FROM DISK
SECOND LINE
```

There is no open file, no descriptor, and no buffer beyond the single sector the
program owns: `print_file` walks the headers, then reads the contents a sector
at a time and prints as it goes. A file larger than memory is therefore not a
problem. A file in a directory is: tar has no directories here, a name is
matched whole, and search is linear. All three of those want data structures,
and data structures want an allocator.

The archive `check_disk.py` builds comes from Python's `tarfile`, so what the
kernel walks is a real archive written by something else — not a layout invented
to be easy to parse.

## The kernel runs a program it was not built with

The kernel is compiled LK. It now also **hosts an interpreter** for LK:

```
>run sq.lk
SUM OF SQUARES
385
```

`sq.lk` is on the disk, not in the image. The shell finds it through
`drivers/tarfs.lk`, copies it into a staging area, and calls `kernel_run` — an
`#[extern]` the board implements in `src/main.rs`, which parses, type-checks and
runs it on the no_std bytecode VM. Everything it prints comes back through
`lk_console_byte`, an `#[export]`ed LK function, so a program the kernel started
scrolls in the same window as the shell that started it.

Both directions of the boundary are therefore in use at once, which is what they
were built for: the board asks the program for a console, and the program asks
the board for an interpreter.

Two consequences worth stating:

- **The image is ten times the size it was** (150 KB → 1.5 MB): the whole front
  end — lexer, parser, type checker, compiler, VM — is now in it. `link.ld`
  asserts that it still stops below the shared page at `0x300000`, because
  growing past that would not fail to build and would not fail to boot; it would
  quietly put the key handler's line buffer on top of the kernel's own data.
- **Memory is now a map, not a habit.** Three things want RAM and none can ask
  for it: the image, the interpreter's heap, and the LK page allocator. The map
  is written down in `src/main.rs` and the numbers are agreed in `program.lk` —
  two allocators on one machine agree by arrangement or not at all.

Bad input is a report, not a crash: `run nope.lk` says `no file`, and a program
that does not parse comes back as `failed 3` — the stage, because a parse
failure and a runtime failure want different next steps. A kernel that dies on a
bad file is not one you can put a disk in.

## Sharing state between them

Read, add, write is three steps. An interrupt landing between the read and the
write discards whatever happened in between, and with a task switch inside that
interrupt the window is wide. `drivers/lock.lk` is the critical section:

```lk
let state = lock();
shared_bump(SHARED_PAIR_A);
shared_bump(SHARED_PAIR_B);
unlock(state);
```

Masking interrupts is enough here *because the scheduler runs inside the timer
handler* — stopping interrupts stops a task switch. That is also the assumption
that breaks first: on a second core the other CPU keeps running and this
protects nothing. The file says so, and every caller already goes through it,
so that is where a spinlock would go.

`sync` reports two counters that the timer handler and the spinner both bump
under one section. They can only differ if an increment read a stale value, so
`check_shell.py` requires them equal. Removing the lock and rerunning is worth
doing once: they diverge within seconds — and the counts nearly double, which
is what the section costs.

`check_tasks.py` screenshots the corner four times and requires the glyph to
change. On the serial line, interleaved output would show that both tasks
*ran*; only the screen shows that one was interrupted mid-work.

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
| the framebuffer | volatile MMIO | sets a mode over the Bochs VBE ports, clears it, draws text, and scrolls by panning the view rather than moving pixels |
| the PS/2 keyboard | port I/O, from an interrupt handler | reads the scancode, decodes it, echoes the character to the serial line and draws it at the cursor |
| the PS/2 mouse | port I/O on the same controller, IRQ12 through the cascade | configures the auxiliary port, assembles three-byte packets, publishes a position and button state |
| an IDE disk | port I/O with status polling | IDENTIFY for the geometry, then reads and writes 512-byte sectors, with bounded waits |

`check_screen.py` screenshots the machine through QEMU's monitor and checks the
pixels. That is a separate claim from the `pixels …` line: reading the
framebuffer back proves the writes reached the device's memory, but an
unconfigured card accepts those too. Only what QEMU scans out shows that the
mode was actually set.

## The drivers are modules

```
drivers/idt.lk           the interrupt descriptor table: gates, and `lidt`
drivers/pic.lk           the 8259 pair: remap, mask, end-of-interrupt
drivers/gdt.lk           segment descriptors, `lgdt`, and reloading CS
drivers/paging.lk        four-level page tables, CR3, and the TLB
drivers/tss.lk           the one field long mode kept: the ring-0 stack
drivers/serial.lk        a 16550 UART
drivers/pci.lk           configuration space
drivers/vbe.lk           the Bochs VBE display interface, including panning
drivers/framebuffer.lk   pixels, given a base and a stride
drivers/keyboard.lk      the PS/2 controller
drivers/pit.lk           the interval timer
drivers/ata.lk           an IDE disk, PIO mode (read, write, IDENTIFY)
drivers/tarfs.lk         read-only tar, straight off sectors
drivers/heap.lk          a free-list allocator: alloc, free, and coalesce
drivers/mouse.lk         the PS/2 mouse: packets, buttons, and a position
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

**The table those interrupts come through is LK's too.** `drivers/idt.lk`
builds all 256 gates and loads them with `cpu_load_idt`; `program.lk` decides
which vector means what:

```lk
idt_set_gate(IDT_BASE, VECTOR_KEYBOARD,
             unsafe { symbol_address("__keyboard_trampoline") }, KERNEL_CODE_SELECTOR, 0);
```

That is not a rewrite for its own sake. A gate is sixteen bytes of *decision* —
which vector, which handler, and which privilege level may raise it — and the
only reason it used to be Rust is that there was no way to say `lidt` in LK.
There is now, so the decisions live where the rest of the program's decisions
do. The board's remaining share is the thing this genuinely cannot be: the
trampolines. An interrupt is not a call — the code it lands in never agreed to
lose its caller-saved registers — so a compiled handler has to be entered
through a stub that spills all of them and leaves with `iretq`. That is
assembly in any language.

**The 8259 is LK's too**, both halves of it — `drivers/pic.lk` holds the
four-write initialisation sequence, the mask register, and the end-of-interrupt.
Those had to move together: bringing the chip up decides which vector each line
lands on, acknowledging decides which chip is told the handler is done, and
split across the boundary they become two files naming one command port.

The acknowledgement is a *wrapper* around each handler rather than its last
line:

```lk
#[export("lk_key_isr")]
fn isr_key() {
    on_key();
    pic_eoi_master();
}
```

`on_key` returns early from four places. An end-of-interrupt a `return` can skip
is one that will be skipped — and the symptom is a device that works once and
then goes silent, which is what an unacknowledged 8259 line does. Written this
way there is nowhere for it not to happen.

Stopping is the program's as well. `program.lk` masks the flag and then the
chip as its last act, in that order: the flag stops the CPU taking anything, the
chip stops it raising anything, and a tick landing between the two would splice
a `.` into the line the board prints afterwards.

What is left in `src/interrupts.rs` is the trampolines and the exception
reporter — the reporter because it runs *after* something has gone wrong, and
the two things a handler must never do (allocate, take a lock) are exactly what
formatting a report in LK would need.

What a tick *means* has always been the program's, including programming the
PIT's divisor, which `program.lk` does with the same `port_out_u8` its UART
driver uses.

### The stride that was derived wrong

The 32 exception stubs are a `.rept` in the board's assembly, padded to a fixed
stride, and `program.lk` computes a stub's address from it. It asks the
assembler for the stride rather than naming 16 — `(ISR_STUBS_END - ISR_STUBS) /
32` — because a copy of that number on the LK side is a copy nothing checks.

Deriving it was right and the derivation was wrong. `.align 16` sits at the
*top* of each iteration, so the array ended nine bytes into its final slot:
span 505, stride 15, and every gate but the first pointed into the middle of the
stub before it. The `fault-probe` build reported a page fault as **vector 2**,
with the faulting address sitting in the error-code field.

Two fixes, and the second is the useful one. The assembly now pads after the
last stub as well. And `program.lk` checks that the span divides by 32 before
dividing, which costs one modulo at boot and is the thing that would have said
so out loud:

```lk
if ((span % EXCEPTION_COUNT) != 0) {
    uart_write(/* "bad isr stride" */);
    halt();
}
```

### Which comes first

The program installs its table before anything can fault, and the board no
longer touches interrupts at boot at all — `kernel_main` calls `main()` and the
program asks for the PIC when it is ready. Between the two there is a window
with no gate for any vector, and a fault with no gate is a triple fault, which
on this machine is a silent reset.

That window is why the deliberate fault moved. It used to be a `write_volatile`
in `kernel_main`, which is now *before* the table exists — the probe stopped
reporting anything and the machine simply reset, which is exactly the failure
the probe exists to make impossible. It is an `#[extern]` the program calls
immediately after installing the table, so what it lands in is the table the
program actually built.

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
