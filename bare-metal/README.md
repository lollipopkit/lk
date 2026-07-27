# LK on bare metal

A bootable Cortex-M image that parses, type-checks and runs an LK program with
**no OS and no std anywhere in the crate graph**. It exists to keep the no_std
claim honest: `cargo build -p lk-core --no-default-features` only proves the
crate *compiles*, and no differential test can tell you whether the VM actually
*works* without std.

```bash
rustup target add thumbv7em-none-eabi
# `.lkm` files are not checked in (see the repo .gitignore), and the
# artifact_only image embeds one at compile time. Run this once, from the repo
# root, before the first build:
cargo run -p lk-cli --no-default-features --features stdlib -- \
  compile bytecode bare-metal/demo.lk

cd bare-metal
cargo run --release              # the runner in .cargo/config.toml starts QEMU
```

Expected output, and an exit code of 0:

```
LK drives hardware
....................................................................
OK: lk ran on bare metal, returned 0
```

That first line does not come from semihosting. It comes from `uart.lk` — a
UART driver written in LK — configuring the board's CMSDK APB UART, polling its
status register until the transmit buffer drains, and sending the bytes. The
text arriving on the serial line is the proof that volatile MMIO from LK reaches
real hardware:

```lk
fn uart_putc(byte: Int) {
    let full = 1;
    while (full != 0) {
        let state = unsafe { volatile_read_u32((UART0_BASE + REG_STATE) as *mut u32) };
        full = state & STATE_TX_FULL;
    }
    unsafe { volatile_write_u32((UART0_BASE + REG_DATA) as *mut u32, byte as u32); };
}
```

That poll is the access a non-volatile load would let the compiler hoist out of
the loop, hanging it forever.

Each `.` after it is a SysTick interrupt. The timer is armed from LK too —
reload value, counter clear, then enable with the interrupt bit set:

```lk
fn systick_start(reload: Int) {
    unsafe { volatile_write_u32(SYST_RVR as *mut u32, reload as u32); };
    unsafe { volatile_write_u32(SYST_CVR as *mut u32, 0 as u32); };
    unsafe { cpu_barrier(); };
    let csr = SYST_ENABLE | SYST_TICKINT | SYST_CLKSOURCE;
    unsafe { volatile_write_u32(SYST_CSR as *mut u32, csr as u32); };
}
```

The handler itself is Rust (`#[exception] fn SysTick`) and writes the character
straight to the UART. It does *not* call back into the VM: an interrupt can land
in the middle of any bytecode instruction, and the executor is not re-entrant.
Touching only hardware from the handler sidesteps that, which is the usual shape
for the fast half of an interrupt anyway — acknowledge, do the minimum, leave
the rest to the main loop.

