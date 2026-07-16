use super::*;

pub(super) fn format_runtime_val(value: &RuntimeVal, heap: &HeapStore, depth: usize) -> String {
    const MAX_DEPTH: usize = 8;
    match value {
        RuntimeVal::Nil => "nil".to_string(),
        RuntimeVal::Bool(b) => b.to_string(),
        RuntimeVal::Int(i) => i.to_string(),
        RuntimeVal::Float(f) => f.to_string(),
        RuntimeVal::ShortStr(s) => s.as_str().to_string(),
        RuntimeVal::Obj(handle) => {
            let Some(heap_val) = heap.get(*handle) else {
                return "<invalid ref>".to_string();
            };
            match heap_val {
                HeapValue::String(s) => s.to_string(),
                HeapValue::List(list) if depth < MAX_DEPTH => format_typed_list(list, heap, depth + 1),
                HeapValue::List(_) => "[...]".to_string(),
                HeapValue::Map(map) if depth < MAX_DEPTH => format_typed_map(map, heap, depth + 1),
                HeapValue::Map(_) => "{...}".to_string(),
                HeapValue::Set(set) if depth < MAX_DEPTH => format_runtime_set(set),
                HeapValue::Set(_) => "Set([...])".to_string(),
                HeapValue::Callable(callable) => format_callable(callable),
                HeapValue::Object(obj) => {
                    if depth < MAX_DEPTH {
                        let mut out = String::new();
                        out.push('<');
                        out.push_str(&obj.type_name);
                        out.push_str(" {");
                        let mut first = true;
                        for (key, value) in &obj.fields {
                            if !first {
                                out.push_str(", ");
                            }
                            first = false;
                            out.push_str(key);
                            out.push_str(": ");
                            out.push_str(&format_runtime_val(value, heap, depth + 1));
                        }
                        out.push_str("}>");
                        out
                    } else {
                        format!("<{} {{...}}>", obj.type_name)
                    }
                }
                _ => "<value>".to_string(),
            }
        }
    }
}

pub(super) fn format_callable(callable: &crate::val::CallableValue) -> String {
    match callable {
        crate::val::CallableValue::Closure {
            function_index,
            captures,
        } => format!("<fn #{}({} captures)>", function_index, captures.len()),
        crate::val::CallableValue::RuntimeNative { name, arity, .. } => {
            if *arity == NativeEntry::VARIADIC {
                format!("<native fn {}(...)>", name)
            } else {
                format!("<native fn {}({} args)>", name, arity)
            }
        }
        crate::val::CallableValue::Runtime(function) => {
            format!(
                "<fn {} ({} captures)>",
                function.display_signature(),
                function.capture_count()
            )
        }
    }
}

pub(super) fn format_typed_list(list: &TypedList, heap: &HeapStore, depth: usize) -> String {
    let mut out = String::new();
    out.push('[');
    match list {
        TypedList::Int(values) => append_display_items(&mut out, values.iter().copied()),
        TypedList::Float(values) => append_display_items(&mut out, values.iter().copied()),
        TypedList::Bool(values) => append_display_items(&mut out, values.iter().copied()),
        TypedList::String(values) => append_display_items(&mut out, values.iter().map(|value| value.as_ref())),
        TypedList::Mixed(values) => append_runtime_items(&mut out, values, heap, depth),
    }
    out.push(']');
    out
}

pub(super) fn format_typed_map(map: &TypedMap, heap: &HeapStore, depth: usize) -> String {
    let mut out = String::new();
    out.push('{');
    match map {
        TypedMap::Mixed(entries) => {
            let mut first = true;
            for (key, value) in entries {
                append_separator(&mut out, &mut first);
                out.push_str(&format_map_key(key));
                out.push_str(": ");
                out.push_str(&format_runtime_val(value, heap, depth));
            }
        }
        TypedMap::StringMixed(entries) => append_string_runtime_map_entries(&mut out, entries, heap, depth),
        TypedMap::StringInt(entries) => append_string_display_map_entries(&mut out, entries),
        TypedMap::StringFloat(entries) => append_string_display_map_entries(&mut out, entries),
        TypedMap::StringBool(entries) => append_string_display_map_entries(&mut out, entries),
    }
    out.push('}');
    out
}

pub(super) fn format_runtime_set(set: &RuntimeSet) -> String {
    let mut out = String::from("Set([");
    let mut first = true;
    for value in set.entries() {
        append_separator(&mut out, &mut first);
        out.push_str(&format_map_key(value));
    }
    out.push_str("])");
    out
}

pub(super) fn append_separator(out: &mut String, first: &mut bool) {
    if !*first {
        out.push_str(", ");
    }
    *first = false;
}

pub(super) fn append_display_items<T: core::fmt::Display>(out: &mut String, values: impl IntoIterator<Item = T>) {
    let mut first = true;
    for value in values {
        append_separator(out, &mut first);
        out.push_str(&value.to_string());
    }
}

pub(super) fn append_runtime_items(out: &mut String, values: &[RuntimeVal], heap: &HeapStore, depth: usize) {
    let mut first = true;
    for value in values {
        append_separator(out, &mut first);
        out.push_str(&format_runtime_val(value, heap, depth));
    }
}

pub(super) fn append_string_runtime_map_entries(
    out: &mut String,
    entries: &FastHashMap<Arc<str>, RuntimeVal>,
    heap: &HeapStore,
    depth: usize,
) {
    let mut first = true;
    for (key, value) in entries {
        append_separator(out, &mut first);
        out.push_str(key);
        out.push_str(": ");
        out.push_str(&format_runtime_val(value, heap, depth));
    }
}

pub(super) fn append_string_display_map_entries<T: core::fmt::Display>(
    out: &mut String,
    entries: &FastHashMap<Arc<str>, T>,
) {
    let mut first = true;
    for (key, value) in entries {
        append_separator(out, &mut first);
        out.push_str(key);
        out.push_str(": ");
        out.push_str(&value.to_string());
    }
}

pub(super) fn format_map_key(key: &RuntimeMapKey) -> String {
    match key {
        RuntimeMapKey::Nil => "nil".to_string(),
        RuntimeMapKey::Bool(b) => b.to_string(),
        RuntimeMapKey::Int(i) => i.to_string(),
        RuntimeMapKey::ShortStr(s) => s.as_str().to_string(),
        RuntimeMapKey::String(s) => s.to_string(),
        RuntimeMapKey::Obj(h) => format!("<obj:{}>", h.index()),
    }
}
