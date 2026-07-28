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
    let Some(line) = manifest.lines().find(|line| line.trim_start().starts_with("lk-stdlib-bare")) else {
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
