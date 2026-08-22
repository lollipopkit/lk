//! Every stdlib module reaches every platform — provided, or explicitly absent.
//!
//! A platform surface (`stdlib/web`, `stdlib/bare`) populates its own
//! `ModuleRegistry` instead of going through a HAL trait, which is the
//! documented design. The cost of that freedom is a list: `web` names the
//! modules it cannot support so `use fs` there says "not supported on this
//! platform" rather than "unknown module".
//!
//! A list is exactly the thing that goes stale. Add a module to the umbrella
//! and neither platform notices — the module simply is not there, and the
//! diagnostic a user gets is about a *name*, not about a platform. Nothing
//! failed when that happened, which is why this asserts it instead.

use lk_core::module::ModuleRegistry;

/// The umbrella's module names — the definition of "every stdlib module".
fn all_module_names() -> Vec<String> {
    crate::stdlib_catalog()
        .modules
        .iter()
        .map(|module| module.name.to_string())
        .collect()
}

fn registered_names(register: fn(&mut ModuleRegistry) -> anyhow::Result<()>) -> Vec<String> {
    let mut registry = ModuleRegistry::new();
    register(&mut registry).expect("platform registration");
    registry.get_module_names()
}

/// `web` either provides a module or registers the placeholder that explains
/// itself. Missing from both is the failure this catches.
#[test]
fn the_web_surface_accounts_for_every_stdlib_module() {
    let available = registered_names(lk_stdlib_web::register_web_stdlib_modules);
    let missing: Vec<String> = all_module_names()
        .into_iter()
        .filter(|name| !available.contains(name))
        .collect();
    assert!(
        missing.is_empty(),
        "these stdlib modules are neither provided nor listed unsupported on web, so `use <name>` \
         there reports an unknown module rather than an unsupported platform: {missing:?}"
    );
}

/// Bare metal is the opposite shape — a *subset* by design, since flash is the
/// scarce resource and every module is opt-in. So this asserts the weaker but
/// still load-bearing thing: what it does register is a subset of the real
/// stdlib, i.e. no platform surface invents a module name that the language
/// does not have.
#[test]
fn no_platform_surface_invents_a_module_name() {
    let all = all_module_names();
    for (platform, names) in [
        ("web", registered_names(lk_stdlib_web::register_web_stdlib_modules)),
        ("bare", registered_names(lk_stdlib_bare::register_bare_stdlib_modules)),
    ] {
        for name in &names {
            assert!(
                all.contains(name),
                "{platform} registers `{name}`, which is not a stdlib module — a platform surface \
                 may omit modules, never add ones the language does not define"
            );
        }
    }
}
