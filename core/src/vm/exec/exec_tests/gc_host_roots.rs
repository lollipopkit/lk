//! Host-root regressions: values a native HOF holds in Rust across re-entrant
//! VM calls (accumulated `map` results, the `reduce` accumulator, materialized
//! item snapshots) must be pinned via `RuntimeModuleState::host_roots` or a
//! collection inside the next callback frees them. Runs with the GC threshold
//! pinned to 1 — the deterministic twin of `LK_GC_STRESS=1` — so a missing
//! root fails in any test run (regression: `json_process.lk` under the CI GC
//! stress job returned `[[Carol,293]] * 3` from a `map` building fresh lists).

use super::*;
use crate::syntax::{ParseOptions, parse_program_source};

fn run_stressed(source: &str) -> crate::vm::ProgramResult {
    let program = parse_program_source(source, ParseOptions::default()).expect("parse");
    let mut ctx = VmContext::new();
    crate::vm::execute_program_with_ctx_and_gc_threshold(&program, &mut ctx, 1).expect("execute stressed")
}

#[test]
fn list_map_results_survive_gc_during_callbacks() {
    let result = run_stressed(
        "let out = [1, 2, 3].map(|u| [u, u * 2]);\n\
         return out[0][0] + out[0][1] + out[1][1] + out[2][1];\n",
    );
    assert_eq!(result.returns, vec![RuntimeVal::Int(1 + 2 + 4 + 6)]);
}

#[test]
fn list_reduce_heap_accumulator_survives_gc_during_callbacks() {
    let result = run_stressed(
        "let out = [1, 2, 3].reduce([], |acc, x| acc.concat([x * 2]));\n\
         return out[0] + out[1] + out[2];\n",
    );
    assert_eq!(result.returns, vec![RuntimeVal::Int(2 + 4 + 6)]);
}

#[test]
fn list_filter_materialized_long_string_items_survive_gc() {
    // Long (>7 byte) strings are materialized onto the heap when the typed
    // receiver list is snapshotted into `RuntimeVal` items — those items are
    // host-held across the predicate callbacks. The predicate must allocate
    // (the `+` concat) so a freed item's slot actually gets reused.
    let result = run_stressed(
        "let kept = [\"aaaaaaaaaaaa-one\", \"bbbbbbbbbbbb-two\", \"cccccccccccc-three\"]\n\
             .filter(|s| (s + \"!\").len() > 17);\n\
         return kept.len() == 1 && kept[0] == \"cccccccccccc-three\";\n",
    );
    assert_eq!(result.returns, vec![RuntimeVal::Bool(true)]);
}

#[test]
fn nested_map_over_map_results_survives_gc() {
    // The json_process.lk shape: an outer map whose callback allocates, over
    // results that were themselves built by callbacks.
    let result = run_stressed(
        "let rows = [1, 2, 3].map(|u| [u, u * 10]);\n\
         let tags = rows.map(|r| [r[0], r[1] + 1]);\n\
         return tags[0][1] + tags[1][1] + tags[2][1];\n",
    );
    assert_eq!(result.returns, vec![RuntimeVal::Int(11 + 21 + 31)]);
}
