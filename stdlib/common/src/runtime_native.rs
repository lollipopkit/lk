use alloc::sync::Arc;
use anyhow::{Result, anyhow, bail};
// From `alloc` directly, not `lk_core::compat::prelude`: feature
// unification can give lk-core `std` while this crate stays no_std, and
// then that prelude does not exist.
#[cfg(not(feature = "std"))]
#[allow(unused_imports)]
use alloc::{
    borrow::ToOwned,
    boxed::Box,
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use lk_core::{
    module::{RuntimeNativeExport, RuntimeValueExport},
    util::fast_map::fast_hash_map_new,
    val::{
        CallableValue, HeapStore, HeapValue, RuntimeMapKey, RuntimeSet, RuntimeVal, ShortStr, TypedList, TypedMap, de,
    },
    vm::{NativeArgs, NativeRuntime, RuntimeExport, import_runtime_export},
};

pub fn runtime_native_export(
    module: &dyn lk_core::module::ModuleProvider,
    name: &str,
) -> Result<(u16, lk_core::vm::NativeFunction)> {
    let export = module.runtime_exports()?;
    let state = export.state_lock()?;
    let RuntimeVal::Obj(handle) = export.value() else {
        return Err(anyhow!("module export must be a map"));
    };
    let Some(HeapValue::Map(map)) = state.heap().get(*handle) else {
        return Err(anyhow!("module export must be a map"));
    };
    let value = map.get_str(name).ok_or_else(|| anyhow!("{name} export present"))?;
    let RuntimeVal::Obj(handle) = value else {
        return Err(anyhow!("{name} must be a heap callable"));
    };
    let Some(HeapValue::Callable(lk_core::val::CallableValue::RuntimeNative { arity, function, .. })) =
        state.heap().get(handle)
    else {
        return Err(anyhow!("{name} must be RuntimeNative"));
    };
    Ok((*arity, function.clone()))
}

pub fn module_export(
    natives: &[RuntimeNativeExport],
    values: &[RuntimeValueExport],
    namespaces: &[(&'static str, RuntimeExport)],
) -> Result<RuntimeExport> {
    let mut heap = HeapStore::new();
    let mut map = fast_hash_map_new();
    for native in natives {
        let value = RuntimeVal::Obj(heap.alloc(HeapValue::Callable(CallableValue::RuntimeNative {
            name: Arc::<str>::from(native.name),
            arity: native.arity,
            function: native.function.clone(),
        })));
        map.insert(Arc::<str>::from(native.name), value);
    }
    for value in values {
        map.insert(Arc::<str>::from(value.name), value.value);
    }
    for (name, export) in namespaces {
        map.insert(Arc::<str>::from(*name), import_runtime_export(export, &mut heap)?);
    }
    let value = RuntimeVal::Obj(heap.alloc(HeapValue::Map(TypedMap::StringMixed(map))));
    Ok(RuntimeExport::from_value(value, heap))
}

pub fn expect_arity(args: NativeArgs<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        Ok(())
    } else {
        bail!(
            "{name} expects exactly {expected} argument{}",
            if expected == 1 { "" } else { "s" }
        )
    }
}

pub fn parse_format(
    args: NativeArgs<'_>,
    runtime: &mut NativeRuntime<'_>,
    name: &str,
    format: de::Format,
) -> Result<RuntimeVal> {
    if args.len() != 1 {
        return Err(anyhow!("{name}(data) requires 1 argument"));
    }
    let data = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), name)?;
    parse_runtime_with_format(data.as_ref(), format, runtime.heap_mut())
}

fn parse_runtime_with_format(input: &str, format: de::Format, heap: &mut HeapStore) -> Result<RuntimeVal> {
    de::parse_runtime_with_format_into_heap(input, format, heap)
}

pub fn runtime_string_arg(value: &RuntimeVal, heap: &HeapStore, name: &str) -> Result<Arc<str>> {
    match value {
        RuntimeVal::ShortStr(value) => Ok(Arc::<str>::from(value.as_str())),
        RuntimeVal::Obj(handle) => match heap
            .get(*handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::String(value) => Ok(value.clone()),
            other => Err(anyhow!("{name} expects string argument, got {}", other.type_name())),
        },
        other => Err(anyhow!("{name} expects string argument, got {:?}", other.kind())),
    }
}

pub fn runtime_string_value(value: &str, heap: &mut HeapStore) -> RuntimeVal {
    if let Some(value) = ShortStr::new(value) {
        RuntimeVal::ShortStr(value)
    } else {
        RuntimeVal::Obj(heap.alloc(HeapValue::String(Arc::<str>::from(value))))
    }
}

