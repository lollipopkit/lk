//! What every host owes a program, checked against the hosts themselves.
//!
//! LK swaps platform capabilities at the stdlib layer rather than behind a
//! capability-trait HAL: a target without an OS supplies its own
//! `ModuleRegistry` population. That is the design, and it has a failure mode
//! the design does not prevent — a name the desktop host has and another host
//! has *never heard of*.
//!
//! There are two absences and they read differently:
//!
//! * **Unavailable.** `use fs` on bare metal answers "module 'fs' is not
//!   available on bare metal". The reader learns the program needs something
//!   this machine has not got.
//! * **Unknown.** A module in neither the backed list nor the unavailable one
//!   answers "unknown module", which is what a *typo* answers. The reader is
//!   sent to look for a spelling mistake that is not there.
//!
//! The second is what these tests forbid. They ask each host what it knows by
//! building its registry, rather than comparing two hand-written lists — a
//! second list is a thing that drifts, and the drift is invisible until someone
//! runs a program on the smaller host.
//!
//! Found the hard way one level down: `error` — the global a `catch` catches —
//! was missing from both alternative hosts, so every raising program parsed,
//! type-checked, and then failed at run time with "undefined function". Nothing
//! compared the lists, because nothing could.

use lk_core::module::ModuleRegistry;

/// Every module the desktop host registers.
fn desktop_modules() -> Vec<String> {
    let mut registry = ModuleRegistry::new();
    crate::register_stdlib_modules(&mut registry).expect("desktop modules register");
    registry.get_module_names()
}

/// Every global the desktop host registers, by name.
fn desktop_globals() -> Vec<String> {
    let mut registry = ModuleRegistry::new();
    crate::register_stdlib_core_globals(&mut registry);
    crate::register_stdlib_concurrency_globals(&mut registry);
    runtime_builtin_names(&registry)
}

fn runtime_builtin_names(registry: &ModuleRegistry) -> Vec<String> {
    // The two-level `chan::try_send` names and the `$`-bearing internals are not
    // things a program can write, so they are not part of what a host owes one.
    desktop_builtin_candidates()
        .into_iter()
        .filter(|name| registry.get_runtime_builtin(name).is_some())
        .collect()
}

