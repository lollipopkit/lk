//! Every declared built-in method, called on a receiver the lowering can type.
//!
//! [`boxed_receiver_coverage_test`] asks the same question about a *boxed*
//! list receiver. This one asks it about the plain case — `"abc".upper()`,
//! `m.keys()`, `s.union(t)` — across all six receiver kinds, and it derives
//! the list from [`BUILTIN_METHODS`] rather than restating it, so a method
//! added to the language is in this gate the moment it is declared.
//!
//! A method that does not lower is not a wrong answer: the program runs, on
//! the VM, roughly three times slower, with no diagnostic. Nothing else in the
//! tree gates it — the differential corpora compare *answers*, the coverage
//! scan only walks `examples/`, and an example is written to demonstrate a
//! feature rather than to reach every method.
//!
//! [`EXCLUDED`] is asserted in both directions, so a name that starts lowering
//! has to leave the list and it cannot become a place to park failures.
//!
//! Each probe must be a program the interpreter accepts before its lowering
//! means anything: a probe with the wrong arity refuses to lower for a reason
//! that has nothing to do with coverage, and would land in [`EXCLUDED`]
//! looking like a finding. That check is on the test, not on the compiler.

use lk_core::typ::{BUILTIN_METHODS, BuiltinReceiverKind};
use std::path::Path;

/// A receiver kind, an expression of that kind, and how to spell the parts its
/// method signatures are written against (`Elem`, `Key`, `Val`).
struct Receiver {
    kind: BuiltinReceiverKind,
    /// Named in the failure message.
    label: &'static str,
    expr: &'static str,
    elem: &'static str,
    key: &'static str,
    val: &'static str,
}

/// One entry per *carrier*, not per kind: the lowering matches on the element
/// and value representation, so `Map<String, Int>` and `Map<String, Bool>` are
/// different arms of the same method name. A single receiver per kind is what
/// let `println(m.get(k))` on a `Map<String, Bool>` emit ill-typed IR — three
/// separate call sites read a `Maybe`'s value half as a machine word when a
/// `MaybeBool` hands back a `Bool` — while the `Map<String, Int>` probe passed.
const RECEIVERS: &[Receiver] = &[
    Receiver {
        kind: BuiltinReceiverKind::List,
        label: "List<Int>",
        expr: "[3, 1, 2]",
        elem: "1",
        key: "1",
        val: "1",
    },
    Receiver {
        kind: BuiltinReceiverKind::List,
        label: "List<Float>",
        expr: "[3.5, 1.5, 2.5]",
        elem: "1.5",
        key: "1.5",
        val: "1.5",
    },
    Receiver {
        kind: BuiltinReceiverKind::List,
        label: "List<String>",
        expr: "[\"c\", \"a\", \"b\"]",
        elem: "\"a\"",
        key: "\"a\"",
        val: "\"a\"",
    },
    Receiver {
        kind: BuiltinReceiverKind::List,
        label: "List<Any>",
        expr: "[3, \"a\", 2.5]",
        elem: "\"a\"",
        key: "\"a\"",
        val: "\"a\"",
    },
    Receiver {
        kind: BuiltinReceiverKind::Map,
        label: "Map<String, Float>",
        expr: "{\"k\": 1.5, \"j\": 2.5}",
        elem: "\"k\"",
        key: "\"k\"",
        val: "1.5",
    },
    Receiver {
        kind: BuiltinReceiverKind::Map,
        label: "Map<String, Bool>",
        expr: "{\"k\": true, \"j\": false}",
        elem: "\"k\"",
        key: "\"k\"",
        val: "true",
    },
    Receiver {
        kind: BuiltinReceiverKind::Map,
        label: "Map<String, String>",
        expr: "{\"k\": \"v\", \"j\": \"w\"}",
        elem: "\"k\"",
        key: "\"k\"",
        val: "\"v\"",
    },
    Receiver {
        kind: BuiltinReceiverKind::Map,
        label: "Map<Int, Int>",
        expr: "{1: 2, 3: 4}",
        elem: "1",
        key: "1",
        val: "2",
    },
    Receiver {
        kind: BuiltinReceiverKind::Set,
        label: "Set<String>",
        expr: "Set([\"a\", \"b\"])",
        elem: "\"a\"",
        key: "\"a\"",
        val: "\"a\"",
    },
    Receiver {
        kind: BuiltinReceiverKind::Bytes,
        label: "Bytes",
        expr: "\"abc\".bytes()",
        elem: "98",
        key: "98",
        val: "98",
    },
    Receiver {
        kind: BuiltinReceiverKind::Slice,
        label: "Slice",
        expr: "[3, 1, 2].slice(0, 2)",
        elem: "1",
        key: "1",
        val: "1",
    },
    Receiver {
        kind: BuiltinReceiverKind::Map,
        label: "Map<String, Int>",
        expr: "{\"k\": 1, \"j\": 2}",
        elem: "\"k\"",
        key: "\"k\"",
        val: "1",
    },
    Receiver {
        kind: BuiltinReceiverKind::Set,
        label: "Set<Int>",
        expr: "Set([1, 2])",
        elem: "1",
        key: "1",
        val: "1",
    },
    Receiver {
        kind: BuiltinReceiverKind::Str,
        label: "Str",
        expr: "\"abc\"",
        elem: "\"a\"",
        key: "\"a\"",
        val: "\"a\"",
    },
];

