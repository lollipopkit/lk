# LK as native machine code, with no OS

The demo next door (`bare-metal/`) runs LK's **bytecode VM** on a board. This
one runs LK's **compiled output**: `build.rs` calls `lk compile object:<triple>`
and the linker places the resulting aarch64 object beside `lkrt`. What executes
is real instructions, not a dispatch loop.

```bash
rustup target add aarch64-unknown-none
cargo build -p lkrt-cabi -p lk-cli --features aot   # from the repo root
cd bare-metal-native
LK_BIN=../target/debug/lk cargo build --release
```

The result:

```
$ file target/aarch64-unknown-none/release/lk-bare-metal-native
ELF 64-bit LSB executable, ARM aarch64, statically linked

$ aarch64-linux-gnu-objdump -d … | grep -A4 '<lk_fn_1>:'
lk_fn_1:                          # program.lk's fib
  cmp   x0, #0x2                  #   if (n < 2)
  b.lt  233870
  sub   x0, x14, #0x1             #   fib(n - 1)
```

`main` is the LK entry point; the other functions stay local `lk_fn_N`. The
`.text` here is ~145 KB against ~705 KB for the interpreter image, because none
of the front end or the dispatch loop is present.

## Booting

`link.ld` places the image at `0x4008_0000` — where the Arm Linux boot protocol
puts a kernel, and where QEMU's `-kernel` jumps. `boot.rs` is the reset path: it
parks every core but the first, sets a stack, zeroes `.bss`, and calls
`kernel_main`.

```bash
qemu-system-aarch64 -M virt -cpu cortex-a53 -display none -serial stdio \
  -kernel target/aarch64-unknown-none/release/lk-bare-metal-native
```

```
native LK drives the PL011: sum(fib(0..9)) = 88
88
[lk returned to the board, status 0000000000000000]
```

The first line is written by `program.lk`'s own PL011 driver — LK code, compiled
to aarch64 instructions, doing `volatile_write_u32` against `0x0900_0000`. The
`88` after it is the script's result value, echoed by `lkrt` through the sink
`kernel_main` installs; both reach the same device by different routes.

## What the reset path has to do before compiled code is legal

Three things, each of which showed up as a fault rather than as a warning:

**Enable the MMU.** With the MMU off, every access is Device-nGnRnE, and Device
memory does not support the exclusive instructions that back atomics — so the
first lock or atomic counter aborts. The abort is reported as an *alignment*
fault (`ESR_EL1` = `0x9600_0021`), which points at the wrong thing entirely.
`boot.rs` installs a four-entry identity map: peripherals Device, RAM Normal
write-back.

**Clear `CPACR_EL1.FPEN`'s trap.** Floating point and SIMD are trapped at reset
(`ESR_EL1` EC = `0x07`), on the assumption that an OS wants to know before it
has to save those registers. LK numbers are `f64`, so a program faults on its
first arithmetic without this.

**Install a vector table.** `VBAR_EL1` is 0 at reset, where nothing is mapped,
so any fault becomes a jump into unmapped memory and the board simply stops.
The table here reports `ESR_EL1`/`FAR_EL1`/`ELR_EL1` over the UART and halts —
which is how the two faults above were identified rather than guessed at.

## Why the pieces are shaped this way

**`lk compile object:` emits an object, not an executable.** The linker script,
entry point and memory map belong to the board. Emitting a relocatable object
lets an existing embedded build place it, the same way a C library is consumed.

**`lkrt` (rlib) rather than `lkrt-cabi` (staticlib).** A `staticlib` must be
self-contained — its own allocator and panic handler — which a bare-metal binary
supplies itself. When those were one crate, depending on it made cargo build the
staticlib too, and that failed. They are separate crates for exactly this.

**`link-dead-code` in `.cargo/config.toml`.** `lkrt`'s `#[no_mangle]` functions
are called from the Cranelift-emitted object, which the linker sees as a
separate input; nothing on the Rust side references them, so they would be
dropped. The hosted path solves this by force-loading `liblkrt_cabi.a`, and an
rlib dependency has no equivalent.

**`lkrt::link_anchor()` in `_start`.** Without *some* reference, cargo treats
`lkrt` as an unused dependency and its rlib never reaches the link line at all —
`link-dead-code` cannot keep code that was never linked.
