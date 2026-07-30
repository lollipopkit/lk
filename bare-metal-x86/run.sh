#!/usr/bin/env bash
# Build the image and boot it under QEMU.
#
# The objcopy step is not incidental: QEMU's multiboot loader only accepts
# ELF32, while the code is 64-bit. Converting the class (rather than the code)
# is what real kernels do — every address in the image is below 4 GiB, so
# nothing is lost.
set -euo pipefail
cd "$(dirname "$0")"

BIN=target/x86_64-unknown-none/release/lk-bare-metal-x86
OBJCOPY=${OBJCOPY:-llvm-objcopy}

# A feature build gets its own image name. `CARGO_FLAGS=--features=fault-probe`
# builds a kernel that page-faults on purpose, and this wrote it to the one path
# every `check_*.py` boots — so the next check reported `#PF page fault` as a
# regression, and bisecting that is hopeless because every revision "fails".
# `kernel.py` builds its own image now, which closes the same hole from the
# other side; this keeps the file name from lying about what is in it.
IMAGE="$BIN.multiboot"
if [ -n "${CARGO_FLAGS:-}" ]; then
    IMAGE="$BIN.$(printf '%s' "${CARGO_FLAGS}" | tr -cs 'a-zA-Z0-9' '-').multiboot"
fi

LK_BIN=${LK_BIN:-../target/debug/lk} cargo build --release ${CARGO_FLAGS:-}
"$OBJCOPY" -O elf32-i386 "$BIN" "$IMAGE"

# `isa-debug-exit` ends the machine when the kernel writes to port 0xf4, so
# this returns instead of needing a timeout to decide it is finished. Its exit
# code is `(value << 1) | 1`, so a clean run is 1.
exec qemu-system-x86_64 -kernel "$IMAGE" -display none -serial stdio \
    -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
    -device edu "$@"