/// A window prints as the part of the list it windows — `[1,4,1]`, not
/// `<Slice>`. It used to fall through to the opaque-handle arm, which is the
/// right answer for a `Stream` or a `Resource` and the wrong one here: a window
/// has elements, and every other way of looking at it (`len`, indexing,
/// `to_list`) already shows them.

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;
    use lk_core::util::fast_map::fast_hash_map_from_iter;

    use super::*;
    use lk_core::val::TypedMap;

    #[test]
    fn runtime_display_formats_typed_containers_without_val_containers() {
        let mut heap = HeapStore::new();
        let nested = RuntimeVal::Obj(heap.alloc(HeapValue::List(TypedList::Int(vec![1, 2]))));
        let map = RuntimeVal::Obj(
            heap.alloc(HeapValue::Map(TypedMap::StringMixed(fast_hash_map_from_iter([
                (Arc::<str>::from("items"), nested),
                (Arc::<str>::from("ok"), RuntimeVal::Bool(true)),
            ])))),
        );

        let output = runtime_display_value(&map, &heap).expect("display");

        assert!(output.contains("\"items\":[1,2]"));
        assert!(output.contains("\"ok\":true"));
    }
}

/// Value equality, shared by everything that needs it.
///
/// There were four copies of this question in the tree. The one in
/// `stdlib/web` — which backs `assert_eq` in the browser playground — was
/// `left == right`, the *derived* `PartialEq` on `RuntimeVal`. That derive
/// means two different things for the two variants: structural for a
/// `ShortStr`, and **handle identity** for an `Obj`. So in the playground
///
/// ```text
/// assert_eq("ab", "ab")                  passed
/// assert_eq("abcdefghij", "abcdefghij")  failed
/// ```
///
/// — the seven-byte inline limit of `ShortStr` deciding whether an assertion
/// held. `ShortStr` is not the bug: it is a small-string optimisation that made
/// half the cases accidentally right. Take it away and the derive is uniformly
/// wrong instead of intermittently.
pub fn runtime_values_equal(left: &RuntimeVal, right: &RuntimeVal, heap: &HeapStore) -> Result<bool> {
    Ok(match (left, right) {
        (RuntimeVal::Nil, RuntimeVal::Nil) => true,
        (RuntimeVal::Bool(left), RuntimeVal::Bool(right)) => left == right,
        (RuntimeVal::Int(left), RuntimeVal::Int(right)) => left == right,
        (RuntimeVal::Float(left), RuntimeVal::Float(right)) => left == right,
        (RuntimeVal::Int(left), RuntimeVal::Float(right)) => *left as f64 == *right,
        (RuntimeVal::Float(left), RuntimeVal::Int(right)) => *left == *right as f64,
        (RuntimeVal::Obj(left), RuntimeVal::Obj(right)) if left == right => true,
        (RuntimeVal::Obj(left), RuntimeVal::Obj(right)) => {
            let left = heap
                .get(*left)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", left.index()))?;
            let right = heap
                .get(*right)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", right.index()))?;
            heap_values_equal(left, right, heap)?
        }
        _ => match (
            runtime_value_to_string(left, heap)?,
            runtime_value_to_string(right, heap)?,
        ) {
            (Some(left), Some(right)) => left == right,
            _ => false,
        },
    })
}
fn heap_values_equal(left: &HeapValue, right: &HeapValue, heap: &HeapStore) -> Result<bool> {
    Ok(match (left, right) {
        (HeapValue::String(left), HeapValue::String(right)) => left == right,
        (HeapValue::List(left), HeapValue::List(right)) => typed_lists_equal(left, right, heap)?,
        (HeapValue::Map(left), HeapValue::Map(right)) => typed_maps_equal(left, right, heap)?,
        (HeapValue::Set(left), HeapValue::Set(right)) => runtime_sets_equal(left, right),
        _ => false,
    })
}
fn runtime_sets_equal(left: &RuntimeSet, right: &RuntimeSet) -> bool {
    left.len() == right.len() && left.entries().all(|key| right.contains(key))
}
fn typed_lists_equal(left: &TypedList, right: &TypedList, heap: &HeapStore) -> Result<bool> {
    if left.len() != right.len() {
        return Ok(false);
    }
    match (left, right) {
        (TypedList::Int(left), TypedList::Int(right)) => return Ok(left == right),
        (TypedList::Float(left), TypedList::Float(right)) => return Ok(left == right),
        (TypedList::Bool(left), TypedList::Bool(right)) => return Ok(left == right),
        (TypedList::String(left), TypedList::String(right)) => return Ok(left == right),
        _ => {}
    }
    for index in 0..left.len() {
        if !typed_list_items_equal(left, index, right, index, heap)? {
            return Ok(false);
        }
    }
    Ok(true)
}
fn runtime_value_equals_string(value: &RuntimeVal, expected: &str, heap: &HeapStore) -> Result<bool> {
    Ok(match value {
        RuntimeVal::ShortStr(value) => value.as_str() == expected,
        RuntimeVal::Obj(handle) => matches!(
            heap.get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?,
            HeapValue::String(value) if value.as_ref() == expected
        ),
        _ => false,
    })
}
fn typed_maps_equal(left: &TypedMap, right: &TypedMap, heap: &HeapStore) -> Result<bool> {
    if left.len() != right.len() {
        return Ok(false);
    }
    match left {
        TypedMap::Mixed(entries) => {
            for (key, value) in entries {
                if !typed_map_value_equal(right, key, value, heap)? {
                    return Ok(false);
                }
            }
        }
        TypedMap::StringMixed(entries) => {
            for (key, value) in entries {
                let key = RuntimeMapKey::String(key.clone());
                if !typed_map_value_equal(right, &key, value, heap)? {
                    return Ok(false);
                }
            }
        }
        TypedMap::StringInt(entries) => {
            for (key, value) in entries {
                let key = RuntimeMapKey::String(key.clone());
                if !typed_map_value_equal(right, &key, &RuntimeVal::Int(*value), heap)? {
                    return Ok(false);
                }
            }
        }
        TypedMap::StringFloat(entries) => {
            for (key, value) in entries {
                let key = RuntimeMapKey::String(key.clone());
                if !typed_map_value_equal(right, &key, &RuntimeVal::Float(*value), heap)? {
                    return Ok(false);
                }
            }
        }
        TypedMap::StringBool(entries) => {
            for (key, value) in entries {
                let key = RuntimeMapKey::String(key.clone());
                if !typed_map_value_equal(right, &key, &RuntimeVal::Bool(*value), heap)? {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

fn runtime_value_to_string(value: &RuntimeVal, heap: &HeapStore) -> Result<Option<Arc<str>>> {
    match value {
        RuntimeVal::ShortStr(value) => Ok(Some(Arc::<str>::from(value.as_str()))),
        RuntimeVal::Obj(handle) => match heap
            .get(*handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::String(value) => Ok(Some(value.clone())),
            _ => Ok(None),
        },
        _ => Ok(None),
    }
}
fn typed_list_items_equal(
    left: &TypedList,
    left_index: usize,
    right: &TypedList,
    right_index: usize,
    heap: &HeapStore,
) -> Result<bool> {
    match (left, right) {
        (TypedList::Mixed(left), TypedList::Mixed(right)) => {
            runtime_values_equal(&left[left_index], &right[right_index], heap)
        }
        (TypedList::Mixed(left), TypedList::String(right)) => {
            runtime_value_equals_string(&left[left_index], &right[right_index], heap)
        }
        (TypedList::String(left), TypedList::Mixed(right)) => {
            runtime_value_equals_string(&right[right_index], &left[left_index], heap)
        }
        (TypedList::Int(left), _) => {
            typed_list_runtime_item_equal(RuntimeVal::Int(left[left_index]), right, right_index, heap)
        }
        (TypedList::Float(left), _) => {
            typed_list_runtime_item_equal(RuntimeVal::Float(left[left_index]), right, right_index, heap)
        }
        (TypedList::Bool(left), _) => {
            typed_list_runtime_item_equal(RuntimeVal::Bool(left[left_index]), right, right_index, heap)
        }
        (TypedList::String(left), _) => typed_list_string_item_equal(&left[left_index], right, right_index, heap),
        (TypedList::Mixed(left), _) => typed_list_runtime_item_equal(left[left_index], right, right_index, heap),
    }
}

fn typed_map_value_equal(
    right: &TypedMap,
    key: &RuntimeMapKey,
    left_value: &RuntimeVal,
    heap: &HeapStore,
) -> Result<bool> {
    let Some(right_value) = right.get(key) else {
        return Ok(false);
    };
    runtime_values_equal(left_value, &right_value, heap)
}

fn typed_list_runtime_item_equal(
    value: RuntimeVal,
    right: &TypedList,
    right_index: usize,
    heap: &HeapStore,
) -> Result<bool> {
    match right {
        TypedList::Mixed(right) => runtime_values_equal(&value, &right[right_index], heap),
        TypedList::Int(right) => runtime_values_equal(&value, &RuntimeVal::Int(right[right_index]), heap),
        TypedList::Float(right) => runtime_values_equal(&value, &RuntimeVal::Float(right[right_index]), heap),
        TypedList::Bool(right) => runtime_values_equal(&value, &RuntimeVal::Bool(right[right_index]), heap),
        TypedList::String(right) => runtime_value_equals_string(&value, &right[right_index], heap),
    }
}
fn typed_list_string_item_equal(
    left: &Arc<str>,
    right: &TypedList,
    right_index: usize,
    heap: &HeapStore,
) -> Result<bool> {
    match right {
        TypedList::Mixed(right) => runtime_value_equals_string(&right[right_index], left, heap),
        TypedList::String(right) => Ok(left == &right[right_index]),
        _ => Ok(false),
    }
}

/// How a value looks — `lk_core::vm::runtime_display_value`, which is the one
/// rendering there is.
///
/// This crate used to hold it and the VM had its own, so `println` and the REPL
/// showed the same value differently. `show` dispatch is a layer above, in
/// `language::display`: it calls user code, which a renderer cannot.
pub fn runtime_display_value(value: &RuntimeVal, heap: &HeapStore) -> Result<String> {
    lk_core::vm::runtime_display_value(value, heap)
}
