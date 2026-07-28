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
