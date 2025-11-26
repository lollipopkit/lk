use std::collections::HashMap;

use dashmap::DashMap;

use once_cell::sync::Lazy;

use crate::val::{RustFunction, Val};

// Global registry: type_name -> method_name -> RustFunction
static METHOD_REGISTRY: Lazy<DashMap<String, HashMap<String, RustFunction>>> = Lazy::new(DashMap::new);

/// Register a method for a type name
pub fn register_method(type_name: &str, method: &str, func: RustFunction) {
    METHOD_REGISTRY
        .entry(type_name.to_string())
        .or_default()
        .insert(method.to_string(), func);
}

/// Find a method function for a given receiver value and method name
pub fn find_method_for_val(receiver: &Val, method: &str) -> Option<RustFunction> {
    // Avoid allocation by using static names or borrowed object name
    let tname: &str = match receiver {
        Val::Str(_) => "String",
        Val::Int(_) => "Int",
        Val::Float(_) => "Float",
        Val::Bool(_) => "Bool",
        Val::List(_) => "List",
        Val::Map(_) => "Map",
        Val::Closure(_) | Val::RustFunction(_) | Val::RustFunctionNamed(_) => "Function",
        Val::Task(_) => "Task",
        Val::Channel(_) => "Channel",
        Val::Stream(_) => "Stream",
        Val::Iterator(_) => "Iterator",
        Val::MutationGuard(guard) => guard.guard_type(),
        Val::StreamCursor { .. } => "StreamCursor",
        Val::Object(object) => object.type_name.as_ref(),
        Val::Nil => "Nil",
    };
    METHOD_REGISTRY
        .get(tname)
        .and_then(|methods| methods.get(method).copied())
}
