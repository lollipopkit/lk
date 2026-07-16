#!/usr/bin/env bash
# AOT native-lowering coverage scan (M4.2): tries a native `lk compile` on every
# example and tallies the Unsupported reasons, so "deep coverage" work stays
# data-driven. Usage:
#   cargo build -p lk-cli --features llvm && bash scripts/aot_coverage.sh
# Output: per-file OK/FAIL lines on stdout, reason ranking on stderr.
set -u
LK_BIN="${LK_BIN:-./target/debug/lk}"
# The metric is *pure native lowering* coverage: with hybrid on (the default),
# a bridged program would count OK and mask a native-coverage regression — pin
# it off for the scan. `LK_AOT_NO_FALLBACK=1` turns a shape the Cranelift
# backend can't lower into a hard error (no Tier 0 VM-bundle fallback), so the
# native compile succeeding == the program lowered fully native.
export LK_AOT_HYBRID=0
export LK_AOT_NO_FALLBACK=1
total=0
ok=0
reasons_file="$(mktemp)"
tmp_bin="$(mktemp)"
trap 'rm -f "$reasons_file" "$tmp_bin"' EXIT

for f in examples/syntax/*.lk examples/stdlib/*.lk examples/general/*.lk; do
    total=$((total + 1))
    out=$("$LK_BIN" compile "$f" --output "$tmp_bin" 2>&1)
    if [ $? -eq 0 ]; then
        ok=$((ok + 1))
        echo "OK   $f"
    else
        # Cranelift/lowering rejects surface as "... (clif: <reason>)" or
        # "... (MIR lowering: <reason>)".
        reason=$(echo "$out" | grep -oE "\((clif: [^)]+|MIR lowering: [^)]+)\)" | head -1)
        echo "FAIL $f: ${reason:-unknown}"
        echo "$reason" | sed 's/(at pc [0-9]*)//; s/at pc [0-9]*/at pc _/' >>"$reasons_file"
    fi
done

echo "----------------------------------------" >&2
echo "coverage: $ok/$total" >&2
echo "blockers by frequency:" >&2
sort "$reasons_file" | uniq -c | sort -rn >&2