/// The globals a program can actually write. `try$call` and `select$block` are
/// deliberately untypeable, and `chan::try_send` is reached through a method.
fn desktop_builtin_candidates() -> Vec<String> {
    [
        "print",
        "println",
        "panic",
        "error",
        "assert",
        "assert_eq",
        "assert_ne",
        "spawn",
        "chan",
        "send",
        "recv",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

#[test]
fn the_bare_host_knows_every_module_the_desktop_host_has() {
    let mut registry = ModuleRegistry::new();
    lk_stdlib_bare::register_bare_stdlib(&mut registry).expect("bare stdlib registers");
    let known = registry.get_module_names();
    let unknown: Vec<String> = desktop_modules()
        .into_iter()
        .filter(|name| !known.contains(name))
        .collect();
    assert!(
        unknown.is_empty(),
        "bare metal has never heard of {unknown:?} — a program importing one is told it made a \
         typo. Add it to `BARE_MODULES` if it can work without an OS, or to \
         `UNSUPPORTED_MODULES` if it cannot"
    );
}

#[test]
fn the_web_host_knows_every_module_the_desktop_host_has() {
    let mut registry = ModuleRegistry::new();
    lk_stdlib_web::register_web_stdlib(&mut registry).expect("web stdlib registers");
    let known = registry.get_module_names();
    let unknown: Vec<String> = desktop_modules()
        .into_iter()
        .filter(|name| !known.contains(name))
        .collect();
    assert!(
        unknown.is_empty(),
        "the browser host has never heard of {unknown:?} — see the bare-metal test for what the \
         two answers mean"
    );
}

#[test]
fn every_host_has_every_global_a_program_can_write() {
    for (host, registry) in [
        ("bare metal", {
            let mut registry = ModuleRegistry::new();
            lk_stdlib_bare::register_bare_stdlib(&mut registry).expect("bare stdlib registers");
            registry
        }),
        ("the browser", {
            let mut registry = ModuleRegistry::new();
            lk_stdlib_web::register_web_stdlib(&mut registry).expect("web stdlib registers");
            registry
        }),
    ] {
        let missing: Vec<String> = desktop_globals()
            .into_iter()
            .filter(|name| registry.get_runtime_builtin(name).is_none())
            .collect();
        assert!(
            missing.is_empty(),
            "{host} does not register {missing:?}. A global it cannot back is still owed a \
             *refusal* — `spawn` on bare metal answers \"not available on bare metal: there is \
             one task\", which a program can catch. Absent, it answers \"undefined function\", \
             which a program cannot tell from a typo"
        );
    }
}

/// The two boards outside the workspace, and the features they ask this crate
/// for.
///
/// `bare-metal/` and `bare-metal-x86/` are `exclude`d from the workspace — they
/// build only for `thumbv7em-none-eabi` and `x86_64-unknown-none` — so
/// `cargo test --workspace` never touches them and neither does CI. Each names
/// a subset of `stdlib/bare`'s features in its own manifest, which makes the
/// list a thing written down three times.
///
/// Removing a module is therefore three edits, and missing one is a build that
/// fails only when somebody builds that target by hand. That is not a
/// hypothetical: the `slice` module was removed and both boards kept asking for
/// its feature. The x86 kernel was found a day later; the Cortex-M demo — the
/// only thing that shows the no_std VM *running*, on a second architecture —
/// was found the day after that, by looking.
///
/// This test cannot build those targets. What it can do is read their manifests
/// and check that every feature they name still exists here, which is exactly
/// the failure both of them had.
#[test]
fn the_out_of_workspace_boards_ask_for_features_that_exist() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let ours = std::fs::read_to_string(root.join("bare/Cargo.toml")).expect("read stdlib/bare manifest");
    let available = feature_names(&ours);
    assert!(
        available.contains(&"math".to_string()),
        "the feature parse found nothing; it is reading the wrong file or the wrong shape"
    );

    for board in ["bare-metal", "bare-metal-x86"] {
        let manifest = root.join("..").join(board).join("Cargo.toml");
        let text = std::fs::read_to_string(&manifest).unwrap_or_else(|e| panic!("read {}: {e}", manifest.display()));
        let asked = requested_features(&text);
        assert!(
            !asked.is_empty(),
            "{board} names no features for `lk-stdlib-bare`; if that dependency went away, this \
             test should go with it"
        );
        let gone: Vec<&String> = asked.iter().filter(|name| !available.contains(name)).collect();
        assert!(
            gone.is_empty(),
            "{board}/Cargo.toml asks `lk-stdlib-bare` for {gone:?}, which it no longer has. That \
             board is outside the workspace, so nothing else here builds it — the error it gets is \
             `failed to select a version for lk-stdlib-bare`, and only when someone builds that \
             target by hand"
        );
    }
}

/// And the fourth copy of the same list: CI's own.
///
/// `.github/workflows/check.yml` builds each computation-only module *alone* on
/// `thumbv7em-none-eabi`, because a crate that only builds when a sibling
/// happens to enable `std` for it is not actually no_std. The loop names them,
/// which makes this the fourth place the set is written down — after
/// `stdlib/bare`'s features and the two boards' manifests.
///
/// It went stale the same way the boards did: `slice` was removed and the loop
/// kept building `lk-stdlib-slice`. Nothing noticed, because this branch had
/// never been pushed — CI does cover these targets, and would have said so on
/// the first run.
#[test]
fn ci_builds_exactly_the_modules_that_exist() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let workflow = root.join("../.github/workflows/check.yml");
    let text = std::fs::read_to_string(&workflow).unwrap_or_else(|e| panic!("read {}: {e}", workflow.display()));
    let line = text
        .lines()
        .find(|line| line.contains("for m in") && line.contains("cargo build"))
        .or_else(|| text.lines().find(|line| line.trim_start().starts_with("for m in")))
        .expect("the per-module thumbv7em loop; if it was rewritten, so should this test be");
    let named: Vec<String> = line
        .split_once("for m in")
        .expect("checked")
        .1
        .split(';')
        .next()
        .expect("checked")
        .split_whitespace()
        .map(str::to_string)
        .collect();
    assert!(!named.is_empty(), "the loop names no modules");

    let ours = std::fs::read_to_string(root.join("bare/Cargo.toml")).expect("read stdlib/bare manifest");
    let available = feature_names(&ours);
    let gone: Vec<&String> = named.iter().filter(|name| !available.contains(name)).collect();
    assert!(
        gone.is_empty(),
        "check.yml builds `lk-stdlib-{{{gone:?}}}` on thumbv7em, and no such crate exists any          more. CI fails on the first push with a message about the crate rather than about the          list it came from"
    );
}

/// The keys of `stdlib/bare`'s `[features]` table.
fn feature_names(manifest: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_features = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_features = line == "[features]";
            continue;
        }
        if !in_features || line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some((name, _)) = line.split_once('=') {
            names.push(name.trim().to_string());
        }
    }
    names
}

/// The feature names a board's `lk-stdlib-bare` dependency asks for.
fn requested_features(manifest: &str) -> Vec<String> {
    let Some(line) = manifest
        .lines()
        .find(|line| line.trim_start().starts_with("lk-stdlib-bare"))
    else {
        return Vec::new();
    };
    let Some(list) = line.split_once("features").and_then(|(_, rest)| rest.split_once('[')) else {
        return Vec::new();
    };
    let Some((inside, _)) = list.1.split_once(']') else {
        return Vec::new();
    };
    inside
        .split(',')
        .map(|piece| piece.trim().trim_matches('"').to_string())
        .filter(|piece| !piece.is_empty())
        .collect()
}

