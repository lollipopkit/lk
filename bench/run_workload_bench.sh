#!/bin/bash
# Run LK vs Lua business-style algorithm workloads with adaptive per-workload medians.
set -uo pipefail

BENCH_DIR="$(cd "$(dirname "$0")" && pwd)"
BASE_RUNS="${RUNS:-5}"
EXTRA_RUNS="${EXTRA_RUNS:-10}"
REGRESSION_MARGIN="${REGRESSION_MARGIN:-0.03}"
NOISE_MARGIN="${NOISE_MARGIN:-0.08}"
LK_BIN="/Users/lk/proj/lk/target/release/lk"
LUA_BIN="lua"
RUN_AOT="${RUN_AOT:-1}"

TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR" EXIT
AOT_BIN="${AOT_BIN:-$TMPDIR/lk-workloads-aot}"

WORKLOADS=(
  gcd_batch
  prime_trial_division
  binary_search
  two_sum_map
  sliding_window_sum
  matrix_3x3_multiply
  stock_max_profit
  histogram_group_count
  string_key_hash
  order_score_pipeline
  log_parse_filter
  cart_pricing_rules
  route_permission_check
  inventory_reorder
  fraud_rule_scoring
)

median_of() {
  local file="$1"
  local count
  count=$(wc -l < "$file")
  sort -n "$file" | head -$((count / 2 + 1)) | tail -1
}

ratio_of() {
  awk -v lk="$1" -v lua="$2" 'BEGIN {
    if (lua == 0) {
      print "?"
    } else {
      printf "%.3f", lk / lua
    }
  }'
}

fmt_ms() {
  awk -v value="$1" 'BEGIN { printf "%.3f", value }'
}

spread_of() {
  local file="$1"
  local med="$2"
  sort -n "$file" | awk -v med="$med" '
    { values[NR] = $1 }
    END {
      if (NR == 0 || med == 0) {
        print "0.000"
        exit
      }
      lo = int(NR * 0.20)
      hi = int(NR * 0.80)
      if (lo < 1) {
        lo = 1
      }
      if (hi < lo) {
        hi = lo
      }
      if (hi > NR) {
        hi = NR
      }
      printf "%.3f", (values[hi] - values[lo]) / med
    }
  '
}

max_of_two() {
  awk -v a="$1" -v b="$2" 'BEGIN {
    if (a > b) {
      printf "%.3f", a
    } else {
      printf "%.3f", b
    }
  }'
}

baseline_ratio() {
  case "$1" in
    gcd_batch) echo "7.310" ;;
    prime_trial_division) echo "5.523" ;;
    binary_search) echo "3.365" ;;
    two_sum_map) echo "1.245" ;;
    sliding_window_sum) echo "4.248" ;;
    matrix_3x3_multiply) echo "6.494" ;;
    stock_max_profit) echo "4.989" ;;
    histogram_group_count) echo "2.096" ;;
    string_key_hash) echo "2.298" ;;
    order_score_pipeline) echo "4.610" ;;
    *) echo "" ;;
  esac
}

classify_ratio() {
  awk -v ratio="$1" 'BEGIN {
    if (ratio == "?") {
      print "unknown"
    } else if (ratio <= 0.95) {
      print "ahead"
    } else if (ratio <= 1.10) {
      print "close"
    } else {
      print "behind"
    }
  }'
}

classify_confidence() {
  awk -v noise="$1" -v limit="$NOISE_MARGIN" 'BEGIN {
    if (noise <= 0.03) {
      print "high"
    } else if (noise <= limit) {
      print "medium"
    } else {
      print "low"
    }
  }'
}

should_extend_runs() {
  local reason_file="$1"
  local should_run=1

  for name in "${WORKLOADS[@]}"; do
    local lk_ms lua_ms ratio baseline lk_spread lua_spread noise
    lk_ms=$(median_of "$TMPDIR/lk_${name}.dat")
    lua_ms=$(median_of "$TMPDIR/lua_${name}.dat")
    ratio=$(ratio_of "$lk_ms" "$lua_ms")
    baseline=$(baseline_ratio "$name")
    lk_spread=$(spread_of "$TMPDIR/lk_${name}.dat" "$lk_ms")
    lua_spread=$(spread_of "$TMPDIR/lua_${name}.dat" "$lua_ms")
    noise=$(max_of_two "$lk_spread" "$lua_spread")

    if awk -v noise="$noise" -v limit="$NOISE_MARGIN" 'BEGIN { exit !(noise > limit) }'; then
      echo "$name: high noise ${noise} > ${NOISE_MARGIN}" >> "$reason_file"
      should_run=0
    fi

    if [ -n "$baseline" ] && awk -v ratio="$ratio" -v baseline="$baseline" -v margin="$REGRESSION_MARGIN" 'BEGIN { exit !(ratio > baseline * (1 + margin)) }'; then
      echo "$name: possible regression ${ratio}x > baseline ${baseline}x by more than ${REGRESSION_MARGIN}" >> "$reason_file"
      should_run=0
    fi
  done

  return "$should_run"
}

