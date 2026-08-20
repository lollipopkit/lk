//! Every declared stdlib module function, asked whether it lowers natively.
//!
//! The module counterpart of `builtin_method_native_coverage_test`, and the
//! same class of gap: a member with no ABI row is not a wrong answer, it is a
//! program that runs on the VM about three times slower with no diagnostic.
//! `MODULE_ABI` is the list of members that *do* lower, so reading it says
//! nothing about what is missing from it — only the stdlib's own declaration
//! can.
//!
//! The probes are checked and compiled, never **run**: this surface is `fs`,
//! `net`, `process` and `os`, and running a generated call to it would touch
//! the machine. So a probe's validity is decided by `lk check` alone, and a
//! member the checker refuses for one spelling of its arguments is reported
//! rather than skipped — there is no second carrier to compare it against the
//! way there is for a method.

use lk_core::module::ModuleRegistry;
use lk_core::val::Type;
use std::path::Path;

/// Members that do not lower, and why.
///
/// Each is a decision. Asserted in both directions: a member here that starts
/// lowering fails the test, so the list cannot quietly become a parking space.
const EXCLUDED: &[(&str, &str)] = &[
    (
        "env.get",
        "`m.get(k)` with one argument compiles to a map *read* whatever `m` is, so this arrives as \
         `GetIndex env, \"KEY\"` — the shape a member read of a member named `KEY` also has. \
         Nothing in the bytecode separates them: both carry a constant string key and both record \
         a key fact. Deciding by \"is the key a member name\" would compile a read of any member \
         the lowering table happens to lack into `env.get(\"that name\")`, which is a wrong answer \
         rather than a fallback. The runtime side is not the obstacle.",
    ),
    (
        "http.get",
        "the `http` module has no lkrt implementation: a native binary would need an HTTP client \
         linked into the runtime, and `lkrt` is deliberately small.",
    ),
    ("http.post", "see `http.get`."),
    ("http.request", "see `http.get`."),
    (
        "math.random",
        "a deterministic xorshift over *process-global* state — the same sequence every run, so \
         both back ends have to produce it in step. A second generator in lkrt would have to match \
         bit for bit, and with the hybrid bridge on it would interleave with the VM's copy of the \
         state and diverge from either. A fallback keeps one generator.",
    ),
    (
        "stream.iterate",
        "an unbounded source. The stream lowering is an eager materialization — sound because a \
         finite pipeline with pure lambdas is observationally the same list — and eagerly \
         materializing an infinite one does not terminate.",
    ),
    ("stream.repeat", "see `stream.iterate`."),
    (
        "task.stats",
        "reports the async runtime's internal counters, and a native binary has no async runtime to \
         report on.",
    ),
];

#[test]
fn every_declared_stdlib_module_function_lowers() {
    let mut registry = ModuleRegistry::new();
    lk_stdlib::register_stdlib_modules(&mut registry).expect("stdlib registers");
    let dir = tempfile::tempdir().expect("temp dir");

    let mut refused = Vec::new();
    let mut stale_exclusions = Vec::new();
    let mut unprobeable = Vec::new();
    let mut checked = 0usize;

    for module in &lk_stdlib::stdlib_catalog().modules {
        for export in &module.exports {
            let path = format!("{}.{}", module.name, export.name);
            // No declared signature: an overloaded export, which the checker
            // itself declines to type. Nothing to generate a call from.
            let Some(sig) = lk_core::typ::stdlib_signature(&path) else {
                continue;
            };
            let Some(args) = sig
                .params
                .iter()
                .filter(|p| !p.optional)
                .map(|p| literal_for(&p.ty))
                .collect::<Option<Vec<_>>>()
            else {
                // A parameter type with no obvious literal (a callback, a
                // handle, a struct). Reported, not skipped: the list of what
                // this test cannot reach is part of what it measures.
                unprobeable.push(path);
                continue;
            };
            let call = format!("{path}({})", args.join(", "));
            let stem = path.replace('.', "_");
            let source = dir.path().join(format!("{stem}.lk"));
            let root = module.name.split('.').next().expect("a module name");
            std::fs::write(&source, format!("use {root};\nlet probe = {call};\nprintln(probe);\n"))
                .expect("write probe");

            if !std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
                .args(["check", source.to_str().expect("utf-8 path")])
                .output()
                .expect("run lk check")
                .status
                .success()
            {
                unprobeable.push(path);
                continue;
            }

            checked += 1;
            let lowers = lowers_natively(&source, &dir.path().join(stem));
            let excluded = EXCLUDED.iter().any(|(name, _)| *name == path);
            match (lowers, excluded) {
                (false, false) => refused.push(call),
                (true, true) => stale_exclusions.push(call),
                _ => {}
            }
        }
    }

    assert!(
        checked > 100,
        "only {checked} module members were probed, which is far below what the stdlib declares — \
         the probes are failing for a reason other than coverage. {} could not be given arguments.",
        unprobeable.len()
    );
    assert!(
        refused.is_empty(),
        "{} of {checked} stdlib module functions do not lower natively and are not in EXCLUDED:\n  {}\n\
         A program calling any of them drops its whole module to the VM, silently. Add the ABI row, \
         or list it in EXCLUDED with the reason it cannot be lowered.",
        refused.len(),
        refused.join("\n  ")
    );
    assert!(
        stale_exclusions.is_empty(),
        "these are in EXCLUDED but now lower: {stale_exclusions:?}. Remove them — an exclusion that \
         no longer holds is what makes the list stop meaning anything."
    );
}

/// A literal of the declared parameter type, or `None` when the type is not one
/// a fixed expression can stand in for.
fn literal_for(ty: &Type) -> Option<String> {
    Some(match ty {
        Type::Int => "1".to_string(),
        Type::Float => "1.5".to_string(),
        Type::Bool => "true".to_string(),
        Type::String => "\"probe\"".to_string(),
        Type::Any => "1".to_string(),
        Type::List(elem) => format!("[{}]", literal_for(elem)?),
        Type::Set(elem) => format!("Set([{}])", literal_for(elem)?),
        Type::Map(key, value) => format!("{{{}: {}}}", literal_for(key)?, literal_for(value)?),
        // A union takes whichever arm has a literal.
        Type::Union(arms) => arms.iter().find_map(literal_for)?,
        _ => return None,
    })
}

/// Whether `source` compiles with the bridge off and fallback forbidden.
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
