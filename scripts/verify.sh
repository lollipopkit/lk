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
#   scripts/verify.sh --fast     # skips the two slow ones (fuzz, perf)
#   scripts/verify.sh --list     # names the gates and exits
#
# The AOT gates need the `aot` feature, which is on by default for lk-cli.

set -u -o pipefail

FAST=0
for arg in "$@"; do
    case "$arg" in
    --fast) FAST=1 ;;
    --list)
        printf '%s\n' fmt clippy tests coverage sweep no_std fuzz perf
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
gate clippy cargo clippy --workspace --all-features -- -D warnings
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