/// The same program, run on every host, must answer the same thing.
///
/// The tests above check *names*: every host knows every module and every
/// global. That is necessary and it is not enough — `assert_eq` was present on
/// all three hosts and did the wrong thing on two of them, because each host
/// had written its own:
///
/// ```text
/// assert_eq("abcdefghij", "abcdefghij")   passed on desktop, failed on web and bare
/// ```
///
/// A name list cannot see that. Running the program can, so this does: the
/// corpus below is answers, not spellings.
#[cfg(test)]
mod behaviour {
    use lk_core::module::ModuleRegistry;
    use lk_core::stmt::stmt_parser::StmtParser;
    use lk_core::token::Tokenizer;
    use lk_core::vm::{ModuleResolver, ProgramExec, VmContext};
    use std::sync::Arc;

    /// What a host answered: the returned value rendered, or the error text.
    fn outcome(register: impl FnOnce(&mut ModuleRegistry), source: &str) -> String {
        let tokens = match Tokenizer::tokenize(source) {
            Ok(tokens) => tokens,
            Err(error) => return format!("parse error: {error}"),
        };
        let program = match StmtParser::new(&tokens).parse_program() {
            Ok(program) => program,
            Err(error) => return format!("parse error: {error}"),
        };
        let mut registry = ModuleRegistry::new();
        register(&mut registry);
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        match program.execute_with_ctx(&mut env) {
            // The *value*, not its kind: two hosts both answering "a String" is
            // not two hosts agreeing. `show is dispatched` below returns a
            // string on every host and returned a different one on two of them.
            Ok(result) => match lk_stdlib_common::runtime_native::runtime_display_value(
                result.first_return(),
                result.state.heap(),
            ) {
                Ok(rendered) => format!("ok: {rendered}"),
                Err(error) => format!("ok, undisplayable: {error:#}"),
            },
            // The text, not the type: an error a program can `catch` is a
            // value it can read, so two hosts disagreeing about the words is
            // two hosts disagreeing.
            Err(error) => format!("error: {error:#}"),
        }
    }

    fn desktop(source: &str) -> String {
        outcome(
            |registry| {
                crate::register_stdlib_core_globals(registry);
                crate::register_stdlib_modules(registry).expect("desktop modules");
            },
            source,
        )
    }

    fn web(source: &str) -> String {
        outcome(
            |registry| {
                lk_stdlib_web::register_web_stdlib(registry).expect("web host");
            },
            source,
        )
    }

    fn bare(source: &str) -> String {
        outcome(
            |registry| {
                lk_stdlib_bare::register_bare_stdlib(registry).expect("bare host");
            },
            source,
        )
    }