record_output() {
  local engine="$1"
  while IFS='|' read -r marker name checksum elapsed; do
    if [ "$marker" != "workload" ]; then
      continue
    fi
    local ms
    ms=$(echo "$elapsed" | sed -E 's/elapsed=([0-9.]+)ms/\1/')
    echo "$ms" >> "$TMPDIR/${engine}_${name}.dat"
    echo "$checksum" | sed -E 's/checksum=//' > "$TMPDIR/${engine}_${name}.checksum"
  done
}

echo ""
echo "LK vs Lua — Business Algorithm Workloads"
echo "LK:  target/release/lk"
echo "Lua: $($LUA_BIN -v 2>&1 | head -1)"
if [ "$RUN_AOT" != "0" ]; then
  AOT_COMPILE_LOG="$TMPDIR/aot_compile.log"
  if "$LK_BIN" compile exe "$BENCH_DIR/workloads_business_algorithms.lk" --output "$AOT_BIN" > "$AOT_COMPILE_LOG" 2>&1; then
    AOT_BACKEND=$(sed -nE 's/.*backend ([^,]+),.*/\1/p' "$AOT_COMPILE_LOG" | tail -1)
    if [ -z "$AOT_BACKEND" ]; then
      AOT_BACKEND="unknown"
    fi
    echo "AOT: $AOT_BIN ($AOT_BACKEND)"
  else
    echo "AOT compile failed:" >&2
    sed 's/^/  /' "$AOT_COMPILE_LOG" >&2
    exit 1
  fi
else
  AOT_BACKEND="disabled"
  echo "AOT: disabled"
fi
echo "Runs: $BASE_RUNS base + $EXTRA_RUNS adaptive extra when regression/noise is suspected"
echo "Regression margin: $(awk -v x="$REGRESSION_MARGIN" 'BEGIN { printf "%.1f%%", x * 100 }') vs documented baseline"
echo "Noise margin: $(awk -v x="$NOISE_MARGIN" 'BEGIN { printf "%.1f%%", x * 100 }') max spread"
echo ""

for name in "${WORKLOADS[@]}"; do
  > "$TMPDIR/lk_${name}.dat"
  > "$TMPDIR/lua_${name}.dat"
  > "$TMPDIR/aot_${name}.dat"
done

run_once() {
  "$LK_BIN" "$BENCH_DIR/workloads_business_algorithms.lk" 2>/dev/null | record_output lk
  if [ "$RUN_AOT" != "0" ]; then
    "$AOT_BIN" | record_output aot
  fi
  "$LUA_BIN" "$BENCH_DIR/workloads_business_algorithms.lua" | record_output lua
}

for _ in $(seq 1 "$BASE_RUNS"); do
  run_once
done

REASONS="$TMPDIR/adaptive_reasons.txt"
> "$REASONS"
if should_extend_runs "$REASONS"; then
  echo "Adaptive rerun triggered:"
  sed 's/^/  - /' "$REASONS"
  echo "Running $EXTRA_RUNS additional samples for the full workload suite..."
  for _ in $(seq 1 "$EXTRA_RUNS"); do
    run_once
  done
  echo ""
fi

TOTAL_RUNS=$(awk 'END { print NR }' "$TMPDIR/lk_${WORKLOADS[0]}.dat")

if [ "$RUN_AOT" != "0" ]; then
  printf "%-28s %10s %10s %10s %10s %10s %10s %8s %10s %11s %s\n" "Workload" "LK VM" "LK AOT" "Lua" "VM/Lua" "AOT/Lua" "AOT/VM" "Noise" "Conf." "Status" "Checksum"
  printf "%-28s %10s %10s %10s %10s %10s %10s %8s %10s %11s %s\n" "────────────────────────────" "──────────" "──────────" "──────────" "──────────" "──────────" "──────────" "────────" "──────────" "───────────" "────────"
else
  printf "%-28s %10s %10s %10s %8s %10s %11s %s\n" "Workload" "LK (ms)" "Lua (ms)" "Ratio" "Noise" "Conf." "Status" "Checksum"
  printf "%-28s %10s %10s %10s %8s %10s %11s %s\n" "────────────────────────────" "──────────" "──────────" "──────────" "────────" "──────────" "───────────" "────────"
