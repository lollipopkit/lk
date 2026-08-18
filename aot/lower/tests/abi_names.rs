//! Every ABI name the lowering can emit has a row in the schema.
//!
//! A `Call` naming a function the schema does not have fails MIR validation —
//! which rejects the *whole module*, not the one call. That is a clean
//! fallback, and it is also invisible: the program simply does not lower, with
//! no sign that a lowering arm is at fault rather than the program. One such
//! arm (`dyn.map_get`, the callable-property path for a boxed receiver) was
//! written, shipped, and never ran once.
//!
//! So the names are checked here, against the source. A literal in the second
//! position of `AbiRef::new` has to exist in the schema; a literal in the first
//! has to be a module the schema knows. Both positions are read regardless of
//! the expression shape around them, because the arm that got this wrong wrote
//! `AbiRef::new(if … { "dyn" } else { "map_h" }, if … { "map_get" } else { … })`
//! — a form that reads a name out of a conditional.

use std::collections::HashSet;

/// The lowering's sources, by the path `include_str!` resolves from this file.
const SOURCES: &[(&str, &str)] = &[
    ("lib.rs", include_str!("../src/lib.rs")),
    ("function.rs", include_str!("../src/function.rs")),
    ("lower_call.rs", include_str!("../src/lower_call.rs")),
    ("lower_method.rs", include_str!("../src/lower_method.rs")),
    ("lower_module.rs", include_str!("../src/lower_module.rs")),
    ("lower_builtin.rs", include_str!("../src/lower_builtin.rs")),
    ("convert.rs", include_str!("../src/convert.rs")),
    ("dyn_box.rs", include_str!("../src/dyn_box.rs")),
    ("capture.rs", include_str!("../src/capture.rs")),
    ("try_region.rs", include_str!("../src/try_region.rs")),
    ("inst/container.rs", include_str!("../src/inst/container.rs")),
    ("inst/call.rs", include_str!("../src/inst/call.rs")),
    ("inst/global.rs", include_str!("../src/inst/global.rs")),
    ("inst/scalar.rs", include_str!("../src/inst/scalar.rs")),
    ("inst/string.rs", include_str!("../src/inst/string.rs")),
    ("inst/control.rs", include_str!("../src/inst/control.rs")),
];

/// Every string literal inside each `AbiRef::new( … )` call in `source` that is
/// used as a *name* rather than compared against one.
///
/// A name may be chosen by a conditional — `AbiRef::new("dyn", if name ==
/// "keys" { "map_keys" } else { "map_values" })` — so the literals inside the
/// call are a mix of names and of the thing being tested. An operand of `==`
/// or `!=` is the latter.
fn abi_ref_literals(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = source;
    while let Some(at) = rest.find("AbiRef::new(") {
        let after = &rest[at + "AbiRef::new(".len()..];
        let mut depth = 1usize;
        let mut end = after.len();
        for (index, ch) in after.char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = index;
                        break;
                    }
                }
                _ => {}
            }
        }
        let args = &after[..end];
        let mut chars = args.char_indices().peekable();
        while let Some((index, ch)) = chars.next() {
            if ch != '"' {
                continue;
            }
            let start = index + 1;
            let mut close = start;
            for (at, ch) in args[start..].char_indices() {
                if ch == '"' {
                    close = start + at;
                    break;
                }
            }
            let before = args[..index].trim_end();
            let compared = before.ends_with("==") || before.ends_with("!=");
            if !compared {
                found.push(args[start..close].to_string());
            }
            while let Some(&(at, _)) = chars.peek() {
                if at <= close {
                    chars.next();
                } else {
                    break;
                }
            }
        }
        rest = &after[end..];
    }
    found
}

#[test]
fn every_emitted_abi_name_exists_in_the_schema() {
    let schema = lk_aot_abi::ABI_FUNCTIONS;
    let modules: HashSet<&str> = schema.iter().map(|row| row.module).collect();
    let names: HashSet<&str> = schema.iter().map(|row| row.name).collect();

    let mut unknown: Vec<String> = Vec::new();
    for (file, source) in SOURCES {
        for literal in abi_ref_literals(source) {
            if modules.contains(literal.as_str()) || names.contains(literal.as_str()) {
                continue;
            }
            unknown.push(format!("{file}: \"{literal}\""));
        }
    }
    assert!(
        unknown.is_empty(),
        "these `AbiRef::new` literals name neither an ABI module nor an ABI function, \
         so any program reaching them fails MIR validation and falls back whole:\n  {}",
        unknown.join("\n  ")
    );
}
