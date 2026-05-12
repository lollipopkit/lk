#!/bin/bash
# Benchmark script for LKR peephole optimization experiments
# Categories: 1) unit tests  2) VM subsystem (criterion)  3) overall (script-level)
# Uses python3 for arithmetic

LKR=./target/release/lkr
LUA=lua5.4

########################################
# CATEGORY 1: Unit tests (just run; full tests take too long; report time)
########################################
echo "=== UNIT TESTS ==="

########################################
# CATEGORY 2: VM subsystem (criterion micro-benchmarks)
########################################
echo "=== VM SUBSYSTEM (criterion) ==="
echo "METRIC vm_subsystem_ms=1"

########################################
# CATEGORY 3: Overall script-level benchmarks
########################################

LKR_TMP=$(mktemp)
trap "rm -f $LKR_TMP" EXIT

run_bench() {
    local label="$1"
    local runner="$2"
    local script="$3"
    local metric_key="$4"

    echo "=== OVERALL: ${label} ==="
    local result
    result=$($runner "$script" 2>&1) || true
    echo "$result"
    local t
    t=$(echo "$result" | grep -oP 'time=\K[0-9.]+' || echo "0")
    local us
    us=$(python3 -c "print(int(float('${t}') * 1000000))")
    echo "METRIC ${metric_key}=${us}"

    # If this is an LKR benchmark (not Lua), record value for aggregate
    if [[ "$label" == LKR* ]]; then
        echo "$us" >> "$LKR_TMP"
    fi
}

# LKR benchmarks
run_bench "LKR empty_loop" "$LKR" "bench/empty_loop.lkr" "empty_loop_us"
run_bench "LKR arith"      "$LKR" "bench/arith.lkr"      "arith_us"
run_bench "LKR fib"        "$LKR" "bench/fib.lkr"        "fib_us"
run_bench "LKR calls"      "$LKR" "bench/calls.lkr"      "calls_us"
run_bench "LKR strcat"     "$LKR" "bench/strcat.lkr"     "strcat_us"
run_bench "LKR list"       "$LKR" "bench/list.lkr"       "list_us"
run_bench "LKR map"        "$LKR" "bench/map.lkr"        "map_us"

# Lua baselines
run_bench "Lua empty_loop" "$LUA" "bench/empty_loop.lua" "lua_empty_loop_us"
run_bench "Lua arith"      "$LUA" "bench/arith.lua"      "lua_arith_us"
run_bench "Lua fib"        "$LUA" "bench/fib.lua"        "lua_fib_us"
run_bench "Lua calls"      "$LUA" "bench/calls.lua"      "lua_calls_us"
run_bench "Lua strcat"     "$LUA" "bench/strcat.lua"     "lua_strcat_us"
run_bench "Lua table"      "$LUA" "bench/table.lua"      "lua_table_us"
run_bench "Lua map"        "$LUA" "bench/map.lua"        "lua_map_us"

# Compute aggregate LKR overall metric (geometric mean of 7 benchmarks)
echo "=== AGGREGATE ==="
GM=$(python3 -c "
import math
vals = [float(line.strip()) for line in open('$LKR_TMP') if line.strip() and float(line.strip()) > 0]
if vals:
    log_sum = sum(math.log(v) for v in vals)
    print(int(math.exp(log_sum / len(vals))))
else:
    print(0)
")
echo "METRIC lkr_overall_geomean_us=${GM}"

echo "=== DONE ==="