/// `(receiver, method)` pairs whose generated call is not the call a program
/// would write, with the argument text to use instead.
///
/// Only for signatures a type text cannot pin down: a format template has to
/// agree with its own argument count, and a callback's body has to return what
/// the method does something with.
const ARG_OVERRIDE: &[(BuiltinReceiverKind, &str, &str)] = &[
    (BuiltinReceiverKind::List, "map", "|v| v + 1"),
    (BuiltinReceiverKind::List, "filter", "|v| v > 1"),
    (BuiltinReceiverKind::List, "reduce", "0, |a, b| a + b"),
    (BuiltinReceiverKind::Slice, "map", "|v| v + 1"),
    (BuiltinReceiverKind::Slice, "filter", "|v| v > 1"),
    (BuiltinReceiverKind::Slice, "reduce", "0, |a, b| a + b"),
    (BuiltinReceiverKind::Bytes, "map", "|v| v + 1"),
    (BuiltinReceiverKind::Bytes, "filter", "|v| v > 1"),
    (BuiltinReceiverKind::Bytes, "reduce", "0, |a, b| a + b"),
];

/// [`ARG_OVERRIDE`] for one carrier rather than a whole kind, consulted first.
///
/// A callback's body has to type against the *element*, and a kind's carriers
/// do not share one: `|v| v > 1` is a list predicate for three of the four list
/// carriers and a type error for the string one. Without an entry here that
/// carrier's `filter` would simply be skipped as inapplicable, which is the
/// coverage this test exists to have.
const ARG_OVERRIDE_BY_CARRIER: &[(&str, &str, &str)] = &[
    ("List<String>", "filter", "|v| v > \"a\""),
    ("List<String>", "reduce", "\"\", |a, b| a + b"),
    ("List<Any>", "filter", "|v| v == 1"),
    ("List<Any>", "reduce", "\"\", |a, b| a + b"),
    ("List<Float>", "reduce", "0.0, |a, b| a + b"),
];

/// A receiver expression to use instead of the kind's default, for the methods
/// whose default receiver would raise or answer nothing to lower.
const RECEIVER_OVERRIDE: &[(BuiltinReceiverKind, &str, &str)] = &[
    // A template's placeholder count has to match its arguments.
    (BuiltinReceiverKind::Str, "format", "\"v={}\""),
];

/// Methods that do not lower on a typed receiver, and why.
///
/// Each is a decision. A name here that starts lowering fails the test, so the
/// list cannot go stale in the quiet direction.
const EXCLUDED: &[(BuiltinReceiverKind, &str, &str)] = &[];

