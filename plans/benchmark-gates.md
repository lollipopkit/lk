# Benchmark Gates for Runtime Work

## Canonical Gate

Use the real workload suite. Microbenchmarks are not enough to claim broad LK VM
improvements.

```bash
cargo build --release -p lk-cli
RUNS=10 EXTRA_RUNS=20 bench/run_workload_bench.sh
```

Checksum parity is mandatory. A faster result with a checksum mismatch is a bug.

## Required Correctness Commands

```bash
cargo test -p lk-core
cargo test --workspace
cargo test --all-features --all-targets
```

For docs-only work, these commands are optional. For runtime changes, they are
part of acceptance unless a real environment blocker is recorded.

## Workload Mapping

| Area | Workloads |
| --- | --- |
| Numeric op specialization | `gcd_batch`, `prime_trial_division`, `matrix_3x3_multiply`, `stock_max_profit` |
| Function call protocol | `gcd_batch`, `binary_search`, `order_score_pipeline`, `route_permission_check` |
| List fast paths | `sliding_window_sum`, `inventory_reorder` |
| Map/string key fast paths | `two_sum_map`, `histogram_group_count`, `log_parse_filter`, `fraud_rule_scoring` |
| Template/string building | `string_key_hash`, `log_parse_filter` |
| Mixed business pipeline | `cart_pricing_rules`, `inventory_reorder`, `fraud_rule_scoring` |

## Measurement Rules

- Rebuild release binaries before measuring.
- Compare against the current branch baseline, not stale README numbers.
- Prefer median and geometric mean from the runner.
- Treat low-confidence rows as inconclusive until rerun on a quieter machine.
- If an optimization only improves one synthetic-looking pattern but worsens
  mixed workloads, do not claim broad success.

## Reporting Format

Every runtime performance PR should include:

- command used
- sample count
- geometric mean before/after
- target workload before/after
- checksum status
- skipped commands with reason

