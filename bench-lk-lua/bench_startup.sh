#!/bin/bash
# bench_startup.sh — Measure startup overhead (process time to run a no-op script)
set -uo pipefail

BENCH_DIR="$(cd "$(dirname "$0")" && pwd)"
RUNS=20

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
RESET='\033[0m'

LK_BIN="/Users/lk/proj/lk/target/release/lk"
LUA_BIN="lua"

echo -e "${BOLD}${CYAN}╔══════════════════════════════════════════════════════════════╗${RESET}"
echo -e "${BOLD}${CYAN}║        LK vs Lua — Startup Overhead Benchmark               ║${RESET}"
echo -e "${BOLD}${CYAN}╚══════════════════════════════════════════════════════════════╝${RESET}"
echo ""

# Create minimal scripts
echo 'nil' > "$BENCH_DIR/bench_startup.lk"
echo '' > "$BENCH_DIR/bench_startup.lua"

echo -e "${BOLD}Running ${RUNS} iterations each...${RESET}"
echo ""

# Measure LK startup
lk_times=""
for i in $(seq 1 $RUNS); do
    t=$({ /usr/bin/time -p $LK_BIN "$BENCH_DIR/bench_startup.lk" > /dev/null 2>&1; } 2>&1 | grep real | awk '{print $2}')
    lk_times="$lk_times $t"
done
lk_sorted=$(echo $lk_times | tr ' ' '\n' | sort -n | head -$((RUNS / 2 + 1)) | tail -1)
echo -e "  ${GREEN}LK startup (median wall-clock):${RESET} ${lk_sorted}s"

# Measure Lua startup
lua_times=""
for i in $(seq 1 $RUNS); do
    t=$({ /usr/bin/time -p $LUA_BIN "$BENCH_DIR/bench_startup.lua" > /dev/null 2>&1; } 2>&1 | grep real | awk '{print $2}')
    lua_times="$lua_times $t"
done
lua_sorted=$(echo $lua_times | tr ' ' '\n' | sort -n | head -$((RUNS / 2 + 1)) | tail -1)
echo -e "  ${RED}Lua startup (median wall-clock):${RESET} ${lua_sorted}s"

ratio=$(echo "scale=1; $lk_sorted / $lua_sorted" | bc 2>/dev/null || echo "?")
echo ""
echo -e "  ${BOLD}Ratio (LK/Lua):${RESET} ${ratio}x"
echo ""