#[test]
fn every_builtin_method_lowers_on_a_typed_receiver() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut bad_probes = Vec::new();
    let mut refused = Vec::new();
    let mut stale_exclusions = Vec::new();
    let mut checked = 0usize;
    let mut inapplicable = 0usize;

    // Written and type-checked first, all of them, because whether a refusal is
    // a broken probe or a method that does not apply to this carrier is only
    // answerable across the kind: `["a"].sum()` is refused and `[1].sum()` is
    // not, and the same generator wrote both.
    let mut probes = Vec::new();
    for recv in RECEIVERS {
        for sig in BUILTIN_METHODS.iter().filter(|s| s.receiver == recv.kind) {
            // Both arities, when they differ. Omitting an optional parameter
            // is a *different* call for the lowering to match — it matches on
            // the argument list — and it is where the first gap this test found
            // was: every carrier lowered `xs.slice(a, b)` and a window alone
            // refused `xs.slice(a)`.
            let override_args = ARG_OVERRIDE_BY_CARRIER
                .iter()
                .find(|(label, name, _)| *label == recv.label && name == &sig.name)
                .map(|(_, _, text)| *text)
                .or_else(|| {
                    ARG_OVERRIDE
                        .iter()
                        .find(|(kind, name, _)| *kind == recv.kind && name == &sig.name)
                        .map(|(_, _, text)| *text)
                });
            let arities: Vec<String> = match override_args {
                Some(text) => vec![text.to_string()],
                None => {
                    let arg_texts: Vec<String> = sig.params.iter().map(|p| argument_for(p.ty, recv)).collect();
                    let required = sig.params.iter().filter(|p| !p.optional).count();
                    let mut forms = vec![arg_texts[..required].join(", ")];
                    if required < arg_texts.len() {
                        forms.push(arg_texts.join(", "));
                    }
                    forms
                }
            };
            let receiver_expr = RECEIVER_OVERRIDE
                .iter()
                .find(|(kind, name, _)| *kind == recv.kind && name == &sig.name)
                .map(|(_, _, expr)| *expr)
                .unwrap_or(recv.expr);

            for (form, args) in arities.iter().enumerate() {
                let stem = format!(
                    "{}_{}_{form}",
                    recv.label
                        .to_lowercase()
                        .replace(['<', '>', ',', ' '], "_")
                        .trim_matches('_'),
                    sig.name
                );
                let source = dir.path().join(format!("{stem}.lk"));
                // Through a binding rather than off the literal: that is how a
                // program writes it, and a mutating method needs a place to
                // write.
                std::fs::write(
                    &source,
                    format!("let r = {receiver_expr};\nprintln(r.{}({args}));\n", sig.name),
                )
                .expect("write probe");
                let accepted = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
                    .args(["check", source.to_str().expect("utf-8 path")])
                    .output()
                    .expect("run lk check");
                probes.push((
                    recv,
                    sig.name,
                    form,
                    format!("{}.{}({args})", recv.label, sig.name),
                    source,
                    dir.path().join(stem),
                    accepted.status.success(),
                    String::from_utf8_lossy(&accepted.stderr).trim().replace('\n', " "),
                ));
            }
        }
    }

    for (recv, method, form, label, source, exe, accepted, message) in &probes {
        if !accepted {
            // Refused for this carrier. If some other carrier of the same kind
            // accepts the identical call, the refusal is the language saying
            // the method does not apply there. If *none* does, the generator
            // wrote something that is not a call, and that is a bug in the test.
            let applies_somewhere = probes.iter().any(|(other, other_method, other_form, .., ok, _)| {
                other.kind == recv.kind && other_method == method && other_form == form && *ok
            });
            if applies_somewhere {
                inapplicable += 1;
            } else {
                bad_probes.push(format!("{label}: {message}"));
            }
            continue;
        }
        // Checked but raising means the same thing one step later: `[1,
        // "a"].sort()` type-checks and refuses at run time.
        let interpreted = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
            .arg(source.to_str().expect("utf-8 path"))
            .env("LK_FORCE_VM", "1")
            .output()
            .expect("run under the VM");
        if !interpreted.status.success() {
            inapplicable += 1;
            continue;
        }

        checked += 1;
        let lowers = lowers_natively(source, exe);
        let excluded = EXCLUDED
            .iter()
            .any(|(kind, name, _)| *kind == recv.kind && name == method);
        match (lowers, excluded) {
            (false, false) => refused.push(label.clone()),
            (true, true) => stale_exclusions.push(label.clone()),
            _ => {}
        }
    }

    assert!(
        bad_probes.is_empty(),
        "`lk check` rejects these probes, so what they measure is the probe rather than the \
         compiler. Fix the generated call (ARG_OVERRIDE / RECEIVER_OVERRIDE):\n  {}",
        bad_probes.join("\n  ")
    );
    // A skipped probe reports nothing, so a change that made most of them raise
    // would empty the gate quietly and read as full coverage. The floor is well
    // under the count at the time of writing (356 compiled, 96 inapplicable)
    // and is here to catch a collapse, not to pin the exact number.
    assert!(
        checked > 250,
        "only {checked} probes reached the compiler ({inapplicable} raised under the VM and were \
         skipped as not applying to their carrier). That is far below what this table generates, so \
         the probes are failing for a reason other than coverage."
    );
    assert!(
        refused.is_empty(),
        "{} of {checked} built-in method calls do not lower natively and are not in EXCLUDED:\n  {}\n\
         A program calling any of them drops its whole module to the VM, silently and about three \
         times slower. Lower it, or list it in EXCLUDED with the reason it cannot be lowered.",
        refused.len(),
        refused.join("\n  ")
    );
    assert!(
        stale_exclusions.is_empty(),
        "these are in EXCLUDED but now lower: {stale_exclusions:?}. Remove them — an exclusion that \
         no longer holds is what makes the list stop meaning anything."
    );
}

