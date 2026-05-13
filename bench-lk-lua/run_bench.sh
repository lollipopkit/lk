#!/bin/bash
# bench_runner.sh — Run LK vs Lua benchmarks with multiple iterations for stability
set -uo pipefail

BENCH_DIR="$(cd "$(dirname "$0")" && pwd)"
RUNS=7

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
RESET='\033[0m'

LK_BIN="/Users/lk/proj/lk/target/release/lk"
LUA_BIN="lua"

# Store results in temp files
TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR" EXIT

median_of() {
    local file="$1"
    local sorted=$(sort -n "$file" | head -$((RUNS / 2 + 1)) | tail -1)
    echo "$sorted"
}

run_single() {
    local cmd="$1"
    local script="$2"
    local pattern="$3"
    local output=$($cmd "$BENCH_DIR/$script" 2>/dev/null | tail -1)
    echo "$output" | sed -E "s/.*$pattern/\1/" | head -1
}

print_banner() {
    echo ""
    echo -e "${BOLD}${CYAN}╔══════════════════════════════════════════════════════════════╗${RESET}"
    echo -e "${BOLD}${CYAN}║           LK vs Lua — Performance Benchmark Suite           ║${RESET}"
    echo -e "${BOLD}${CYAN}╚══════════════════════════════════════════════════════════════╝${RESET}"
    echo ""
    echo -e "  ${BOLD}LK:${RESET}    target/release/lk (Rust VM, release build)"
    echo -e "  ${BOLD}Lua:${RESET}   $($LUA_BIN -v 2>&1 | head -1)"
    echo -e "  ${BOLD}Runs:${RESET}  ${RUNS} (median reported)"
    echo -e "  ${BOLD}Note:${RESET}  Measuring runtime only (excluding startup overhead)"
    echo ""
}

run_bench() {
    local name="$1"
    local lk_script="$2"
    local lua_script="$3"
    local idx="$4"

    echo -e "${BOLD}${YELLOW}▶ ${name}${RESET}"

    local lk_file="$TMPDIR/lk_${idx}.dat"
    local lua_file="$TMPDIR/lua_${idx}.dat"
    > "$lk_file"
    > "$lua_file"

    for i in $(seq 1 $RUNS); do
        local lk_output=$($LK_BIN "$BENCH_DIR/$lk_script" 2>/dev/null | tail -1)
        local lk_ms=$(echo "$lk_output" | grep -oE 'elapsed=[0-9.]+ms' | grep -oE '[0-9.]+')
        echo "$lk_ms" >> "$lk_file"

        local lua_output=$($LUA_BIN "$BENCH_DIR/$lua_script" | tail -1)
        local lua_ms=$(echo "$lua_output" | grep -oE 'elapsed=[0-9.]+ms' | grep -oE '[0-9.]+' | head -1)
        echo "$lua_ms" >> "$lua_file"
    done

    local lk_median=$(median_of "$lk_file")
    local lua_median=$(median_of "$lua_file")

    echo -e "  ${GREEN}LK:${RESET}  ${lk_median}ms"
    echo -e "  ${RED}Lua:${RESET} ${lua_median}ms"

    # ratio
    local ratio=$(echo "scale=2; $lk_median / $lua_median" | bc 2>/dev/null || echo "?")
    echo -e "  ${BOLD}Ratio (LK/Lua):${RESET} ${ratio}x"
    echo ""
}

print_banner

# Run benchmarks - collect data
idx=0
for bench_spec in \
    "Empty Loop|bench_empty_loop.lk|bench_empty_loop.lua" \
    "Fibonacci Iterative (n=30,50k)|bench_fib.lk|bench_fib.lua" \
    "Fibonacci Recursive (n=15,5k)|bench_fib_recursive.lk|bench_fib_recursive.lua" \
    "Function Call (100k)|bench_func_call.lk|bench_func_call.lua" \
    "List Ops (10k x 100)|bench_list_ops.lk|bench_list_ops.lua" \
    "Map Ops (10k x 50)|bench_map_ops.lk|bench_map_ops.lua" \
    "String Concat (10k x 100)|bench_string_concat.lk|bench_string_concat.lua" \
    "Closure Call (100k)|bench_closure.lk|bench_closure.lua"
do
    name=$(echo "$bench_spec" | cut -d'|' -f1)
    lk_script=$(echo "$bench_spec" | cut -d'|' -f2)
    lua_script=$(echo "$bench_spec" | cut -d'|' -f3)
    run_bench "$name" "$lk_script" "$lua_script" "$idx"

    # Store for summary
    lk_med=$(median_of "$TMPDIR/lk_${idx}.dat")
    lua_med=$(median_of "$TMPDIR/lua_${idx}.dat")
    ratio=$(echo "scale=2; $lk_med / $lua_med" | bc 2>/dev/null || echo "?")
    echo "$name|$lk_med|$lua_med|$ratio" >> "$TMPDIR/summary.dat"

    idx=$((idx + 1))
done

# Summary table
echo -e "${BOLD}${CYAN}═════════════════════════════════════════════════════════════════${RESET}"
echo -e "${BOLD}Summary (median of ${RUNS} runs)${RESET}"
echo -e "${BOLD}${CYAN}═════════════════════════════════════════════════════════════════${RESET}"
printf "${BOLD}%-40s %10s %10s %12s${RESET}\n" "Benchmark" "LK (ms)" "Lua (ms)" "Ratio"
printf "%-40s %10s %10s %12s\n" "───────────────────────────────────────" "──────────" "──────────" "────────────"
while IFS='|' read -r name lk lua ratio; do
    printf "%-40s %10s %10s %12s\n" "$name" "$lk" "$lua" "${ratio}x"
done < "$TMPDIR/summary.dat"
echo -e "${BOLD}${CYAN}═════════════════════════════════════════════════════════════════${RESET}"
echo ""