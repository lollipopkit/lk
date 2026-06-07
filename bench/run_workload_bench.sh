#!/bin/bash
# Run LK vs Lua business-style algorithm workloads with adaptive per-workload medians.
set -uo pipefail

BENCH_DIR="$(cd "$(dirname "$0")" && pwd)"
BASE_RUNS="${RUNS:-3}"
EXTRA_RUNS="${EXTRA_RUNS:-3}"
REGRESSION_MARGIN="${REGRESSION_MARGIN:-0.03}"
NOISE_MARGIN="${NOISE_MARGIN:-0.08}"
LK_BIN="/Users/lk/proj/lk/target/release/lk"
LUA_BIN="lua"
RUN_AOT="${RUN_AOT:-1}"
AOT_ENABLED=0
PROFILE_WORKLOADS="${PROFILE_WORKLOADS:-0}"
BENCH_TIMEOUT="${BENCH_TIMEOUT:-30}"
BENCH_PROGRESS="${BENCH_PROGRESS:-1}"

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
  customer_ltv_segments
  event_join_by_id
  config_defaults_merge
  template_render_mix
  state_machine_transitions
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

fmt_ratio_cell() {
  local value="$1"
  if [ "$value" = "?" ]; then
    printf "?"
  else
    printf "%sx" "$value"
  fi
}

profile_value() {
  local file="$1"
  local key="$2"
  sed -n 's/^VM profile: //p' "$file" | tr ' ' '\n' | awk -F= -v key="$key" '$1 == key { print $2; found = 1 } END { if (!found) print "0" }'
}

progress() {
  if [ "$BENCH_PROGRESS" != "0" ]; then
    echo "$@" >&2
  fi
}

run_with_timeout() {
  local out_file="$1"
  local err_file="$2"
  shift 2
  if [ "$BENCH_TIMEOUT" = "0" ]; then
    "$@" >"$out_file" 2>"$err_file"
  else
    perl -e 'my $timeout = shift @ARGV; my $pid = fork(); die "fork failed: $!" unless defined $pid; if ($pid == 0) { exec @ARGV; die "exec failed: $!"; } $SIG{ALRM} = sub { kill "TERM", $pid; sleep 1; kill "KILL", $pid; exit 124; }; alarm $timeout if $timeout > 0; waitpid($pid, 0); my $status = $?; exit($status & 127 ? 128 + ($status & 127) : $status >> 8);' \
      "$BENCH_TIMEOUT" "$@" >"$out_file" 2>"$err_file"
  fi
}

