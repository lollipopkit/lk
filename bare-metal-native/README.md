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

## What it does not yet do

**It links, but it does not boot.** There is no linker script and no startup
code, so nothing sets up a stack, a vector table or a memory map — all of which
are properties of the board rather than of LK. `_start` is a plain symbol here,
not something a reset vector reaches. Running this on hardware or in QEMU needs
those pieces added the way `bare-metal/` does for Cortex-M (a `memory.x` and
`cortex-m-rt`), which is deliberately left to the board's own build.

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
