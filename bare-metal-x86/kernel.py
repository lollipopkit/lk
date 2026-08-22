"""The kernel image the `check_*.py` scripts boot — built from the tree they run in.

Every one of them booted `target/x86_64-unknown-none/release/*.multiboot`, and
not one of them put it there. The path was a bare default, so a script tested
whatever happened to be on disk. Both ways that lies have happened here:

* **The file is absent.** QEMU exits before it opens its monitor socket, and the
  script dies in `connection.connect(monitor)` with `ConnectionRefusedError` —
  a message about a socket, nowhere near the cause.
* **The file is a `fault-probe` build.** `CARGO_FLAGS=--features=fault-probe
  ./run.sh` builds a kernel that touches 0x900000000 on purpose, to prove the
  exception reporter names a fault; `run.sh` used to objcopy it over the very
  path every check boots. The next check then reported `#PF page fault` as a
  regression, and bisecting it is hopeless: every revision "fails", because no
  revision is what is running.

So the image is an *output* of these scripts, not an input. `cargo build` with
default features is a fast no-op when nothing changed and rebuilds when a
feature set differs, which is what makes the second case above unreachable
rather than merely unlikely.

Passing a path explicitly (`python3 check_pci.py IMAGE`) still boots that file
as given — that is for testing an image from somewhere else, and it is the
caller's business whether it matches the tree.
"""

import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.abspath(__file__))
BIN = os.path.join(ROOT, "target/x86_64-unknown-none/release/lk-bare-metal-x86")
# The compiler that turns this crate's `.lk` files into native code. `build.rs`
# falls back to whatever `lk` is on PATH, which is how a two-day-old installed
# binary once got credit for a fix that was never compiled — so this names the
# repo's own build and refuses if it is missing.
LK = os.path.join(ROOT, "../target/debug/lk")


def kernel_image(argv=None):
    """Build the image and return its path, or return `argv[1]` untouched."""
    argv = sys.argv if argv is None else argv
    if len(argv) > 1:
        return argv[1]

    # An explicit `LK_BIN` names a specific compiler and is used as given; only
    # the default is checked, because that is the one nobody chose.
    compiler = os.environ.get("LK_BIN") or LK
    if not os.environ.get("LK_BIN") and not os.path.exists(LK):
        sys.exit(
            f"{os.path.relpath(LK, ROOT)} is missing: the kernel's LK sources are compiled by\n"
            "the repo's own `lk`, not by whatever is installed on PATH. Build it first:\n"
            "    cargo build -p lk-cli --features aot"
        )

    environment = dict(os.environ, LK_BIN=os.path.abspath(compiler))
    # This crate's `.cargo/config.toml` carries `relocation-model=static` and
    # the SSE settings in its own rustflags table, and an inherited `RUSTFLAGS`
    # would replace that table rather than add to it — including an empty one.
    environment.pop("RUSTFLAGS", None)
    subprocess.run(["cargo", "build", "--release"], cwd=ROOT, env=environment, check=True)
    # QEMU's multiboot loader only accepts ELF32, while the code is 64-bit;
    # `run.sh` explains why converting the class loses nothing.
    objcopy = os.environ.get("OBJCOPY", "llvm-objcopy")
    subprocess.run([objcopy, "-O", "elf32-i386", BIN, f"{BIN}.multiboot"], check=True)
    return f"{BIN}.multiboot"