collect_profile_once() {
  local exec_widths=(28 12 10 10 8 10 10 10 10)
  local copy_widths=(28 10 10 10 10 10 10 10 10 10 10)
  local opcode_widths=(28 70)
  local write_source_widths=(28 70)
  local index_key_widths=(28 70)
  local exec_rows=()
  local copy_rows=()
  local opcode_rows=()
  local write_source_rows=()
  local index_key_rows=()
  echo ""
  echo "VM Profile by Workload"

  for name in "${WORKLOADS[@]}"; do
    local err_file opcodes top_opcodes write_sources index_keys calls branches typed containers list_ops map_ops string_ops clones heap_clones copy_heap reg_heap local_heap load_heap store_heap const_heap arg_heap cont_heap
    err_file="$TMPDIR/profile_${name}.err"
    local out_file
    out_file="$TMPDIR/profile_${name}.out"
    if ! LK_VM_PROFILE=1 LK_WORKLOAD_FILTER="$name" run_with_timeout "$out_file" "$err_file" "$LK_BIN" "$BENCH_DIR/workloads_business_algorithms.lk"; then
      echo "VM profile run failed for workload '$name'" >&2
      sed 's/^/  /' "$err_file" >&2
      return 1
    fi
    opcodes=$(profile_value "$err_file" opcode_steps)
    top_opcodes=$(profile_value "$err_file" top_opcodes)
    write_sources=$(profile_value "$err_file" write_sources)
    index_keys=$(profile_value "$err_file" index_keys)
    calls=$(profile_value "$err_file" calls)
    branches=$(profile_value "$err_file" branches)
    typed=$(profile_value "$err_file" typed_branches)
    containers=$(profile_value "$err_file" containers)
    list_ops=$(profile_value "$err_file" list_ops)
    map_ops=$(profile_value "$err_file" map_ops)
    string_ops=$(profile_value "$err_file" string_ops)
    clones=$(profile_value "$err_file" val_clones)
    heap_clones=$(profile_value "$err_file" heap_clones)
    copy_heap=$(profile_value "$err_file" copy_policy_heap_clones)
    reg_heap=$(profile_value "$err_file" register_copy_heap_clones)
    local_heap=$(profile_value "$err_file" local_copy_heap_clones)
    load_heap=$(profile_value "$err_file" local_load_heap_clones)
    store_heap=$(profile_value "$err_file" local_store_heap_clones)
    const_heap=$(profile_value "$err_file" const_load_heap_clones)
    arg_heap=$(profile_value "$err_file" call_arg_heap_clones)
    cont_heap=$(profile_value "$err_file" container_copy_heap_clones)
    exec_rows+=("$name|$opcodes|$calls|$branches|$typed|$containers|$list_ops|$map_ops|$string_ops")
    copy_rows+=("$name|$clones|$heap_clones|$copy_heap|$reg_heap|$local_heap|$load_heap|$store_heap|$const_heap|$arg_heap|$cont_heap")
    opcode_rows+=("$name|$top_opcodes")
    write_source_rows+=("$name|$write_sources")
    index_key_rows+=("$name|$index_keys")
  done

  printf "%-28s %12s %10s %10s %8s %10s %10s %10s %10s\n" \
    "Workload" "Opcodes" "Calls" "Branches" "Typed" "Containers" "List" "Map" "String"
  print_separator "${exec_widths[@]}"
  for row in "${exec_rows[@]}"; do
    IFS='|' read -r name opcodes calls branches typed containers list_ops map_ops string_ops <<< "$row"
    printf "%-28s %12s %10s %10s %8s %10s %10s %10s %10s\n" \
      "$name" "$opcodes" "$calls" "$branches" "$typed" "$containers" "$list_ops" "$map_ops" "$string_ops"
  done

  echo ""
  echo "VM Copy Profile by Workload"
  printf "%-28s %10s %10s %10s %10s %10s %10s %10s %10s %10s %10s\n" \
    "Workload" "Clones" "HeapClone" "CopyHeap" "RegHeap" "LocalHeap" "LoadHeap" "StoreHeap" "ConstHeap" "ArgHeap" "ContHeap"
  print_separator "${copy_widths[@]}"
  for row in "${copy_rows[@]}"; do
    IFS='|' read -r name clones heap_clones copy_heap reg_heap local_heap load_heap store_heap const_heap arg_heap cont_heap <<< "$row"
    printf "%-28s %10s %10s %10s %10s %10s %10s %10s %10s %10s %10s\n" \
      "$name" "$clones" "$heap_clones" "$copy_heap" "$reg_heap" "$local_heap" "$load_heap" "$store_heap" "$const_heap" "$arg_heap" "$cont_heap"
  done

  echo ""
  echo "VM Dynamic Opcode Top-6 by Workload"
  printf "%-28s %-70s\n" "Workload" "Top opcodes"
  print_separator "${opcode_widths[@]}"
  for row in "${opcode_rows[@]}"; do
    IFS='|' read -r name top_opcodes <<< "$row"
    printf "%-28s %-70s\n" "$name" "$top_opcodes"
  done

  echo ""
  echo "VM Register Write Source Top-6 by Workload"
  printf "%-28s %-70s\n" "Workload" "Top write sources"
  print_separator "${write_source_widths[@]}"
  for row in "${write_source_rows[@]}"; do
    IFS='|' read -r name write_sources <<< "$row"
    printf "%-28s %-70s\n" "$name" "$write_sources"
  done

  echo ""
  echo "VM Index Key Top-6 by Workload"
  printf "%-28s %-70s\n" "Workload" "Top index key metrics"
  print_separator "${index_key_widths[@]}"
  for row in "${index_key_rows[@]}"; do
    IFS='|' read -r name index_keys <<< "$row"
    printf "%-28s %-70s\n" "$name" "$index_keys"
  done
}

repeat_char() {
  local char="$1"
  local count="$2"
  awk -v char="$char" -v count="$count" 'BEGIN {
    for (i = 0; i < count; i++) {
      printf "%s", char
    }
  }'
}

