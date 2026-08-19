//! MIR snapshot tests (docs/aot/aot-redesign.md §6): real LK source is compiled
//! to bytecode, lowered, and rendered with `lk_aot_mir::render`. The snapshots are
//! the review surface for lowering changes — they are stable against codegen
//! (LLVM-text) churn by construction.
//!
//! When a lowering change intentionally shifts a snapshot, update the golden text
//! after reviewing the printed diff (`cargo test -p lk-aot-lower --test
//! mir_snapshots -- --nocapture` prints the actual rendering on mismatch).

use lk_core::syntax::{ParseOptions, parse_program_source};
use lk_core::vm::{Compiler, ModuleArtifact};

fn mir_text(source: &str) -> String {
    let program = parse_program_source(source, ParseOptions::default()).expect("parse");
    let module = Compiler::compile_module(&program).expect("compile");
    let artifact = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let mir = lk_aot_lower::lower(&artifact).expect("lower");
    lk_aot_mir::validate(&mir).expect("validate");
    lk_aot_mir::render(&mir)
}

#[track_caller]
fn assert_snapshot(source: &str, expected: &str) {
    let actual = mir_text(source);
    assert_eq!(
        actual.trim(),
        expected.trim(),
        "MIR snapshot mismatch.\n--- source ---\n{source}\n--- actual ---\n{actual}"
    );
}

/// `/` yields a `Float`, so two `Int`s widen and divide as `f64`.
///
/// This snapshot used to hold `int.div … -> i64`, pinning the one place that
/// disagreed with the rest of the language: the checker, the constant folder
/// and the `dyn.*` helpers all said `/` produces a `Float`, and only the typed
/// lowering (and the VM's typed fast path) divided as integers. A native
/// `7 / 2` answered `3` where the VM answered `3.5`.
#[test]
fn straightline_division() {
    assert_snapshot(
        "let x = 20;\nlet y = 4;\nreturn x / y;\n",
        r#"
mir module (abi v1)
fn f0() -> f64 entry {
bb0():
  v0 = const.i64 20
  v1 = const.i64 4
  v2 = sitofp v0
  v3 = sitofp v1
  v4 = float.div v2, v3
  ret v4
}
"#,
    );
}

#[test]
fn if_else_merge() {
    assert_snapshot(
        "let a = 3;\nlet b = 5;\nif a < b { return a; }\nreturn b;\n",
        r#"
mir module (abi v1)
fn f0() -> i64 entry {
bb0():
  v0 = const.i64 3
  v1 = const.i64 5
  v2 = icmp.lt v0, v1
  condbr v2, bb1(), bb2()
bb1():
  ret v0
bb2():
  ret v1
}
"#,
    );
}

#[test]
fn counted_loop() {
    assert_snapshot(
        "let s = 0;\nlet i = 0;\nwhile (i < 3) { s = s + i; i = i + 1; }\nreturn s;\n",
        r#"
mir module (abi v1)
fn f0() -> i64 entry {
bb0():
  v0 = const.i64 0
  v1 = const.i64 0
  v2 = const.i64 3
  v3 = const.i64 1
  br bb1(v1, v0)
bb1(v4: i64, v7: i64):
  v5 = const.i64 3
  v6 = icmp.lt v4, v5
  condbr v6, bb2(), bb3()
bb2():
  v8 = int.add v7, v4
  v9 = const.i64 1
  v10 = int.add v4, v9
  br bb1(v10, v8)
bb3():
  ret v7
}
"#,
    );
}

#[test]
fn direct_call() {
    assert_snapshot(
        "fn add(a, b) { return a + b; }\nreturn add(3, 4);\n",
        r#"
mir module (abi v1)
fn f0() -> i64 entry {
bb0():
  v0 = const.i64 3
  v1 = const.i64 4
  v2 = call f1(v0, v1)
  ret v2
}
fn f1(v0: i64, v1: i64) -> i64 {
bb0():
  v2 = int.add v0, v1
  ret v2
}
"#,
    );
}

#[test]
fn list_literal_and_dynamic_index() {
    assert_snapshot(
        "let xs = [10, 20, 30];\nlet i = 0;\nlet s = 0;\nwhile (i < 3) { s = s + xs[i]; i = i + 1; }\nreturn s;\n",
        r#"
mir module (abi v1)
global g0 = "Add expected numbers or strings, got Int and Nil"
fn f0() -> i64 entry {
bb0():
  v0 = call list_h.i64_new()
  v1 = const.i64 10
  call list_h.i64_push(v0, v1)
  v2 = const.i64 20
  call list_h.i64_push(v0, v2)
  v3 = const.i64 30
  call list_h.i64_push(v0, v3)
  v4 = const.i64 0
  v5 = const.i64 0
  v6 = const.i64 3
  v7 = const.i64 1
  br bb1(v4, v0, v5)
bb1(v8: i64, v11: list<i64>, v13: i64):
  v9 = const.i64 3
  v10 = icmp.lt v8, v9
  condbr v10, bb2(), bb3()
bb2():
  v12 = list.i64.get_maybe v11, v8
  v14 = maybe.present<maybe<i64>> v12
  v15 = zext.bool v14
  v16 = const.str g0
  call rt.maybe_guard(v15, v16)
  v17 = maybe.value<maybe<i64>> v12
  v18 = int.add v13, v17
  v19 = const.i64 1
  v20 = int.add v8, v19
  br bb1(v20, v11, v18)
bb3():
  ret v13
}
"#,
    );
}

#[test]
fn string_map_lookup() {
    assert_snapshot(
        "let m = {\"a\": 1, \"b\": 2};\nreturn m[\"b\"];\n",
        r#"
mir module (abi v1)
global g0 = "a"
global g1 = "b"
fn f0() -> maybe<i64> entry {
bb0():
  v0 = call map_h.str_i64_new()
  v1 = const.str g0
  v2 = const.i64 1
  call map_h.str_i64_set(v0, v1, v2)
  v3 = const.str g1
  v4 = const.i64 2
  call map_h.str_i64_set(v0, v3, v4)
  v5 = const.str g1
  v6 = map.str_i64.get_maybe v0, v5
  ret v6
}
"#,
    );
}
