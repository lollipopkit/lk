#!/usr/bin/env bash
#
# Runs the gates and answers with an exit code.
#
# The reason this exists: every gate here already exits non-zero when it fails,
# and every one of them also prints something. Reading the printout is not the
# same as reading the status — a test target that fails to *compile* prints
# `error[E0308]` and no failure keyword at all, so a filter looking for
# "FAILED" or "failures:" sees an empty result and reads as green. That
# happened, for four rounds, and every "all green" reported in them was false.
#
# So: each gate runs, its status is captured, and the summary at the end is the
# statuses. Nothing here parses output to decide anything.
#
# Usage:
#   scripts/verify.sh            # the gates a change has to pass
#   scripts/verify.sh --fast     # skips the slow ones (gc_stress, fuzz, perf, …)
#   scripts/verify.sh --list     # names the gates and exits
#
# Not covered here, because they need a toolchain or emulator this script
# cannot assume: the QEMU bare-metal smokes (thumbv7em, aarch64), the wasm32
# playground build, the Zed extension check, miri, and the ASan/UBSan
# differential runs. `.github/workflows/` is the full set.
#
# The AOT gates need the `aot` feature, which is on by default for lk-cli.

set -u -o pipefail

FAST=0
for arg in "$@"; do
    case "$arg" in
    --fast) FAST=1 ;;
    --list)
        printf '%s\n' fmt lk_fmt artifacts clippy tests coverage sweep no_std gc_stress verify_fuzz sweep_hybrid fuzz perf
        exit 0
        ;;
    *)
        echo "unknown argument: $arg" >&2
        exit 2
        ;;
    esac
done

cd "$(dirname "$0")/.." || exit 2

NAMES=()
STATUSES=()

# Runs one gate, keeping its status. Output goes to the terminal as it happens —
# a gate that hangs should be visible, not buffered.
gate() {
    local name="$1"
    shift
    echo
    echo "=== $name"
    "$@"
    local status=$?
    NAMES+=("$name")
    STATUSES+=("$status")
    if [ "$status" -ne 0 ]; then
        echo "=== $name FAILED (exit $status)"
    fi
    return 0
}

gate fmt cargo fmt --check

# `lk fmt --check` over every `.lk` in the repo. It shipped as a CI feature
# with no workflow running it, and 36 of 97 files were then not in the shape
# the tool produces — including the ones it is demonstrated on. Needs the
# binary, so it builds one first.
lk_fmt_shape() {
    cargo build -p lk-cli || return 1
    ./target/debug/lk fmt --check
}
gate lk_fmt lk_fmt_shape

# `lk compile foo.lk` writes `foo` — extensionless, so no suffix pattern in
# `.gitignore` reaches it and `git add -A` after a compile takes it. Two 20MB
# binaries reached `main` that way. This catches a `git add -f` past the rule.
no_tracked_artifacts() {
    local found
    found=$(git ls-files examples bench | grep -v '\.' || true)
    if [ -n "$found" ]; then
        echo "tracked files with no extension under examples/ or bench/ — build artifacts?" >&2
        echo "$found" >&2
        return 1
    fi
    return 0
}
gate artifacts no_tracked_artifacts

# `--all-targets`, like CI: without it clippy never lints test code, which is
# most of the code added in a normal change.
gate clippy cargo clippy --workspace --all-targets --all-features -- -D warnings
gate tests cargo test --workspace --all-features
gate coverage env AOT_COVERAGE_REQUIRE_FULL=1 bash scripts/aot_coverage.sh
gate sweep bash scripts/vm_native_sweep.sh

no_std_targets() {
    local failed=0
    for crate in lk-core lk-values lkrt; do
        if ! cargo build -p "$crate" --target thumbv7em-none-eabi --no-default-features; then
            echo "no_std build failed: $crate" >&2
            failed=1
        fi
    done
    return "$failed"
}
gate no_std no_std_targets

if [ "$FAST" -eq 0 ]; then
    # Every GC safepoint collects, so a value the host holds without rooting it
    # is freed under the holder. The failure it catches is a wrong answer, not a
    # crash — `json_process.lk` returning the wrong thing is what found it.
    gate gc_stress env LK_GC_STRESS=1 cargo test -p lk-core -p lk-stdlib -p lk-cli
    # The artifact decoder against random bytes: a `.lkm` is an untrusted input
    # to `lk FILE.lkm`, and the verifier is what stands between a corrupt one
    # and the executor.
    gate verify_fuzz env LK_FUZZ_CASES=20000 cargo test -p lk-core verify_fuzz
    # The *shipping* configuration. Every other AOT gate pins `LK_AOT_HYBRID=0`
    # — the pure-native measurement is what they are for — so until this existed
    # nothing swept the arrangement a user gets by default: hybrid on, fallback
    # allowed. Slow for the same reason the pure pass is (a link per program),
    # which is why it sits with the fuzz and the perf run rather than in
    # `--fast`.
    gate sweep_hybrid bash scripts/vm_native_sweep.sh --hybrid
    # The generative differential fuzz is not part of `cargo test --workspace`
    # (its own CI workflow runs it), and it is the only gate that *combines*
    # features. 300 cases is the floor the native-lowering count is stable at.
    gate fuzz env LK_FUZZ_CASES=300 LK_FUZZ_SEED=4242 \
        cargo test -p lk-cli --test aot_fuzz_differential_test
    # Performance is a hard PR gate. This runs it; reading the geometric mean is
    # still the human's job, because "regressed" is a comparison against a base
    # this script does not have.
    gate perf env RUN_AOT=0 RUNS=3 EXTRA_RUNS=5 BENCH_PROGRESS=0 BENCH_TIMEOUT=60 \
        bash bench/run_workload_bench.sh
fi

echo
echo "=== summary"
failed=0
for index in "${!NAMES[@]}"; do
    status="${STATUSES[$index]}"
    if [ "$status" -eq 0 ]; then
        printf '  ok    %s\n' "${NAMES[$index]}"
    else
        printf '  FAIL  %s (exit %s)\n' "${NAMES[$index]}" "$status"
        failed=1
    fi
done
exit "$failed"