print_separator() {
  local widths=("$@")
  local first=1
  local width
  for width in "${widths[@]}"; do
    if [ "$first" -eq 0 ]; then
      printf " "
    fi
    repeat_char "-" "$width"
    first=0
  done
  printf "\n"
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
    AOT_ENABLED=1
    AOT_BACKEND=$(sed -nE 's/.*backend ([^,]+),.*/\1/p' "$AOT_COMPILE_LOG" | tail -1)
    if [ -z "$AOT_BACKEND" ]; then
      AOT_BACKEND="unknown"
    fi
    echo "AOT: $AOT_BIN ($AOT_BACKEND)"
  else
    AOT_BACKEND="skipped"
    echo "AOT: skipped (compile failed)"
    echo "AOT compile failed; continuing with LK VM and Lua only:" >&2
    sed 's/^/  /' "$AOT_COMPILE_LOG" >&2
  fi
else
  AOT_BACKEND="disabled"
  echo "AOT: disabled"
fi
echo "Runs: $BASE_RUNS base + $EXTRA_RUNS adaptive extra when regression/noise is suspected"
echo "Per-workload timeout: ${BENCH_TIMEOUT}s (set BENCH_TIMEOUT=0 to disable)"
echo "Regression margin: $(awk -v x="$REGRESSION_MARGIN" 'BEGIN { printf "%.1f%%", x * 100 }') vs documented baseline"
echo "Noise margin: $(awk -v x="$NOISE_MARGIN" 'BEGIN { printf "%.1f%%", x * 100 }') max spread"
if [ "$PROFILE_WORKLOADS" != "0" ]; then
  echo "VM profile: enabled, one extra filtered LK run per workload"
fi
echo ""

for name in "${WORKLOADS[@]}"; do
  > "$TMPDIR/lk_${name}.dat"
  > "$TMPDIR/lua_${name}.dat"
  > "$TMPDIR/aot_${name}.dat"
done

run_engine_workload() {
  local engine="$1"
  local name="$2"
  local sample="$3"
  local out_file err_file command_label
  out_file="$TMPDIR/${engine}_${name}_${sample}.out"
  err_file="$TMPDIR/${engine}_${name}_${sample}.err"

  case "$engine" in
    lk)
      command_label="LK"
      progress "[$sample] $command_label $name"
      if ! LK_WORKLOAD_FILTER="$name" run_with_timeout "$out_file" "$err_file" "$LK_BIN" "$BENCH_DIR/workloads_business_algorithms.lk"; then
        echo "$command_label workload '$name' failed or timed out after ${BENCH_TIMEOUT}s" >&2
        sed 's/^/  /' "$err_file" >&2
        return 1
      fi
      ;;
    aot)
      command_label="AOT"
      progress "[$sample] $command_label $name"
      if ! LK_WORKLOAD_FILTER="$name" run_with_timeout "$out_file" "$err_file" "$AOT_BIN"; then
        echo "$command_label workload '$name' failed or timed out after ${BENCH_TIMEOUT}s" >&2
        sed 's/^/  /' "$err_file" >&2
        return 1
      fi
      ;;
    lua)
      command_label="Lua"
      progress "[$sample] $command_label $name"
      if ! LK_WORKLOAD_FILTER="$name" run_with_timeout "$out_file" "$err_file" "$LUA_BIN" "$BENCH_DIR/workloads_business_algorithms.lua"; then
        echo "$command_label workload '$name' failed or timed out after ${BENCH_TIMEOUT}s" >&2
        sed 's/^/  /' "$err_file" >&2
        return 1
      fi
      ;;
    *)
      echo "Unknown benchmark engine '$engine'" >&2
      return 1
      ;;
  esac

  if ! grep -q "^workload|${name}|" "$out_file"; then
    echo "$command_label workload '$name' produced no matching workload output" >&2
    sed 's/^/  /' "$out_file" >&2
    sed 's/^/  /' "$err_file" >&2
    return 1
  fi

  record_output "$engine" < "$out_file"
}

run_once() {
  local sample="$1"
  local name
  for name in "${WORKLOADS[@]}"; do
    run_engine_workload lk "$name" "$sample" || return 1
    if [ "$AOT_ENABLED" = "1" ]; then
      run_engine_workload aot "$name" "$sample" || return 1
    fi
    run_engine_workload lua "$name" "$sample" || return 1
  done
}

for run_index in $(seq 1 "$BASE_RUNS"); do
  run_once "base-$run_index" || exit 1
done

REASONS="$TMPDIR/adaptive_reasons.txt"
> "$REASONS"
if should_extend_runs "$REASONS"; then
  echo "Adaptive rerun triggered:"
  sed 's/^/  - /' "$REASONS"
  echo "Running $EXTRA_RUNS additional samples for the full workload suite..."
  if [ "$EXTRA_RUNS" -gt 0 ]; then
    for _ in $(seq 1 "$EXTRA_RUNS"); do
      run_once "extra-$_" || exit 1
    done
  fi
  echo ""
fi

TOTAL_RUNS=$(awk 'END { print NR }' "$TMPDIR/lk_${WORKLOADS[0]}.dat")

if [ "$AOT_ENABLED" = "1" ]; then
  table_widths=(28 10 10 10 10 10 10 8 10 11 8)
  printf "%-28s %10s %10s %10s %10s %10s %10s %8s %10s %11s %s\n" "Workload" "LK VM" "LK AOT" "Lua" "VM/Lua" "AOT/Lua" "AOT/VM" "Noise" "Conf." "Status" "Checksum"
  print_separator "${table_widths[@]}"