The second image, `artifact_only`, runs the language-feature corpus
(`demo.lk`) from precompiled bytecode instead of source. Its output is
byte-identical to `lk bare-metal/demo.lk` on a host — including the
`libm`-computed `sqrt` and the crc32 — which is the other half of the guarantee:
swapping in `libm`, `hashbrown` and spin locks for their std counterparts must
not change what a program computes. See [Footprint](#footprint):

```bash
cargo run --release --bin artifact_only
```

The first line comes from LK's own `println`, routed through `stdlib/bare` to
semihosting. A wrong answer or a raise exits non-zero, so this works as a CI
gate.

Without the cargo runner:

```bash
qemu-system-arm -cpu cortex-m4 -machine mps2-an386 -nographic \
  -semihosting-config enable=on,target=native \
  -kernel target/thumbv7em-none-eabi/release/lk-bare-metal
```

## Footprint

Two images are built, differing only in how they get their bytecode. Measured
with `opt-level = "z"`, LTO, `panic = "abort"`:

| image | what it compiles in | flash (`.text` + `.rodata`) |
| --- | --- | --- |
| `lk-bare-metal` | full front end: tokenizer, parser, macro machinery, type checker, VM compiler, executor | **705 KB** |
| `artifact_only` | precompiled `ModuleArtifact` → executor | **560 KB** |

Both include the seven stdlib modules listed below. The front end costs
**145 KB, about 21%**. Both fit an STM32H7/F7-class part; neither fits a
256KB-class MCU.

### What each stdlib module costs

Every module in `stdlib/bare` is a cargo feature, because on an MCU flash is
the scarce resource and a board should pay only for what it imports. Measured
against a 561 KB image with no modules at all:

| module | added flash | notes |
| --- | --- | --- |
| `math` | +36 KB | `libm` supplies what std's inherent `f64` methods would |
| `encoding` | +33 KB | `json` / `base64` / `hex` only |
| `hash` | +30 KB | sha256 / sha1 / crc32 / fnv64 |
| `string` | +23 KB | |
| `iter` | +23 KB | |
| `bytes` | +12 KB | |
| `slice` | +11 KB | |
| all seven | +144 KB | |

(The individual numbers sum to more than the total because the modules share
code.) Pick a subset in your own board crate:

```toml
lk-stdlib-bare = { path = "…", default-features = false, features = ["math", "string"] }
```

Everything else is registered as present-but-unavailable, so importing one
fails with a reason rather than "unknown module". Why each is out:

- `fs`, `io`, `net`, `http`, `env`, `os`, `process`, `task`, `chan`, `stream` —
  need an OS.
- `time`, `datetime` — need a clock. A board with an RTC would have to supply
  one; `stdlib/bare` does not guess.
- `random` — needs an entropy source (`rand` wants the OS RNG). Note that
  `math.random()` *is* available, but on bare metal it is a deterministic
  sequence from a fixed seed.
- `uuid` — `parse`/`is_valid` would work, but `v4` (the reason to use it) needs
  entropy, so shipping a half-module was judged worse than shipping none.
- `regex` — the `regex` crate needs std; a no_std build means moving to
  `regex-automata`, which is a rewrite rather than a port.
- `path` — depends on `std::path` semantics (`components`, `with_extension`,
  `MAIN_SEPARATOR`) that would have to be reimplemented, for little value with
  no filesystem to address.
- `encoding.yaml` / `encoding.toml` / `encoding.url` — YAML and TOML decoding
  is std-gated in lk-core itself; `url` pulls in `idna`, which needs std.

Worth knowing: the artifact-only saving needs **no build configuration**. The
linker's `--gc-sections` drops the front end on its own once nothing references
it, which is why there is no `artifact-only` cargo feature — one would make the
build more explicit without making the binary smaller.

`.bss` is dominated by the 1MB demo heap, which is a knob in `main.rs`, not a
requirement.

Further shrinking means attacking the executor, which dominates what is left,
or replacing the artifact's JSON decode with a binary format. The latter is a
cross-cutting change to the `.lkm` format shared with the AOT path, not a
no_std-local one.

`demo.lkm` is generated from `demo.lk` (see the build steps above) and is
build-locked to the artifact version, so it must be regenerated after an
artifact bump. CI regenerates it every run rather than trusting a stale copy.

## Why MPS2 AN386

QEMU's Cortex-M boards differ mostly in memory. `lm3s6965evb` is the common
choice but has 256KB of flash, which LK does not fit in. MPS2 AN386 is a
Cortex-M4 (matching `thumbv7em-none-eabi`) with two 4MB SRAM banks, so the
board is not the constraint while the VM is under test.

## What this demo is not

- **Not a minimal footprint.** A bump allocator that never reclaims, a 1MB
  heap, and the full front end compiled in. Each is a deliberate choice to keep
  the demo about the VM.
- **Not a HAL.** Platform capabilities are swapped at the stdlib layer (see
  `stdlib/bare`), matching how `stdlib/web` does it for the browser.
- **Not the whole stdlib.** Only the computation-only modules are available;
  see the table above. `math.random()` in particular is a deterministic
  sequence here — bare metal has no wall clock to seed from, and a board with
  a real entropy source should seed the VM itself.
- **Not part of the workspace.** It only builds for a bare-metal target and
  pulls in `cortex-m-*`, so the root `Cargo.toml` excludes it. Build it from
  this directory.
