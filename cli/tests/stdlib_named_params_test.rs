//! The AOT lowering's `named(...)` lists are copies. This is what checks them.
//!
//! `aot/lower` cannot read the stdlib signature registry — it is populated at
//! run time by whoever links the standard library, and a lowering that quietly
//! stops lowering because registration has not happened yet would be worse than
//! a copy. So the copy stays, and this test, in the one crate that has both
//! sides, compares it against the declaration.

use lk_core::module::ModuleRegistry;

/// Every `named` list in the lowering table matches the stdlib export's own
/// `named(...)`, in the same order.
///
/// Order is the whole point: the list *is* the permutation from caller order
/// into frame order, so a swapped pair would compile
/// `regex.replace(p, text: t, replacement: r)` into `replace(p, r, t)` — a
/// wrong answer that still runs.
#[test]
fn lowering_named_parameter_lists_match_the_stdlib_declaration() {
    let mut registry = ModuleRegistry::new();
    lk_stdlib::register_stdlib_modules(&mut registry).expect("stdlib registers");

    let mut checked = 0;
    for (module, member, leading, named) in lk_aot_lower::named_parameter_rows() {
        let path = format!("{module}.{member}");
        let signature =
            lk_core::typ::stdlib_signature(&path).unwrap_or_else(|| panic!("{path} has no declared signature"));
        let declared: Vec<&str> = signature
            .params
            .iter()
            .filter(|param| param.named)
            .map(|param| param.name.as_str())
            .collect();
        assert_eq!(
            declared, named,
            "{path}: the lowering's named(...) copy disagrees with the stdlib declaration"
        );
        // And where the named block *starts*, which is what turns a name into a
        // frame slot. Off by one and `string.slice(s, 1, end: 3)` writes `end`
        // past the end of the frame — the mixed spelling, which the VM accepts,
        // stops lowering.
        let declared_leading = signature.params.iter().take_while(|param| !param.named).count();
        assert_eq!(
            declared_leading, leading,
            "{path}: the lowering thinks the named block starts at {leading}, the declaration says {declared_leading}"
        );
        checked += 1;
    }
    assert!(checked > 0, "no named rows were checked — the accessor lost its rows");
}