    /// Programs whose answer must not depend on which host runs them.
    ///
    /// Deliberately about the *globals* — the surface every host reimplements
    /// rather than shares, and therefore the surface where they can differ
    /// without anything noticing.
    const CORPUS: &[(&str, &str)] = &[
        // The bug that prompted this: equality across the seven-byte boundary.
        ("assert_eq short", r#"assert_eq("ab", "ab"); return 1;"#),
        ("assert_eq long", r#"assert_eq("abcdefghij", "abcdefghij"); return 1;"#),
        ("assert_eq list", "assert_eq([1, 2], [1, 2]); return 1;"),
        ("assert_eq map", r#"assert_eq({"a": 1}, {"a": 1}); return 1;"#),
        ("assert_eq int", "assert_eq(1, 1); return 1;"),
        // …and the failing side, whose *message* a program can catch.
        ("assert_eq fails", r#"assert_eq("a", "b"); return 1;"#),
        ("assert_ne holds", r#"assert_ne("abcdefghij", "abcdefghik"); return 1;"#),
        ("assert_ne fails", r#"assert_ne("abcdefghij", "abcdefghij"); return 1;"#),
        // Truthiness: only nil and false are falsy.
        ("assert nil", "assert(nil); return 1;"),
        ("assert zero", "assert(0); return 1;"),
        ("assert empty string", r#"assert(""); return 1;"#),
        ("assert empty list", "assert([]); return 1;"),
        ("panic", r#"panic("boom"); return 1;"#),
        // `error`/`catch` — the pair that was missing from both alternative
        // hosts once already.
        (
            "catch a raise",
            r#"try { panic("boom"); } catch e { return e; } return 0;"#,
        ),
        // Interpolation, which the *VM* renders — so this cannot diverge
        // between hosts and is here to say so: `print`'s rendering is the
        // host's and is checked in `formatting` below, template interpolation
        // is not. Confusing the two is how a "parity" case ends up testing
        // nothing (this one did, until the deliberate-break check caught it).
        // `error(v)` carries `v` itself where it can. The doc on it says a host
        // without full VM state falls back to the rendered message — so this
        // asks whether the two alternative hosts have it.
        (
            "error carries a list",
            "try { error([1, 2]); } catch e { return e; } return 0;",
        ),
        (
            "error carries an int",
            "try { error(42); } catch e { return e; } return 0;",
        ),
        (
            "error carries a long string",
            r#"try { error("abcdefghij"); } catch e { return e; } return 0;"#,
        ),
        (
            "interpolation renders in the VM",
            r#"struct P { a: Int }
               let p = P { a: 1 };
               return "${p}";"#,
        ),
    ];

    #[test]
    fn every_host_answers_the_same() {
        let mut differences: Vec<String> = Vec::new();
        for (name, source) in CORPUS {
            let expected = desktop(source);
            for (host, actual) in [("web", web(source)), ("bare", bare(source))] {
                if actual != expected {
                    differences.push(format!("{name} — desktop: {expected}\n              {host}: {actual}"));
                }
            }
        }
        assert!(
            differences.is_empty(),
            "hosts disagree about what these programs do:\n  {}",
            differences.join("\n  ")
        );
    }
}

/// What `print` renders, checked against the one implementation that renders it.
///
/// The formatter lived in all three hosts. They agreed on the interesting parts
/// — `{}` takes the next argument, a hole with nothing left stays a hole — and
/// disagreed on one line: the separator before arguments that run past the last
/// hole. With an empty template, bare metal pushed a space and the other two
/// did not, so `print("", 1, 2)` was `" 1 2"` there and `"1 2"` everywhere else.
///
/// One implementation now (`lk_stdlib_common::language::format_variadic`), so
/// this checks the *rules* rather than three copies against each other. Run
/// through the web host because it is the one that can hand its output back.
#[cfg(test)]
mod formatting {
    use lk_core::module::ModuleRegistry;
    use lk_core::stmt::stmt_parser::StmtParser;
    use lk_core::token::Tokenizer;
    use lk_core::vm::{ModuleResolver, ProgramExec, VmContext};
    use std::sync::Arc;

    fn printed(call_args: &str) -> String {
        let source = format!("print({call_args});");
        let tokens = Tokenizer::tokenize(&source).expect("tokenize");
        let program = StmtParser::new(&tokens).parse_program().expect("parse");
        let mut registry = ModuleRegistry::new();
        lk_stdlib_web::register_web_stdlib(&mut registry).expect("web host");
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        lk_stdlib_web::clear_stdout();
        program.execute_with_ctx(&mut env).expect("run");
        lk_stdlib_web::take_stdout()
    }

    #[test]
    fn a_template_takes_arguments_and_says_what_is_left_over() {
        assert_eq!(printed(""), "");
        assert_eq!(printed(r#""a={}", 1"#), "a=1");
        // A hole with nothing left stays a hole, rather than closing over
        // nothing.
        assert_eq!(printed(r#""a={}""#), "a={}");
        assert_eq!(printed(r#""a={} b={}", 1"#), "a=1 b={}");
        assert_eq!(printed(r#""{}{}", 1, 2"#), "12");
        // Arguments past the last hole are appended, space-separated…
        assert_eq!(printed(r#""x", 1, 2"#), "x 1 2");
        // …and with nothing to separate them from, no leading space. The line
        // the three copies disagreed on.
        assert_eq!(printed(r#""", 1, 2"#), "1 2");
        // A first argument that is not a string is not a template.
        assert_eq!(printed("1, 2"), "1 2");
    }

    /// `show` decides what printing a struct says.
    ///
    /// A language rule — `impl Show for P` is in the program, not in the host —
    /// and it lived in the desktop host alone. The web and bare hosts rendered
    /// the raw struct, so the same value printed `P!` on a desktop and `P{a:1}`
    /// in the browser. This runs through the web host, which is one of the two
    /// that could not do it.
    #[test]
    fn printing_a_struct_asks_its_show_impl() {
        let source = r#"struct P { a: Int }
            trait Display { fn show(self) -> String; }
            impl Display for P { fn show(self) -> String { return "P!"; } }
            print(P { a: 1 });"#;
        let tokens = Tokenizer::tokenize(source).expect("tokenize");
        let program = StmtParser::new(&tokens).parse_program().expect("parse");
        let mut registry = ModuleRegistry::new();
        lk_stdlib_web::register_web_stdlib(&mut registry).expect("web host");
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        lk_stdlib_web::clear_stdout();
        program.execute_with_ctx(&mut env).expect("run");
        assert_eq!(lk_stdlib_web::take_stdout(), "P!");
    }
}
