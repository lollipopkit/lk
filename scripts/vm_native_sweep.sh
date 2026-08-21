#!/usr/bin/env bash
# Run every example and bench program under both executors and compare stdout.
#
# What only this catches: a program that compiles *and* answers differently.
# `scripts/aot_coverage.sh` measures whether a program lowers natively and says
# nothing about the answer; the differential test suites compare a pinned corpus
# of small cases. Between them sits "lowers fine, wrong answer, and no case in
# the corpus has that shape" — which is where the `Bytes` global miscompile and
# the typed-list rebuild both lived.
#
# This lived in `/tmp` as a hand-written loop for months, which meant it was
# re-typed from memory after every tmpfs sweep and its expected counts lived in
# a commit message. It is a gate; it belongs in the repo.
#
#   bash scripts/vm_native_sweep.sh              # compare, print a summary
#   SWEEP_REQUIRE="identical=62 diverged=1" …    # fail unless the counts match
#
# Today: identical=62 diverged=1 fallback=0 over 63 programs, ~35s.
#
# One divergence is expected today: `bench/workloads_business_algorithms.lk` is
# nondeterministic (it prints timings), so it differs run to run under either
# executor. `SWEEP_ALLOW_DIVERGED` names the files allowed to differ.
set -uo pipefail

cd "$(dirname "$0")/.."
LK=${LK_BIN:-./target/debug/lk}
ALLOW=${SWEEP_ALLOW_DIVERGED:-bench/workloads_business_algorithms.lk}
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

if [ ! -x "$LK" ]; then
    echo "no $LK — build it with \`cargo build -p lk-cli --features aot\`" >&2
    exit 1
fi

# `--hybrid` sweeps the default configuration instead of the pure-native one.
HYBRID=0
for arg in "$@"; do
    case "$arg" in
    --hybrid) HYBRID=1 ;;
    esac
done

identical=0
diverged=0
fallback=0
diverged_files=""

# `examples/_references` holds *other languages'* sources, so it is not a set of
# LK programs at all. Nothing else is excluded by hand: a program that cannot be
# compiled counts as `fallback`, which is a number worth watching rather than a
# name worth maintaining. The workspace example's app is the current one — its
# package imports do not lower, and it is multi-file so the Tier 0 bundle
# refuses it too.
for src in $(git ls-files 'examples/**/*.lk' 'bench/*.lk' |
    grep -v '^examples/_references/' | sort); do
    vm_out=$("$LK" "$src" 2>&1)
    out_bin="$WORK/$(echo "$src" | tr / _)"
    # `LK_AOT_NO_FALLBACK=1` on the *compile*: without it a program that does
    # not lower falls back to the Tier 0 bundle, which embeds the interpreter —
    # comparing that against the VM compares the VM with itself, and it costs a
    # full Rust link per file to learn nothing.
    if [ "$HYBRID" = 1 ]; then
        # The *shipping* configuration: hybrid on, fallback allowed. Every other
        # gate pins hybrid off, so nothing swept the arrangement a user actually
        # gets — a program whose helper rides the bridge, or that fell back to
        # the Tier 0 bundle, was compared against the VM by nothing at all.
        #
        # A Tier 0 bundle *is* the interpreter, so agreeing proves little there;
        # the value is in the bridged programs, and the two cannot be told apart
        # from outside without reading the build log.
        if ! LK_AOT_HYBRID=1 "$LK" compile "$src" --output "$out_bin" >/dev/null 2>&1; then
            fallback=$((fallback + 1))
            echo "COMPILE-FAILED $src"
            continue
        fi
        native_out=$(cd "$(dirname "$src")" && "$out_bin" 2>&1)
        if [ "$vm_out" = "$native_out" ]; then
            identical=$((identical + 1))
        else
            diverged=$((diverged + 1))
            diverged_files="$diverged_files $src"
            case " $ALLOW " in
            *" $src "*) echo "DIVERGED (allowed) $src" ;;
            *) echo "DIVERGED $src" ;;
            esac
        fi
        continue
    fi
    if ! LK_AOT_NO_FALLBACK=1 "$LK" compile "$src" --output "$out_bin" >/dev/null 2>&1; then
        fallback=$((fallback + 1))
        echo "FALLBACK $src"
        continue
    fi
    # Run from the source's directory: a program that reads a relative path
    # must find the same files either way. `$out_bin` is absolute (mktemp -d),
    # so the `cd` does not reach it.
    native_out=$(cd "$(dirname "$src")" && LK_AOT_NO_FALLBACK=1 "$out_bin" 2>&1)
    if [ "$vm_out" = "$native_out" ]; then
        identical=$((identical + 1))
    else
        diverged=$((diverged + 1))
        diverged_files="$diverged_files $src"
        case " $ALLOW " in
        *" $src "*) echo "DIVERGED (allowed) $src" ;;
        *) echo "DIVERGED $src" ;;
        esac
    fi
done

echo "identical=$identical diverged=$diverged fallback=$fallback"

# Unexpected divergence is a failure even when the totals were not pinned: a
# file that is not on the allow list has no business differing.
status=0
for src in $diverged_files; do
    case " $ALLOW " in
    *" $src "*) ;;
    *)
        echo "::error file=$src::stdout differs between the VM and the native build"
        status=1
        ;;
    esac
done

if [ -n "${SWEEP_REQUIRE:-}" ]; then
    actual="identical=$identical diverged=$diverged"
    if [ "$actual" != "$SWEEP_REQUIRE" ]; then
        echo "::error::expected \"$SWEEP_REQUIRE\", got \"$actual\"" >&2
        status=1
    fi
fi
exit $status
