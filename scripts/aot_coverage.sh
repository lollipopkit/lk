#!/usr/bin/env bash
# AOT native-lowering coverage scan (M4.2): tries `lk compile llvm` on every
# example and tallies the Unsupported reasons, so "deep coverage" work stays
# data-driven. Usage:
#   cargo build -p lk-cli && bash scripts/aot_coverage.sh
# Output: per-file OK/FAIL lines on stdout, reason ranking on stderr.
set -u
LK_BIN="${LK_BIN:-./target/debug/lk}"
total=0
ok=0
reasons_file="$(mktemp)"
trap 'rm -f "$reasons_file"' EXIT

for f in examples/syntax/*.lk examples/stdlib/*.lk examples/general/*.lk; do
    total=$((total + 1))
    out=$("$LK_BIN" compile llvm "$f" 2>&1)
    if [ $? -eq 0 ]; then
        ok=$((ok + 1))
        echo "OK   $f"
    else
        reason=$(echo "$out" | grep -oE "(opcode [A-Za-z0-9]+ \(at pc [0-9]+\)[^\"]*|an operand at pc [0-9]+ has a type outside the natively lowerable subset|MIR lowering: [^\"]+)" | head -1)
        echo "FAIL $f: ${reason:-unknown}"
        echo "$reason" | sed 's/(at pc [0-9]*)//; s/at pc [0-9]*/at pc _/' >>"$reasons_file"
    fi
done

# compile llvm drops .ll files next to the sources — clean them up.
find examples -name '*.ll' -delete 2>/dev/null

echo "----------------------------------------" >&2
echo "coverage: $ok/$total" >&2
echo "blockers by frequency:" >&2
sort "$reasons_file" | uniq -c | sort -rn >&2
