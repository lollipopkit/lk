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
sum(fib(0..9)) = 88
OK: lk ran on bare metal, returned 88
```

There is a second image, `artifact_only`, which runs the same program from
precompiled bytecode instead of source — see [Footprint](#footprint):

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
| `lk-bare-metal` | full front end: tokenizer, parser, macro machinery, type checker, VM compiler, executor | **560 KB** |
| `artifact_only` | precompiled `ModuleArtifact` → executor | **416 KB** |

So the front end costs **144 KB, about 26%**. Both fit an STM32H7/F7-class
part; neither fits a 256KB-class MCU.

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
  `stdlib/bare`), matching how `stdlib/web` does it for the browser. Everything
  needing an OS — `fs`, `net`, `env`, `time`, `task`, … — is registered as
  present-but-unavailable so an import fails with a reason instead of
  "unknown module".
- **Not part of the workspace.** It only builds for a bare-metal target and
  pulls in `cortex-m-*`, so the root `Cargo.toml` excludes it. Build it from
  this directory.
