//! Every read-only list method, called on a receiver the lowering cannot type.
//!
//! `fn show(xs) { return xs.join(", "); }` is how a list method is usually
//! written, and it is the shape that had no coverage: the receiver reaches
//! `lower_method` as `Ty::Dyn`, which unboxes through `dyn.as_list` only when
//! the name's `METHOD_TABLE` row says `unbox_list`. Six names were missing
//! that row or had it wrong, each found separately, each by a program that
//! stopped compiling — `chain` said `false` while `concat`, which shares its
//! dispatch arm, said `true`; `first`, `last`, `index_of` and `count` had no
//! row at all though their arms accepted `ListDyn` already.
//!
//! Nothing gated it. A missing row is not a wrong answer — the program still
//! runs, on the VM, about three times slower and with no diagnostic — so the
//! differential corpora, the coverage gate and the fuzzer are all green for it.
//! This test is the gate: the name list comes from the VM's own
//! `list_dispatch`, and a name that does not lower has to be in [`EXCLUDED`]
//! with a reason.
//!
//! [`EXCLUDED`] is asserted in both directions. A name that starts lowering
//! must leave the list, so it cannot quietly become a place to park failures.

use std::path::Path;

/// `(method, call)` for every read-only list method the VM dispatches.
///
/// Read from `core/src/vm/context/core_methods/list_dispatch.rs`, minus the six
/// that mutate — `clear`, `insert`, `pop`, `push`, `remove_at`, `set` — which
/// must *not* unbox: `dyn.as_list` materializes a copy for three of the four
/// list representations, so a write through it lands on the copy.
/// `no_unbox_list_name_mutates_its_receiver` in the lowering says the same
/// thing from the other side.
const READ_ONLY: &[(&str, &str)] = &[
    ("chunk", "chunk(2)"),
    ("contains", "contains(1)"),
    ("count", "count(1)"),
    ("enumerate", "enumerate()"),
    ("first", "first()"),
    ("flatten", "flatten()"),
    ("get", "get(0)"),
    ("index_of", "index_of(1)"),
    ("is_empty", "is_empty()"),
    ("join", "join(\",\")"),
    ("last", "last()"),
    ("reverse", "reverse()"),
    ("skip", "skip(1)"),
    ("slice", "slice(0, 1)"),
    ("sort", "sort()"),
    ("sum", "sum()"),
    ("take", "take(1)"),
    ("to_bytes", "to_bytes()"),
    ("unique", "unique()"),
    ("zip", "zip([9])"),
    // Not in `list_dispatch` — they reach lists through the shared reduction
    // and iterator paths — but they are list methods a program writes, and
    // they are in the same position.
    ("min", "min()"),
    ("max", "max()"),
    ("map", "map(|v| v)"),
    ("filter", "filter(|v| true)"),
    ("reduce", "reduce(|a, b| a)"),
];

/// Names that do not lower on a boxed receiver, and why.
///
/// Each is a decision, not a gap left open. Removing a name from here means it
/// now lowers, which the test below also checks — a stale exclusion fails.
const EXCLUDED: &[(&str, &str)] = &[
    (
        "slice",
        "answers a *window* over the receiver, and `dyn.as_list` materializes a \
         plain list for three of the four carriers — so unboxing changes the \
         answer's kind. `\"\" + xs.slice(0, 1)` raises in the VM, which is what a \
         window does, and answered a list when the row said `unbox_list`.",
    ),
    (
        "sort",
        "its order is `compare_runtime_values` across *kinds*, which needs two \
         rank tables, a depth-limited recursive list comparison and the slice \
         view — a mirror of that size wants its own conformance test (see \
         `vm_mirror`), not a copy.",
    ),
    ("min", "same order as `sort`, same missing mirror."),
    ("max", "same order as `sort`, same missing mirror."),
    (
        "sum",
        "adds across kinds, which is `dyn.add` folded over the elements — \
         reachable, but it is a second summation rule until it is written \
         against the VM's.",
    ),
    (
        "reduce",
        "has its `METHOD_TABLE` row; the `ListDyn` dispatch arm is missing.",
    ),
    ("is_empty", "no `ListDyn` dispatch arm."),
    ("to_bytes", "no `ListDyn` dispatch arm."),
    ("zip", "no `ListDyn` dispatch arm."),
];

#[test]
fn every_read_only_list_method_takes_a_boxed_receiver() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut unexpectedly_refused = Vec::new();
    let mut unexpectedly_lowered = Vec::new();

    for (name, call) in READ_ONLY {
        // Two call sites with different element carriers join the parameter to
        // `Dyn`, which is the receiver type under test. Without the second
        // call the parameter would be inferred as one concrete list.
        let source = dir.path().join(format!("{name}.lk"));
        std::fs::write(
            &source,
            format!(
                "fn probe(xs) {{\n    return xs.{call};\n}}\nprintln(probe([1, 2]));\nprintln(probe([1.5, 2.5]));\n"
            ),
        )
        .expect("write probe");

        let lowers = lowers_natively(&source, &dir.path().join(format!("{name}_exe")));
        let excluded = EXCLUDED.iter().any(|(excluded, _)| excluded == name);
        match (lowers, excluded) {
            (false, false) => unexpectedly_refused.push(*name),
            (true, true) => unexpectedly_lowered.push(*name),
            _ => {}
        }
    }

    assert!(
        unexpectedly_refused.is_empty(),
        "these list methods refuse a boxed receiver and are not in EXCLUDED: {unexpectedly_refused:?}. \
         A program written `fn f(xs) {{ return xs.NAME(); }}` falls to the VM for each of them, \
         silently. Add the `METHOD_TABLE` row (and the `ListDyn` arm, if it is missing), or list \
         the name in EXCLUDED with the reason it cannot."
    );
    assert!(
        unexpectedly_lowered.is_empty(),
        "these are in EXCLUDED but now lower: {unexpectedly_lowered:?}. Remove them — an exclusion \
         that no longer holds is what makes the list stop meaning anything."
    );
}

/// Whether `source` compiles with the bridge off and fallback forbidden.
///
/// Both are pinned, because a fallback compiles and runs and prints the right
/// answer — which is exactly why a missing row survived so long.
fn lowers_natively(source: &Path, exe: &Path) -> bool {
    std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
        .args(["compile", source.to_str().expect("utf-8 path")])
        .arg("--output")
        .arg(exe.to_str().expect("utf-8 path"))
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .output()
        .expect("run lk compile")
        .status
        .success()
}
