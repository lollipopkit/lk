#!/usr/bin/env bash
# Builds an AddressSanitizer-instrumented lkrt static library and points the
# native-compile pipeline at it via LKRT_STATICLIB (deep-coverage plan F1).
#
# Motivation: `LK_NATIVE_SANITIZE=address` only instruments the clang-compiled
# generated IR; lkrt's own Rust code (the unsafe pointer surface) stayed
# uninstrumented — heap overflows were caught via malloc interposition, but
# stack/global/UAF inside lkrt were not. Rebuilding lkrt with
# `-Zsanitizer=address` (nightly) closes that gap. `std` itself stays
# uninstrumented: `-Zbuild-std` currently trips E0152 (duplicate `core` lang
# items against the sysroot) in this workspace, and mixing an instrumented
# crate with an uninstrumented std is a supported ASan configuration — lkrt's
# own unsafe surface is what needs the redzones. UBSan has no Rust-side equivalent (`-Zsanitizer` offers no
# `undefined`); lkrt's UB surface is covered separately by Miri
# (`make miri-lkrt`).
#
# Usage:
#   bash scripts/build_lkrt_asan.sh                # build the library
#   source <(bash scripts/build_lkrt_asan.sh env)  # also export LKRT_STATICLIB
#   LKRT_STATICLIB=... LK_NATIVE_SANITIZE=address \
#     cargo test -p lk-cli --test aot_differential_test
#
# Note: the nightly rustc ASan runtime and the system clang ASan runtime must
# be ABI-compatible (usually fine on matching LLVM major versions); this is a
# non-blocking correctness harness, not a PR gate.
set -euo pipefail

TARGET="${TARGET:-x86_64-unknown-linux-gnu}"
TARGET_DIR="${TARGET_DIR:-target/lkrt-asan}"

RUSTFLAGS="-Zsanitizer=address" cargo +nightly build \
    -p lkrt \
    --release \
    --target "$TARGET" \
    --target-dir "$TARGET_DIR" 1>&2

LIB="$TARGET_DIR/$TARGET/release/liblkrt.a"
if [ ! -f "$LIB" ]; then
    echo "error: expected $LIB after the build" >&2
    exit 1
fi

# `lk-api` too, with the *same* toolchain. A hybrid program links both archives,
# each of which statically links `std`; two different rustc versions therefore
# both define `rust_eh_personality` and the link fails. Same nightly for both
# means the archives carry the identical `std` member, which the linker pulls
# once. (No sanitizer flags here — instrumenting `lkrt` is the point; partial
# instrumentation is fine, mismatched toolchains are not.)
cargo +nightly build \
    -p lk-api-cabi \
    --release \
    --target "$TARGET" \
    --target-dir "$TARGET_DIR" 1>&2

API_LIB="$TARGET_DIR/$TARGET/release/liblk_api_cabi.a"
if [ ! -f "$API_LIB" ]; then
    echo "error: expected $API_LIB after the build" >&2
    exit 1
fi

if [ "${1:-}" = "env" ]; then
    echo "export LKRT_STATICLIB=$(pwd)/$LIB"
    echo "export LK_API_STATICLIB=$(pwd)/$API_LIB"
else
    echo "built: $LIB" >&2
    echo "built: $API_LIB" >&2
    echo "export LKRT_STATICLIB=$(pwd)/$LIB LK_API_STATICLIB=$(pwd)/$API_LIB to use them" >&2
fi
