//! The catalogue and the runtime describe the same standard library.
//!
//! Two lists of members exist: `lk_stdlib::stdlib_catalog()`, which the type
//! checker, the completion engine and the LSP read, and the `ModuleRegistry`
//! the executor actually looks names up in. Nothing keeps them in step, and a
//! disagreement is silent in both directions:
//!
//! - **Catalogued, not registered.** The member type-checks and resolves to
//!   `nil` at run time, so the program dies with "nil is not a function" — a
//!   sentence naming neither the module nor the member.
//! - **Registered, not catalogued.** The member works, and the checker refuses
//!   it (`has no member`), completion never offers it, and the LSP marks it an
//!   error. A working feature nobody can find.
//!
//! A module at run time *is* a map, so the runtime list is `module.keys()` —
//! the same lookup the executor does, asked from LK.

use std::collections::BTreeSet;
use std::process::Command;

/// Every member the runtime exposes, as `module.member`.
fn runtime_members() -> BTreeSet<String> {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut roots: BTreeSet<&str> = BTreeSet::new();
    for module in &lk_stdlib::stdlib_catalog().modules {
        roots.insert(module.name.split('.').next().expect("a module name"));
    }

    let mut members = BTreeSet::new();
    for root in roots {
        let source = dir.path().join(format!("{root}.lk"));
        // Bound to a local first: `root.keys()` would look `keys` up *in* the
        // module, and the point is to read the map it is.
        std::fs::write(
            &source,
            format!(
                "use {root};\n\
                 let module = {root};\n\
                 let names = module.keys();\n\
                 let index = 0;\n\
                 while index < names.len() {{\n\
                 \x20   println(\"{root}.\" + names[index]);\n\
                 \x20   index = index + 1;\n\
                 }}\n"
            ),
        )
        .expect("write probe");

        let output = Command::new(env!("CARGO_BIN_EXE_lk"))
            .arg(source.to_str().expect("utf-8 path"))
            .output()
            .expect("run lk");
        assert!(
            output.status.success(),
            "listing `{root}`'s members failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let line = line.trim();
            if !line.is_empty() {
                members.insert(line.to_string());
            }
        }
    }
    members
}

#[test]
fn the_catalogue_and_the_runtime_list_the_same_members() {
    let catalogued: BTreeSet<String> = lk_stdlib::stdlib_catalog()
        .modules
        .iter()
        .flat_map(|module| {
            module
                .exports
                .iter()
                .map(move |export| format!("{}.{}", module.name, export.name))
        })
        .collect();
    let live = runtime_members();

    let missing_at_runtime: Vec<_> = catalogued.difference(&live).collect();
    let missing_from_catalogue: Vec<_> = live.difference(&catalogued).collect();

    assert!(
        missing_at_runtime.is_empty(),
        "catalogued but not registered — these type-check and answer nil: {missing_at_runtime:?}"
    );
    assert!(
        missing_from_catalogue.is_empty(),
        "registered but not catalogued — these work and `lk check` refuses them: {missing_from_catalogue:?}"
    );
    // A floor, so that a registry that silently stops registering anything at
    // all cannot pass by matching an empty catalogue.
    assert!(
        catalogued.len() > 200,
        "the catalogue lost members: {}",
        catalogued.len()
    );
}