/// Every built-in method again, on a receiver the lowering cannot type.
///
/// A parameter reached with two *different carriers of the same kind* is a
/// `Dyn`: `fn empty(c) { c.clear(); }` called with a `Map<String, Int>` and a
/// `Map<String, Float>` has no carrier to specialize to. That is a different
/// set of arms from the typed case above, and only lists had a gate for it
/// (`boxed_receiver_coverage_test`, which also covers the mutating names this
/// deliberately does not re-litigate).
///
/// It found `clear`, which had an arm for every carrier and none for `Dyn`.
///
/// Only kinds this table gives two carriers for can be boxed this way; `Str`
/// and `Bytes` have one apiece, so a probe over them would be the typed case
/// again under another name.
#[test]
fn every_builtin_method_lowers_on_a_boxed_receiver() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut refused = Vec::new();
    let mut checked = 0usize;

    for kind in [
        BuiltinReceiverKind::List,
        BuiltinReceiverKind::Map,
        BuiltinReceiverKind::Set,
    ] {
        let carriers: Vec<&Receiver> = RECEIVERS.iter().filter(|r| r.kind == kind).collect();
        let [first, second, ..] = carriers.as_slice() else {
            continue;
        };
        for sig in BUILTIN_METHODS.iter().filter(|s| s.receiver == kind) {
            // The first carrier's spelling for the arguments: the receiver is
            // erased, so the *call* has to be one both carriers accept, and
            // anything else is reported as inapplicable by the run below.
            let args = match ARG_OVERRIDE.iter().find(|(k, name, _)| *k == kind && name == &sig.name) {
                Some((_, _, text)) => (*text).to_string(),
                None => sig
                    .params
                    .iter()
                    .filter(|p| !p.optional)
                    .map(|p| argument_for(p.ty, first))
                    .collect::<Vec<_>>()
                    .join(", "),
            };
            let label = format!("Dyn({:?}).{}({args})", kind, sig.name);
            let stem = format!("boxed_{:?}_{}", kind, sig.name).to_lowercase();
            let source = dir.path().join(format!("{stem}.lk"));
            std::fs::write(
                &source,
                format!(
                    "fn probe(r) {{\n    return r.{}({args});\n}}\nprintln(probe({}));\nprintln(probe({}));\n",
                    sig.name, first.expr, second.expr
                ),
            )
            .expect("write probe");

            // Checked and run first, for the same reason the typed sweep does
            // it: a method that does not apply to one of the two carriers is a
            // fact about the language, not a coverage gap.
            if !std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
                .args(["check", source.to_str().expect("utf-8 path")])
                .output()
                .expect("run lk check")
                .status
                .success()
            {
                continue;
            }
            if !std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
                .arg(source.to_str().expect("utf-8 path"))
                .env("LK_FORCE_VM", "1")
                .output()
                .expect("run under the VM")
                .status
                .success()
            {
                continue;
            }

            checked += 1;
            // `slice` answers a *window* over the receiver, and reaching a
            // boxed one means `dyn.as_list`, which materializes a plain list
            // for three of the four carriers — so unboxing would change the
            // answer's kind. `boxed_receiver_coverage_test` excludes it for
            // the same reason and states it at length.
            if sig.name == "slice" {
                continue;
            }
            if !lowers_natively(&source, &dir.path().join(stem)) {
                refused.push(label);
            }
        }
    }

    assert!(
        checked > 40,
        "only {checked} boxed-receiver probes reached the compiler, far below what this table \
         generates — they are failing for a reason other than coverage"
    );
    assert!(
        refused.is_empty(),
        "{} of {checked} built-in methods do not lower on a boxed receiver:\n  {}\n\
         A program whose container parameter meets two carriers drops its whole module to the VM \
         for each of them, silently.",
        refused.len(),
        refused.join("\n  ")
    );
}

