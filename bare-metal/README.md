# LK on bare metal

A bootable Cortex-M image that parses, type-checks and runs an LK program with
**no OS and no std anywhere in the crate graph**. It exists to keep the no_std
claim honest: `cargo build -p lk-core --no-default-features` only proves the
crate *compiles*, and no differential test can tell you whether the VM actually
*works* without std.

```bash
rustup target add thumbv7em-none-eabi
cargo run --release              # the runner in .cargo/config.toml starts QEMU
```

Expected output, and an exit code of 0:

```
sum(fib(0..9)) = 88
OK: lk ran on bare metal, returned 88
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

Measured on this demo (`opt-level = "z"`, LTO, `panic = "abort"`):

| section | size |
| --- | --- |
| `.text` | 470 KB |
| `.rodata` | 91 KB |
| **flash total** | **~561 KB** |

`.bss` is dominated by the 1MB demo heap, which is a knob in `main.rs`, not a
requirement.

That figure is the *whole front end* — tokenizer, parser, macro machinery, type
checker, VM compiler and executor. Running a precompiled `ModuleArtifact`
instead, so the front end can be dropped, measured ~375 KB in a separate probe.
Neither fits a 256KB-class MCU; both fit an STM32H7/F7-class part. Shrinking the
artifact-only path further means replacing its `serde_json` decode with a
binary format — see task 6 in the no_std work.

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