else
  table_widths=(28 10 10 10 8 10 11 8)
  printf "%-28s %10s %10s %10s %8s %10s %11s %s\n" "Workload" "LK (ms)" "Lua (ms)" "Ratio" "Noise" "Conf." "Status" "Checksum"
  print_separator "${table_widths[@]}"
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
  if [ "$AOT_ENABLED" = "1" ]; then
    aot_ms=$(median_of "$TMPDIR/aot_${name}.dat")
    aot_lua_ratio=$(ratio_of "$aot_ms" "$lua_ms")
    aot_vm_ratio=$(ratio_of "$aot_ms" "$lk_ms")
    aot_fmt=$(fmt_ms "$aot_ms")
    aot_lua_cell=$(fmt_ratio_cell "$aot_lua_ratio")
    aot_vm_cell=$(fmt_ratio_cell "$aot_vm_ratio")
  fi
  lk_fmt=$(fmt_ms "$lk_ms")
  lua_fmt=$(fmt_ms "$lua_ms")
  ratio_cell=$(fmt_ratio_cell "$ratio")
  lk_spread=$(spread_of "$TMPDIR/lk_${name}.dat" "$lk_ms")
  lua_spread=$(spread_of "$TMPDIR/lua_${name}.dat" "$lua_ms")
  noise=$(max_of_two "$lk_spread" "$lua_spread")
  confidence=$(classify_confidence "$noise")
  status=$(classify_ratio "$ratio")
  if [ "$ratio" != "?" ]; then
    echo "$ratio" >> "$ratio_file"
  fi
  if [ "$AOT_ENABLED" = "1" ] && [ "$aot_lua_ratio" != "?" ]; then
    echo "$aot_lua_ratio" >> "$aot_ratio_file"
  fi
  if [ "$AOT_ENABLED" = "1" ] && [ "$aot_vm_ratio" != "?" ]; then
    echo "$aot_vm_ratio" >> "$speedup_file"
  fi
  lk_sum=$(cat "$TMPDIR/lk_${name}.checksum")
  lua_sum=$(cat "$TMPDIR/lua_${name}.checksum")
  checksum="$lk_sum"
  if [ "$lk_sum" != "$lua_sum" ]; then
    checksum="MISMATCH lk=$lk_sum lua=$lua_sum"
    mismatch_count=$((mismatch_count + 1))
  fi
  if [ "$AOT_ENABLED" = "1" ]; then
    aot_sum=$(cat "$TMPDIR/aot_${name}.checksum")
    if [ "$aot_sum" != "$lua_sum" ] || [ "$aot_sum" != "$lk_sum" ]; then
      checksum="MISMATCH lk=$lk_sum aot=$aot_sum lua=$lua_sum"
      mismatch_count=$((mismatch_count + 1))
    fi
    printf "%-28s %10s %10s %10s %10s %10s %10s %8s %10s %11s %s\n" "$name" "$lk_fmt" "$aot_fmt" "$lua_fmt" "$ratio_cell" "$aot_lua_cell" "$aot_vm_cell" "$noise" "$confidence" "$status" "$checksum"
  else
    printf "%-28s %10s %10s %10s %8s %10s %11s %s\n" "$name" "$lk_fmt" "$lua_fmt" "$ratio_cell" "$noise" "$confidence" "$status" "$checksum"
  fi
done
geo_ratio=$(awk '{ sum += log($1); n++ } END { if (n > 0) { printf "%.3f", exp(sum / n) } else { print "?" } }' "$ratio_file")
aot_geo_ratio=$(awk '{ sum += log($1); n++ } END { if (n > 0) { printf "%.3f", exp(sum / n) } else { print "?" } }' "$aot_ratio_file")
aot_vm_geo_ratio=$(awk '{ sum += log($1); n++ } END { if (n > 0) { printf "%.3f", exp(sum / n) } else { print "?" } }' "$speedup_file")
echo ""
echo "Samples reported: $TOTAL_RUNS per engine"
echo "Geometric mean ratio: ${geo_ratio}x"
if [ "$AOT_ENABLED" = "1" ]; then
  echo "AOT backend: $AOT_BACKEND"
  echo "AOT geometric mean ratio: ${aot_geo_ratio}x vs Lua"
  echo "AOT/VM geometric mean ratio: ${aot_vm_geo_ratio}x"
elif [ "$RUN_AOT" != "0" ]; then
  echo "AOT backend: skipped"
fi
echo "Status thresholds: ahead <=0.95x, close <=1.10x, behind >1.10x"
echo "Confidence: high <=3% noise, medium <=${NOISE_MARGIN} noise, low above that."
echo "Noise is max((p80-p20)/median) across LK and Lua samples for that workload."

if [ "$mismatch_count" -gt 0 ]; then
  echo "Checksum mismatches: $mismatch_count" >&2
  exit 1
fi

if [ "$PROFILE_WORKLOADS" != "0" ]; then
  collect_profile_once
fi