/// An expression of the declared parameter type, with the receiver's own
/// spelling substituted for the placeholders the table writes signatures
/// against.
fn argument_for(ty: &str, recv: &Receiver) -> String {
    match ty {
        "Int" => "1".to_string(),
        "Bool" => "true".to_string(),
        "String" => "\"a\"".to_string(),
        "Any" => recv.elem.to_string(),
        "Bytes" => "\"z\".bytes()".to_string(),
        "Elem" => recv.elem.to_string(),
        "Key" => recv.key.to_string(),
        "Val" => recv.val.to_string(),
        "Self" => recv.expr.to_string(),
        "Set<Elem>" => format!("Set([{}])", recv.elem),
        "List<_>" | "List<Elem>" | "List<Any>" => format!("[{}]", recv.elem),
        // `Fn` says only that a callback goes here; the arity and the result
        // type come from the method, which is what ARG_OVERRIDE carries. A
        // signature reaching this arm is a new callback method with no entry.
        "Fn" => panic!("a callback parameter needs an ARG_OVERRIDE entry"),
        other => panic!("no probe argument for the declared parameter type `{other}`"),
    }
}

/// Whether `source` compiles with the bridge off and fallback forbidden.
///
/// Both are pinned, because a fallback compiles and runs and prints the right
/// answer — which is exactly why this class of gap is invisible everywhere
/// else.
fn lowers_natively(source: &Path, exe: &Path) -> bool {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
        .args(["compile", source.to_str().expect("utf-8 path")])
        .arg("--output")
        .arg(exe.to_str().expect("utf-8 path"))
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .output()
        .expect("run lk compile");
    if out.status.success() {
        return true;
    }
    // A failed compile is only an answer about *coverage* when the compiler
    // says so. Everything else — a linker that could not write, a full disk —
    // exits non-zero too, and reading that as "does not lower" reports a
    // coverage regression for a machine problem. A full `/tmp` did exactly
    // that here: the last two receivers in the table failed as a block, which
    // is what a resource running out looks like and not what a lowering gap
    // looks like.
    let message = String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
    assert!(
        message.contains("native AOT does not support this program yet"),
        "`lk compile` failed for a reason that is not a lowering refusal, so this run says nothing \
         about coverage:\n{}",
        message.trim()
    );
    false
}
