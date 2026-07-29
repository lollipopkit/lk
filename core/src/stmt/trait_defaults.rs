//! Trait default methods, erased before anything downstream sees them.
//!
//! `trait Greet { fn hi(self) -> String { return "hi"; } }` gives every
//! implementor a `hi` unless it writes its own. This module is what makes that
//! true: after macros and before the type checker, each `impl Trait for Type`
//! gets a copy of the trait's bodies for the methods it left out.
//!
//! **Copied per implementing type, not shared.** Dispatch in this language is
//! indexed by the *target type* (see `vm::TypeScope`), and `self` in a default
//! body is the implementing type — so a copy is both the simplest lowering and
//! the correct one. The cost is one compiled function per implementor, which is
//! what writing the method out by hand would have cost anyway.
//!
//! Nothing downstream learns that defaults exist: the type checker, the VM
//! compiler and the AOT lowering all see an ordinary `impl` block. That is the
//! same shape `defer` uses (see [`crate::stmt::defer`]) and for the same
//! reason — a rewrite of the code's shape is easier to keep correct than a
//! second dispatch rule.

#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;

use crate::compat::collections::HashMap;
use crate::stmt::Stmt;

/// Fills every `impl Trait for Type` with the trait's default bodies for the
/// methods it did not write.
///
/// A trait declared *after* the impl that uses it still applies: the traits are
/// collected in one pass over the whole program first. Declaration order is a
/// property of the file, not of the language.
pub fn apply_trait_defaults(statements: &mut [Box<Stmt>]) {
    let defaults = collect_defaults(statements);
    if defaults.is_empty() {
        return;
    }
    for stmt in statements.iter_mut() {
        fill_impl(stmt, &defaults);
    }
}

fn collect_defaults(statements: &[Box<Stmt>]) -> HashMap<String, Vec<Stmt>> {
    let mut out: HashMap<String, Vec<Stmt>> = HashMap::new();
    for stmt in statements {
        let item = match stmt.as_ref() {
            Stmt::Attributed { item, .. } => item.as_ref(),
            other => other,
        };
        if let Stmt::Trait {
            name, default_methods, ..
        } = item
            && !default_methods.is_empty()
        {
            out.insert(name.clone(), default_methods.clone());
        }
    }
    out
}

fn fill_impl(stmt: &mut Stmt, defaults: &HashMap<String, Vec<Stmt>>) {
    if let Stmt::Attributed { item, .. } = stmt {
        fill_impl(item, defaults);
        return;
    }
    let Stmt::Impl {
        trait_name: Some(trait_name),
        methods,
        ..
    } = stmt
    else {
        return;
    };
    let Some(trait_defaults) = defaults.get(trait_name.as_str()) else {
        return;
    };
    for default in trait_defaults {
        let Some(name) = method_name(default) else {
            continue;
        };
        if methods.iter().any(|m| method_name(m) == Some(name)) {
            continue;
        }
        methods.push(default.clone());
    }
}

fn method_name(stmt: &Stmt) -> Option<&str> {
    match stmt {
        Stmt::Attributed { item, .. } => method_name(item),
        Stmt::Function { name, .. } => Some(name.as_str()),
        _ => None,
    }
}