fi
mismatch_count=0
ratio_file="$TMPDIR/ratios.dat"
aot_ratio_file="$TMPDIR/aot_ratios.dat"
speedup_file="$TMPDIR/aot_vm_ratios.dat"
> "$ratio_file"
> "$aot_ratio_file"
> "$speedup_file"
for name in "${WORKLOADS[@]}"; do
  lk_ms=$(median_of "$TMPDIR/lk_${name}.dat")
  lua_ms=$(median_of "$TMPDIR/lua_${name}.dat")
  ratio=$(ratio_of "$lk_ms" "$lua_ms")
  if [ "$RUN_AOT" != "0" ]; then
    aot_ms=$(median_of "$TMPDIR/aot_${name}.dat")
    aot_lua_ratio=$(ratio_of "$aot_ms" "$lua_ms")
    aot_vm_ratio=$(ratio_of "$aot_ms" "$lk_ms")
    aot_fmt=$(fmt_ms "$aot_ms")
  fi
  lk_fmt=$(fmt_ms "$lk_ms")
  lua_fmt=$(fmt_ms "$lua_ms")
  lk_spread=$(spread_of "$TMPDIR/lk_${name}.dat" "$lk_ms")
  lua_spread=$(spread_of "$TMPDIR/lua_${name}.dat" "$lua_ms")
  noise=$(max_of_two "$lk_spread" "$lua_spread")
  confidence=$(classify_confidence "$noise")
  status=$(classify_ratio "$ratio")
  if [ "$ratio" != "?" ]; then
    echo "$ratio" >> "$ratio_file"
  fi
  if [ "$RUN_AOT" != "0" ] && [ "$aot_lua_ratio" != "?" ]; then
    echo "$aot_lua_ratio" >> "$aot_ratio_file"
  fi
  if [ "$RUN_AOT" != "0" ] && [ "$aot_vm_ratio" != "?" ]; then
    echo "$aot_vm_ratio" >> "$speedup_file"
  fi
  lk_sum=$(cat "$TMPDIR/lk_${name}.checksum")
  lua_sum=$(cat "$TMPDIR/lua_${name}.checksum")
  checksum="$lk_sum"
  if [ "$lk_sum" != "$lua_sum" ]; then
    checksum="MISMATCH lk=$lk_sum lua=$lua_sum"
    mismatch_count=$((mismatch_count + 1))
  fi
  if [ "$RUN_AOT" != "0" ]; then
    aot_sum=$(cat "$TMPDIR/aot_${name}.checksum")
    if [ "$aot_sum" != "$lua_sum" ] || [ "$aot_sum" != "$lk_sum" ]; then
      checksum="MISMATCH lk=$lk_sum aot=$aot_sum lua=$lua_sum"
      mismatch_count=$((mismatch_count + 1))
    fi
    printf "%-28s %10s %10s %10s %10sx %10sx %10sx %8s %10s %11s %s\n" "$name" "$lk_fmt" "$aot_fmt" "$lua_fmt" "$ratio" "$aot_lua_ratio" "$aot_vm_ratio" "$noise" "$confidence" "$status" "$checksum"
  else
    printf "%-28s %10s %10s %10sx %8s %10s %11s %s\n" "$name" "$lk_fmt" "$lua_fmt" "$ratio" "$noise" "$confidence" "$status" "$checksum"
  fi
done
geo_ratio=$(awk '{ sum += log($1); n++ } END { if (n > 0) { printf "%.3f", exp(sum / n) } else { print "?" } }' "$ratio_file")
aot_geo_ratio=$(awk '{ sum += log($1); n++ } END { if (n > 0) { printf "%.3f", exp(sum / n) } else { print "?" } }' "$aot_ratio_file")
aot_vm_geo_ratio=$(awk '{ sum += log($1); n++ } END { if (n > 0) { printf "%.3f", exp(sum / n) } else { print "?" } }' "$speedup_file")
echo ""
echo "Samples reported: $TOTAL_RUNS per engine"
echo "Geometric mean ratio: ${geo_ratio}x"
if [ "$RUN_AOT" != "0" ]; then
  echo "AOT backend: $AOT_BACKEND"
  echo "AOT geometric mean ratio: ${aot_geo_ratio}x vs Lua"
  echo "AOT/VM geometric mean ratio: ${aot_vm_geo_ratio}x"
fi
echo "Status thresholds: ahead <=0.95x, close <=1.10x, behind >1.10x"
echo "Confidence: high <=3% noise, medium <=${NOISE_MARGIN} noise, low above that."
echo "Noise is max((p80-p20)/median) across LK and Lua samples for that workload."

if [ "$mismatch_count" -gt 0 ]; then
  echo "Checksum mismatches: $mismatch_count" >&2
  exit 1
fi
