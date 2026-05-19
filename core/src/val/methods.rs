use std::collections::HashMap;

use dashmap::DashMap;

use once_cell::sync::Lazy;

use crate::val::{RustFastFunction, RustFunction, Val};

// Global registry: type_name -> method_name -> callable method value.
static METHOD_REGISTRY: Lazy<DashMap<String, HashMap<String, Val>>> = Lazy::new(DashMap::new);

/// Register a method for a type name
pub fn register_method(type_name: &str, method: &str, func: RustFunction) {
    METHOD_REGISTRY
        .entry(type_name.to_string())
        .or_default()
        .insert(method.to_string(), Val::RustFunction(func));
}

/// Register a native fastcall method for a type name.
pub fn register_fast_method(type_name: &str, method: &str, func: RustFastFunction) {
    METHOD_REGISTRY
        .entry(type_name.to_string())
        .or_default()
        .insert(method.to_string(), Val::RustFastFunction(func));
}

// Per-thread inline cache for method lookups.
// Avoids DashMap overhead on monomorphic call sites (same type + method name every time).
// Uses a simple single-entry cache per thread — this is highly effective because
// in tight loops, the same type+method pattern repeats 100% of the time.
thread_local! {
    static METHOD_IC: std::cell::RefCell<(u8, usize, Option<Val>)> =
        std::cell::RefCell::new((0, 0, None));
}

/// Find a method function for a given receiver value and method name.
/// Uses a per-thread inline cache to avoid DashMap lookup on monomorphic sites.
#[inline]
pub fn find_method_for_val(receiver: &Val, method: &str) -> Option<Val> {
    let disc = match receiver {
        Val::ShortStr(_) | Val::Str(_) => 0u8,
        Val::Int(_) => 1,
        Val::Float(_) => 2,
        Val::Bool(_) => 3,
        Val::List(_) => 4,
        Val::Map(_) => 5,
        Val::Closure(_)
        | Val::RustFunction(_)
        | Val::RustFastFunction(_)
        | Val::RustFastFunctionNamed(_)
        | Val::RustFunctionNamed(_)
        | Val::AotFunction(_) => 6,
        Val::Task(_) => 7,
        Val::Channel(_) => 8,
        Val::Stream(_) => 9,
        Val::Iterator(_) => 10,
        Val::MutationGuard(_) => 11,
        Val::StreamCursor(_) => 12,
        Val::Object(_) => 13,
        Val::Nil => 14,
    };
    let method_ptr = method.as_ptr() as usize;

    // Fast path: check thread-local inline cache
    let hit = METHOD_IC.with(|ic| {
        let ic = ic.borrow();
        ic.0 == disc && ic.1 == method_ptr && ic.2.is_some()
    });
    if hit {
        return METHOD_IC.with(|ic| ic.borrow().2.clone());
    }

    // Slow path: full registry lookup + cache update
    let tname: &str = match receiver {
        Val::ShortStr(_) | Val::Str(_) => "String",
        Val::Int(_) => "Int",
        Val::Float(_) => "Float",
        Val::Bool(_) => "Bool",
        Val::List(_) => "List",
        Val::Map(_) => "Map",
        Val::Closure(_)
        | Val::RustFunction(_)
        | Val::RustFastFunction(_)
        | Val::RustFastFunctionNamed(_)
        | Val::RustFunctionNamed(_)
        | Val::AotFunction(_) => "Function",
        Val::Task(_) => "Task",
        Val::Channel(_) => "Channel",
        Val::Stream(_) => "Stream",
        Val::Iterator(_) => "Iterator",
        Val::MutationGuard(guard) => guard.guard_type(),
        Val::StreamCursor(_) => "StreamCursor",
        Val::Object(object) => object.type_name.as_ref(),
        Val::Nil => "Nil",
    };
    let result = METHOD_REGISTRY
        .get(tname)
        .and_then(|methods| methods.get(method).cloned());

    if let Some(func) = result.clone() {
        METHOD_IC.with(|ic| {
            *ic.borrow_mut() = (disc, method_ptr, Some(func));
        });
    }

    result
}
