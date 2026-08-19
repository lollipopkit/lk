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
//!
//! And the interpreter has to accept each probe first, which is a check on the
//! *test* rather than on the compiler. Without it a probe with the wrong
//! signature refuses to lower for a reason that has nothing to do with the
//! receiver and lands in [`EXCLUDED`] looking like a finding: `reduce` is
//! `reduce(initial, f)` and was written `reduce(f)`, so a name that lowers
//! correctly sat in the list with an invented excuse; `to_bytes` was probed
//! with floats it refuses. Two of the entries were about the probe.

use std::path::Path;

/// `(method, call, first, second)` for every read-only list method the VM
/// dispatches — the call, and the two call-site arguments that join its
/// receiver to `Dyn`.
///
/// The two arguments have to differ in *carrier*, which is what makes the
/// parameter erase; a single `xs: Any` annotation does not do it, because the
/// lowering specializes on the one call site it can see. `to_bytes` is why the
/// pair is per-name rather than fixed: it wants Ints, so its second site is a
/// mixed list narrowed back to one.
///
/// Read from `core/src/vm/context/core_methods/list_dispatch.rs`, minus the six
/// that mutate — `clear`, `insert`, `pop`, `push`, `remove_at`, `set` — which
/// must *not* unbox: `dyn.as_list` materializes a copy for three of the four
/// list representations, so a write through it lands on the copy.
/// `no_unbox_list_name_mutates_its_receiver` in the lowering says the same
/// thing from the other side.
const READ_ONLY: &[(&str, &str, &str, &str)] = &[
    ("chunk", "chunk(2)", "[1, 2]", "[1.5, 2.5]"),
    ("contains", "contains(1)", "[1, 2]", "[1.5, 2.5]"),
    ("count", "count(1)", "[1, 2]", "[1.5, 2.5]"),
    ("enumerate", "enumerate()", "[1, 2]", "[1.5, 2.5]"),
    ("first", "first()", "[1, 2]", "[1.5, 2.5]"),
    ("flatten", "flatten()", "[1, 2]", "[1.5, 2.5]"),
    ("get", "get(0)", "[1, 2]", "[1.5, 2.5]"),
    ("index_of", "index_of(1)", "[1, 2]", "[1.5, 2.5]"),
    ("is_empty", "is_empty()", "[1, 2]", "[1.5, 2.5]"),
    ("join", "join(\",\")", "[1, 2]", "[1.5, 2.5]"),
    ("last", "last()", "[1, 2]", "[1.5, 2.5]"),
    ("reverse", "reverse()", "[1, 2]", "[1.5, 2.5]"),
    ("skip", "skip(1)", "[1, 2]", "[1.5, 2.5]"),
    ("slice", "slice(0, 1)", "[1, 2]", "[1.5, 2.5]"),
    ("sort", "sort()", "[1, 2]", "[1.5, 2.5]"),
    ("sum", "sum()", "[1, 2]", "[1.5, 2.5]"),
    ("take", "take(1)", "[1, 2]", "[1.5, 2.5]"),
    ("to_bytes", "to_bytes()", "[1, 2]", "[3, \"x\"].take(1)"),
    ("unique", "unique()", "[1, 2]", "[1.5, 2.5]"),
    ("zip", "zip([9])", "[1, 2]", "[1.5, 2.5]"),
    // Not in `list_dispatch` — they reach lists through the shared reduction
    // and iterator paths — but they are list methods a program writes, and
    // they are in the same position.
    ("min", "min()", "[1, 2]", "[1.5, 2.5]"),
    ("max", "max()", "[1, 2]", "[1.5, 2.5]"),
    ("map", "map(|v| v)", "[1, 2]", "[1.5, 2.5]"),
    ("filter", "filter(|v| true)", "[1, 2]", "[1.5, 2.5]"),
    ("reduce", "reduce(0, |a, b| a)", "[1, 2]", "[1.5, 2.5]"),
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
        "to_bytes",
        "no `ListDyn` dispatch arm: `bytes_h.from_i64_list` takes the typed carrier, \
         and a boxed list has to check every element is an Int in byte range first.",
    ),
];

#[test]
fn every_read_only_list_method_takes_a_boxed_receiver() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut unexpectedly_refused = Vec::new();
    let mut unexpectedly_lowered = Vec::new();

    for (name, call, first, second) in READ_ONLY {
        // Two call sites with different element carriers join the parameter to
        // `Dyn`, which is the receiver type under test. Without the second
        // call the parameter would be inferred as one concrete list.
        let source = dir.path().join(format!("{name}.lk"));
        std::fs::write(
            &source,
            format!(
                "fn probe(xs) {{\n    return xs.{call};\n}}\nprintln(probe({first}));\nprintln(probe({second}));\n"
            ),
        )
        .expect("write probe");

        // The interpreter has to accept it first. A probe with the wrong
        // signature refuses to lower for a reason that has nothing to do with
        // the receiver, and lands in `EXCLUDED` looking like a finding —
        // `reduce` is `reduce(initial, f)` and was written `reduce(f)`, so a
        // name that lowers correctly sat in the list with an invented excuse.
        let interpreted = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
            .arg(source.to_str().expect("utf-8 path"))
            .env("LK_FORCE_VM", "1")
            .output()
            .expect("run under the VM");
        assert!(
            interpreted.status.success(),
            "the probe for `{name}` is not a program the interpreter accepts, so what it \
             measures is the probe: {}",
            String::from_utf8_lossy(&interpreted.stderr)
        );

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
