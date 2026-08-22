#!/usr/bin/env bash
# AOT native-lowering coverage scan (M4.2): tries a native `lk compile` on every
# example and tallies the Unsupported reasons, so "deep coverage" work stays
# data-driven. Usage:
#   bash scripts/aot_coverage.sh          # builds the compiler it scans with
# Output: per-file OK/FAIL lines on stdout, reason ranking on stderr.
#
# Gate mode (used by CI): `AOT_COVERAGE_REQUIRE_FULL=1` makes the script exit
# non-zero unless every example lowers fully native. A program that regresses
# out of native lowering still *runs* correctly — it silently drops to the
# hybrid bridge or the Tier 0 VM bundle — so no differential test can catch
# that; this scan is the only gate. An example that legitimately cannot lower
# must be listed explicitly in `AOT_COVERAGE_ALLOW` (comma-separated paths),
# never dropped silently.
set -u
# The compiler under test is built here rather than assumed. A *missing* binary
# is loud — every compile fails and the count goes to zero — but a *stale* one
# is silent: it reports full coverage for a compiler that never contained the
# change being scanned, which is exactly how a fix once got credit for lowering
# it had not done. `cargo build` is a no-op when nothing changed, so the only
# cost is honesty. An explicit `LK_BIN` is used verbatim: that names a specific
# binary, and whether it matches the tree is the caller's business.
if [ -z "${LK_BIN:-}" ]; then
    cargo build -p lk-cli --features aot || exit 1
fi
LK_BIN="${LK_BIN:-./target/debug/lk}"
REQUIRE_FULL="${AOT_COVERAGE_REQUIRE_FULL:-0}"
ALLOW="${AOT_COVERAGE_ALLOW:-}"
# The metric is *pure native lowering* coverage: with hybrid on (the default),
# a bridged program would count OK and mask a native-coverage regression — pin
# it off for the scan. `LK_AOT_NO_FALLBACK=1` turns a shape the Cranelift
# backend can't lower into a hard error (no Tier 0 VM-bundle fallback), so the
# native compile succeeding == the program lowered fully native.
export LK_AOT_HYBRID=0
export LK_AOT_NO_FALLBACK=1
total=0
ok=0
unexpected=0
reasons_file="$(mktemp)"
tmp_bin="$(mktemp)"
trap 'rm -f "$reasons_file" "$tmp_bin"' EXIT

stale_allow=""
# The bench corpus belongs in the scan for a reason of its own: the bench script
# compiles it with a plain `lk compile`, which happily falls back. A workload
# that stopped lowering would be measured as "AOT" while running the VM bundle —
# the perf numbers would be wrong and nothing would say so.
for f in examples/syntax/*.lk examples/stdlib/*.lk examples/general/*.lk bench/workloads_business_algorithms.lk; do
    total=$((total + 1))
    out=$("$LK_BIN" compile "$f" --output "$tmp_bin" 2>&1)
    if [ $? -eq 0 ]; then
        ok=$((ok + 1))
        echo "OK   $f"
        # An allow-listed file that now compiles is a fixed exemption: report
        # it, or the list quietly keeps waiving coverage it no longer needs.
        case ",$ALLOW," in
            *",$f,"*) stale_allow="$stale_allow $f" ;;
        esac
    else
        # Cranelift/lowering rejects surface as "... (clif: <reason>)" or
        # "... (MIR lowering: <reason>)".
        reason=$(echo "$out" | grep -oE "\((clif: [^)]+|MIR lowering: [^)]+)\)" | head -1)
        case ",$ALLOW," in
            *",$f,"*)
                echo "FAIL $f (allow-listed): ${reason:-unknown}"
                ;;
            *)
                echo "FAIL $f: ${reason:-unknown}"
                unexpected=$((unexpected + 1))
                ;;
        esac
        echo "$reason" | sed 's/(at pc [0-9]*)//; s/at pc [0-9]*/at pc _/' >>"$reasons_file"
    fi
done

echo "----------------------------------------" >&2
echo "coverage: $ok/$total" >&2
echo "blockers by frequency:" >&2
sort "$reasons_file" | uniq -c | sort -rn >&2

if [ -n "$stale_allow" ]; then
    echo "" >&2
    echo "stale AOT_COVERAGE_ALLOW entries (these now lower fully native — drop them):" >&2
    for f in $stale_allow; do
        echo "  $f" >&2
    done
fi

if [ "$REQUIRE_FULL" = "1" ] && [ "$unexpected" -gt 0 ]; then
    echo "" >&2
    echo "FAILED: $unexpected example(s) no longer lower fully native." >&2
    echo "Fix the lowering, or add the file to AOT_COVERAGE_ALLOW with a rationale." >&2
    exit 1
fi
