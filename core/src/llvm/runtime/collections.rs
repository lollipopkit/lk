use std::{cell::RefCell, sync::Arc};

use super::*;

thread_local! {
    static STR_INT_KEY_CACHE: RefCell<FastHashMap<(usize, i64, i64), ArcStr>> =
        RefCell::new(fast_hash_map_new());
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_build_list(ptr: *const i64, len: i64) -> i64 {
    let len_usize = len.max(0) as usize;
    with_state(|state| {
        let elements = state.decode_values(ptr, len_usize);
        let list = Val::list(Arc::new(elements));
        state.encode_value(list)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_list_push(list: i64, value: i64) -> i64 {
    with_state(|state| {
        let item = state.decode_value(value);
        list_push_decoded(state, list, item, "lk_rt_list_push")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_list_push_int(list: i64, value: i64) -> i64 {
    with_state(|state| {
        if state.handles.get_ref(value).is_none()
            && let Val::Int(item) = encoding::decode_immediate(value)
        {
            return list_push_decoded(state, list, Val::Int(item), "lk_rt_list_push_int");
        }
        let item = state.decode_value(value);
        list_push_decoded(state, list, item, "lk_rt_list_push_int")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_list_push_str_int(list: i64, prefix: *const i8, prefix_len: i64, suffix: i64) -> i64 {
    with_state(|state| {
        let key = cached_str_int_key(state, prefix, prefix_len, suffix);
        list_push_decoded(state, list, Val::from_str(key.as_str()), "lk_rt_list_push_str_int")
    })
}

fn list_push_decoded(state: &mut RuntimeState, list: i64, item: Val, helper: &str) -> i64 {
    if let Some(items) = state.handles.get_ref(list).and_then(Val::as_list) {
        let mut updated = items.as_ref().clone();
        updated.push(item);
        if let Some(slot) = state.handles.get_mut(list) {
            *slot = Val::list(Arc::new(updated));
        }
        return list;
    }
    match state.decode_value(list) {
        value if value.as_list().is_some() => {
            let mut items = value.as_list().expect("checked list").as_ref().clone();
            items.push(item);
            state.encode_value(Val::list(Arc::new(items)))
        }
        other => {
            eprintln!("{helper}: target is not a List, got {}", other.type_name());
            encoding::NIL_VALUE
        }
    }
}

fn replace_map_handle(state: &mut RuntimeState, handle: i64, items: FastHashMap<ArcStr, Val>) -> i64 {
    if let Some(slot) = state.handles.get_mut(handle) {
        *slot = Val::map(Arc::new(items));
        handle
    } else {
        state.encode_value(Val::map(Arc::new(items)))
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_build_map(ptr: *const i64, len: i64) -> i64 {
    let len_usize = len.max(0) as usize;
    with_state(|state| {
        if len_usize == 0 {
            return state.encode_value(Val::map(Arc::new(fast_hash_map_new())));
        }
        if ptr.is_null() {
            return encoding::NIL_VALUE;
        }
        let raw = unsafe { std::slice::from_raw_parts(ptr, len_usize * 2) };
        let mut map = fast_hash_map_with_capacity(len_usize);
        for i in 0..len_usize {
            let key = state.decode_value(raw[2 * i]);
            let val = state.decode_value(raw[2 * i + 1]);
            match encode_map_key(&key) {
                Ok(k) => {
                    Val::map_insert_arcstr(&mut map, k, val);
                }
                Err(err) => {
                    eprintln!("lk_rt_build_map: {err}");
                    return encoding::NIL_VALUE;
                }
            }
        }
        state.encode_value(Val::map(Arc::new(map)))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_map_set(map: i64, key: i64, value: i64) -> i64 {
    with_state(|state| {
        let key_value = state.decode_value(key);
        let item = state.decode_value(value);
        let encoded_key = match encode_map_key(&key_value) {
            Ok(key) => key,
            Err(err) => {
                eprintln!("lk_rt_map_set: {err}");
                return encoding::NIL_VALUE;
            }
        };
        if let Some(items) = state.handles.get_ref(map).and_then(Val::as_map) {
            let mut updated = items.as_ref().clone();
            Val::map_insert_arcstr(&mut updated, encoded_key, item);
            if let Some(slot) = state.handles.get_mut(map) {
                *slot = Val::map(Arc::new(updated));
            }
            return map;
        }
        match state.decode_value(map) {
            value if value.as_map().is_some() => {
                let mut items = value.as_map().expect("checked map").as_ref().clone();
                Val::map_insert_arcstr(&mut items, encoded_key, item);
                state.encode_value(Val::map(Arc::new(items)))
            }
            other => {
                eprintln!("lk_rt_map_set: target is not a Map, got {}", other.type_name());
                encoding::NIL_VALUE
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_map_set_const_str(map: i64, key: *const i8, key_len: i64, value: i64) -> i64 {
    let key = Val::intern_str(read_str_int_prefix(key, key_len).as_ref());
    with_state(|state| {
        let item = state.decode_value(value);
        if let Some(items) = state.handles.get_ref(map).and_then(Val::as_map) {
            let mut updated = items.as_ref().clone();
            Val::map_insert_arcstr(&mut updated, key, item);
            if let Some(slot) = state.handles.get_mut(map) {
                *slot = Val::map(Arc::new(updated));
            }
            return map;
        }
        match state.decode_value(map) {
            value if value.as_map().is_some() => {
                let mut items = value.as_map().expect("checked map").as_ref().clone();
                Val::map_insert_arcstr(&mut items, key, item);
                state.encode_value(Val::map(Arc::new(items)))
            }
            other => {
                eprintln!(
                    "lk_rt_map_set_const_str: target is not a Map, got {}",
                    other.type_name()
                );
                encoding::NIL_VALUE
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_map_set_str_int(map: i64, prefix: *const i8, prefix_len: i64, suffix: i64, value: i64) -> i64 {
    with_state(|state| {
        let key = cached_str_int_key(state, prefix, prefix_len, suffix);
        let item = state.decode_value(value);
        if let Some(items) = state.handles.get_ref(map).and_then(Val::as_map) {
            let mut updated = items.as_ref().clone();
            Val::map_insert_arcstr(&mut updated, key, item);
            if let Some(slot) = state.handles.get_mut(map) {
                *slot = Val::map(Arc::new(updated));
            }
            return map;
        }
        match state.decode_value(map) {
            value if value.as_map().is_some() => {
                let mut items = value.as_map().expect("checked map").as_ref().clone();
                Val::map_insert_arcstr(&mut items, key, item);
                state.encode_value(Val::map(Arc::new(items)))
            }
            other => {
                eprintln!("lk_rt_map_set_str_int: target is not a Map, got {}", other.type_name());
                encoding::NIL_VALUE
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_str_int_key(prefix: *const i8, prefix_len: i64, suffix: i64) -> i64 {
    with_state(|state| {
        let key = cached_str_int_key(state, prefix, prefix_len, suffix);
        state.encode_value(Val::from_str(key.as_str()))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_int_decimal_len(value: i64) -> i64 {
    let len = match encoding::decode_immediate(value) {
        Val::Int(i) => i.to_string().len(),
        Val::Bool(value) => bool_to_str(value).len(),
        Val::Nil => 3,
        _ => with_state(|state| state.decode_value(value).to_string().chars().count()),
    };
    len as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_starts_with_const(value: i64, prefix: *const i8, prefix_len: i64) -> i64 {
    with_state(|state| {
        let value = state.decode_value(value);
        let prefix = read_str_int_prefix(prefix, prefix_len);
        let out = value.as_str().is_some_and(|value| value.starts_with(prefix.as_ref()));
        state.encode_value(Val::Bool(out))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_str_in_const3(
    value: i64,
    key0: *const i8,
    key0_len: i64,
    key1: *const i8,
    key1_len: i64,
    key2: *const i8,
    key2_len: i64,
    count: i64,
) -> i64 {
    with_state(|state| {
        let value = state.decode_value(value);
        let Some(value) = value.as_str() else {
            return state.encode_value(Val::Bool(false));
        };
        let keys = [
            read_str_int_prefix(key0, key0_len),
            read_str_int_prefix(key1, key1_len),
            read_str_int_prefix(key2, key2_len),
        ];
        let out = keys
            .iter()
            .take(count.clamp(0, 3) as usize)
            .any(|key| value == key.as_ref());
        state.encode_value(Val::Bool(out))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_access(base: i64, key: i64) -> i64 {
    with_state(|state| {
        if let Some(result) = access_list_by_immediate_index(&state.handles, base, key) {
            return state.encode_value(result);
        }
        if let Some(result) = access_map_by_string_handle(&state.handles, base, key) {
            return state.encode_value(result);
        }
        let base_val = state.decode_value(base);
        let key_val = state.decode_value(key);
        let result = base_val.access(&key_val).unwrap_or(Val::Nil);
        state.encode_value(result)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_access_str_int(base: i64, prefix: *const i8, prefix_len: i64, suffix: i64) -> i64 {
    with_state(|state| {
        let key = cached_str_int_key(state, prefix, prefix_len, suffix);
        if let Some(result) = state
            .handles
            .get_ref(base)
            .and_then(Val::as_map)
            .map(|map| Val::map_get_str(&map, key.as_str()).cloned().unwrap_or(Val::Nil))
        {
            return state.encode_value(result);
        }
        let base_val = state.decode_value(base);
        let result = base_val.access(&Val::from_str(key.as_str())).unwrap_or(Val::Nil);
        state.encode_value(result)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_map_get_const_str(base: i64, key: *const i8, key_len: i64) -> i64 {
    let key = read_str_int_prefix(key, key_len);
    with_state(|state| {
        let result = state
            .handles
            .get_ref(base)
            .and_then(Val::as_map)
            .map(|map| Val::map_get_str(&map, key.as_ref()).cloned().unwrap_or(Val::Nil));
        state.encode_value(result.unwrap_or(Val::Nil))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_map_get_str_int(base: i64, prefix: *const i8, prefix_len: i64, suffix: i64) -> i64 {
    with_state(|state| {
        let key = cached_str_int_key(state, prefix, prefix_len, suffix);
        let result = state
            .handles
            .get_ref(base)
            .and_then(Val::as_map)
            .map(|map| Val::map_get_str(&map, key.as_str()).cloned().unwrap_or(Val::Nil));
        state.encode_value(result.unwrap_or(Val::Nil))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_map_has(base: i64, key: i64) -> i64 {
    with_state(|state| {
        let out = state.handles.get_ref(base).and_then(Val::as_map).is_some_and(|map| {
            state
                .handles
                .get_ref(key)
                .and_then(Val::as_str)
                .is_some_and(|key| Val::map_contains_str(&map, key))
        });
        state.encode_value(Val::Bool(out))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_map_has_const_str(base: i64, key: *const i8, key_len: i64) -> i64 {
    let key = read_str_int_prefix(key, key_len);
    with_state(|state| {
        let out = state
            .handles
            .get_ref(base)
            .and_then(Val::as_map)
            .is_some_and(|map| Val::map_contains_str(&map, key.as_ref()));
        state.encode_value(Val::Bool(out))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_map_has_str_int(base: i64, prefix: *const i8, prefix_len: i64, suffix: i64) -> i64 {
    with_state(|state| {
        let key = cached_str_int_key(state, prefix, prefix_len, suffix);
        let out = state
            .handles
            .get_ref(base)
            .and_then(Val::as_map)
            .is_some_and(|map| Val::map_contains_str(&map, key.as_str()));
        state.encode_value(Val::Bool(out))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_add_access(lhs: i64, base: i64, key: i64) -> i64 {
    lk_rt_binop_access(lhs, base, key, BinOp::Add)
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_mul_access(lhs: i64, base: i64, key: i64) -> i64 {
    lk_rt_binop_access(lhs, base, key, BinOp::Mul)
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_add_map_get_const_str(lhs: i64, base: i64, key: *const i8, key_len: i64) -> i64 {
    let key = read_str_int_prefix(key, key_len);
    with_state(|state| {
        if let Val::Int(left) = encoding::decode_immediate(lhs)
            && let Some(map) = state.handles.get_ref(base).and_then(Val::as_map)
            && let Some(Val::Int(right)) = Val::map_get_str(&map, key.as_ref())
        {
            return state.encode_value(Val::Int(left + right));
        }
        let left = state.decode_value(lhs);
        let right = state
            .handles
            .get_ref(base)
            .and_then(Val::as_map)
            .map(|map| Val::map_get_str(&map, key.as_ref()).cloned().unwrap_or(Val::Nil))
            .unwrap_or(Val::Nil);
        match BinOp::Add.eval_vals(&left, &right) {
            Ok(value) => state.encode_value(value),
            Err(err) => {
                eprintln!("lk_rt_add_map_get_const_str error: {err}");
                encoding::NIL_VALUE
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_add_map_get_str_int(
    lhs: i64,
    base: i64,
    prefix: *const i8,
    prefix_len: i64,
    suffix: i64,
) -> i64 {
    with_state(|state| {
        let key = cached_str_int_key(state, prefix, prefix_len, suffix);
        if let Val::Int(left) = encoding::decode_immediate(lhs)
            && let Some(map) = state.handles.get_ref(base).and_then(Val::as_map)
            && let Some(Val::Int(right)) = Val::map_get_str(&map, key.as_str())
        {
            return state.encode_value(Val::Int(left + right));
        }
        let left = state.decode_value(lhs);
        let right = state
            .handles
            .get_ref(base)
            .and_then(Val::as_map)
            .map(|map| Val::map_get_str(&map, key.as_str()).cloned().unwrap_or(Val::Nil))
            .unwrap_or(Val::Nil);
        match BinOp::Add.eval_vals(&left, &right) {
            Ok(value) => state.encode_value(value),
            Err(err) => {
                eprintln!("lk_rt_add_map_get_str_int error: {err}");
                encoding::NIL_VALUE
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_mul_map_get_const_str(lhs: i64, base: i64, key: *const i8, key_len: i64) -> i64 {
    let key = read_str_int_prefix(key, key_len);
    with_state(|state| {
        if let Val::Int(left) = encoding::decode_immediate(lhs)
            && let Some(map) = state.handles.get_ref(base).and_then(Val::as_map)
            && let Some(Val::Int(right)) = Val::map_get_str(&map, key.as_ref())
        {
            return state.encode_value(Val::Int(left * right));
        }
        let left = state.decode_value(lhs);
        let right = state
            .handles
            .get_ref(base)
            .and_then(Val::as_map)
            .map(|map| Val::map_get_str(&map, key.as_ref()).cloned().unwrap_or(Val::Nil))
            .unwrap_or(Val::Nil);
        match BinOp::Mul.eval_vals(&left, &right) {
            Ok(value) => state.encode_value(value),
            Err(err) => {
                eprintln!("lk_rt_mul_map_get_const_str error: {err}");
                encoding::NIL_VALUE
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_mul_map_get_str_int(
    lhs: i64,
    base: i64,
    prefix: *const i8,
    prefix_len: i64,
    suffix: i64,
) -> i64 {
    with_state(|state| {
        let key = cached_str_int_key(state, prefix, prefix_len, suffix);
        if let Val::Int(left) = encoding::decode_immediate(lhs)
            && let Some(map) = state.handles.get_ref(base).and_then(Val::as_map)
            && let Some(Val::Int(right)) = Val::map_get_str(&map, key.as_str())
        {
            return state.encode_value(Val::Int(left * right));
        }
        let left = state.decode_value(lhs);
        let right = state
            .handles
            .get_ref(base)
            .and_then(Val::as_map)
            .map(|map| Val::map_get_str(&map, key.as_str()).cloned().unwrap_or(Val::Nil))
            .unwrap_or(Val::Nil);
        match BinOp::Mul.eval_vals(&left, &right) {
            Ok(value) => state.encode_value(value),
            Err(err) => {
                eprintln!("lk_rt_mul_map_get_str_int error: {err}");
                encoding::NIL_VALUE
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_map_set_add_map_get_const_str(map: i64, key: *const i8, key_len: i64, lhs: i64) -> i64 {
    let lookup_key = read_str_int_prefix(key, key_len);
    let encoded_key = Val::intern_str(lookup_key.as_ref());
    with_state(|state| {
        let lhs_int = match encoding::decode_immediate(lhs) {
            Val::Int(value) => Some(value),
            _ => None,
        };
        let left = lhs_int.map(Val::Int).unwrap_or_else(|| state.decode_value(lhs));
        if let Some(items) = state.handles.get_ref(map).and_then(Val::as_map) {
            let mut items = items.as_ref().clone();
            if let Some(left) = lhs_int
                && let Some(right) = Val::map_get_str(&items, lookup_key.as_ref()).and_then(|value| match value {
                    Val::Int(value) => Some(*value),
                    _ => None,
                })
            {
                Val::map_insert_arcstr(&mut items, encoded_key, Val::Int(left + right));
                return replace_map_handle(state, map, items);
            }
            let right = Val::map_get_str(&items, lookup_key.as_ref())
                .cloned()
                .unwrap_or(Val::Nil);
            let value = match BinOp::Add.eval_vals(&left, &right) {
                Ok(value) => value,
                Err(err) => {
                    eprintln!("lk_rt_map_set_add_map_get_const_str error: {err}");
                    return encoding::NIL_VALUE;
                }
            };
            Val::map_insert_arcstr(&mut items, encoded_key, value);
            return replace_map_handle(state, map, items);
        }
        match state.decode_value(map) {
            value if value.as_map().is_some() => {
                let mut items = value.as_map().expect("checked map").as_ref().clone();
                if let Some(left) = lhs_int
                    && let Some(right) = Val::map_get_str(&items, lookup_key.as_ref()).and_then(|value| match value {
                        Val::Int(value) => Some(*value),
                        _ => None,
                    })
                {
                    Val::map_insert_arcstr(&mut items, encoded_key, Val::Int(left + right));
                    return state.encode_value(Val::map(Arc::new(items)));
                }
                let right = Val::map_get_str(&items, lookup_key.as_ref())
                    .cloned()
                    .unwrap_or(Val::Nil);
                let value = match BinOp::Add.eval_vals(&left, &right) {
                    Ok(value) => value,
                    Err(err) => {
                        eprintln!("lk_rt_map_set_add_map_get_const_str error: {err}");
                        return encoding::NIL_VALUE;
                    }
                };
                Val::map_insert_arcstr(&mut items, encoded_key, value);
                state.encode_value(Val::map(Arc::new(items)))
            }
            other => {
                eprintln!(
                    "lk_rt_map_set_add_map_get_const_str: target is not a Map, got {}",
                    other.type_name()
                );
                encoding::NIL_VALUE
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_map_set_add_map_get_str_int(
    map: i64,
    prefix: *const i8,
    prefix_len: i64,
    suffix: i64,
    lhs: i64,
) -> i64 {
    with_state(|state| {
        let key = cached_str_int_key(state, prefix, prefix_len, suffix);
        let lhs_int = match encoding::decode_immediate(lhs) {
            Val::Int(value) => Some(value),
            _ => None,
        };
        let left = lhs_int.map(Val::Int).unwrap_or_else(|| state.decode_value(lhs));
        if let Some(items) = state.handles.get_ref(map).and_then(Val::as_map) {
            let mut items = items.as_ref().clone();
            if let Some(left) = lhs_int
                && let Some(right) = Val::map_get_str(&items, key.as_str()).and_then(|value| match value {
                    Val::Int(value) => Some(*value),
                    _ => None,
                })
            {
                Val::map_insert_arcstr(&mut items, key, Val::Int(left + right));
                return replace_map_handle(state, map, items);
            }
            let right = Val::map_get_str(&items, key.as_str()).cloned().unwrap_or(Val::Nil);
            let value = match BinOp::Add.eval_vals(&left, &right) {
                Ok(value) => value,
                Err(err) => {
                    eprintln!("lk_rt_map_set_add_map_get_str_int error: {err}");
                    return encoding::NIL_VALUE;
                }
            };
            Val::map_insert_arcstr(&mut items, key, value);
            return replace_map_handle(state, map, items);
        }
        match state.decode_value(map) {
            value if value.as_map().is_some() => {
                let mut items = value.as_map().expect("checked map").as_ref().clone();
                if let Some(left) = lhs_int
                    && let Some(right) = Val::map_get_str(&items, key.as_str()).and_then(|value| match value {
                        Val::Int(value) => Some(*value),
                        _ => None,
                    })
                {
                    Val::map_insert_arcstr(&mut items, key, Val::Int(left + right));
                    return state.encode_value(Val::map(Arc::new(items)));
                }
                let right = Val::map_get_str(&items, key.as_str()).cloned().unwrap_or(Val::Nil);
                let value = match BinOp::Add.eval_vals(&left, &right) {
                    Ok(value) => value,
                    Err(err) => {
                        eprintln!("lk_rt_map_set_add_map_get_str_int error: {err}");
                        return encoding::NIL_VALUE;
                    }
                };
                Val::map_insert_arcstr(&mut items, key, value);
                state.encode_value(Val::map(Arc::new(items)))
            }
            other => {
                eprintln!(
                    "lk_rt_map_set_add_map_get_str_int: target is not a Map, got {}",
                    other.type_name()
                );
                encoding::NIL_VALUE
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_map_update_int_const_str(map: i64, key: *const i8, key_len: i64, init: i64, delta: i64) -> i64 {
    let lookup_key = read_str_int_prefix(key, key_len);
    let encoded_key = Val::intern_str(lookup_key.as_ref());
    with_state(|state| {
        let init_value = state.decode_value(init);
        let delta_value = state.decode_value(delta);
        if let Some(items) = state.handles.get_ref(map).and_then(Val::as_map) {
            let mut items = items.as_ref().clone();
            let value = map_update_value(&items, lookup_key.as_ref(), &init_value, &delta_value);
            Val::map_insert_arcstr(&mut items, encoded_key, value);
            return replace_map_handle(state, map, items);
        }
        match state.decode_value(map) {
            value if value.as_map().is_some() => {
                let mut items = value.as_map().expect("checked map").as_ref().clone();
                let value = map_update_value(&items, lookup_key.as_ref(), &init_value, &delta_value);
                Val::map_insert_arcstr(&mut items, encoded_key, value);
                state.encode_value(Val::map(Arc::new(items)))
            }
            other => {
                eprintln!(
                    "lk_rt_map_update_int_const_str: target is not a Map, got {}",
                    other.type_name()
                );
                encoding::NIL_VALUE
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_map_update_int_str_int(
    map: i64,
    prefix: *const i8,
    prefix_len: i64,
    suffix: i64,
    init: i64,
    delta: i64,
) -> i64 {
    with_state(|state| {
        let key = cached_str_int_key(state, prefix, prefix_len, suffix);
        let init_value = state.decode_value(init);
        let delta_value = state.decode_value(delta);
        if let Some(items) = state.handles.get_ref(map).and_then(Val::as_map) {
            let mut items = items.as_ref().clone();
            let value = map_update_value(&items, key.as_str(), &init_value, &delta_value);
            Val::map_insert_arcstr(&mut items, key, value);
            return replace_map_handle(state, map, items);
        }
        match state.decode_value(map) {
            value if value.as_map().is_some() => {
                let mut items = value.as_map().expect("checked map").as_ref().clone();
                let value = map_update_value(&items, key.as_str(), &init_value, &delta_value);
                Val::map_insert_arcstr(&mut items, key, value);
                state.encode_value(Val::map(Arc::new(items)))
            }
            other => {
                eprintln!(
                    "lk_rt_map_update_int_str_int: target is not a Map, got {}",
                    other.type_name()
                );
                encoding::NIL_VALUE
            }
        }
    })
}

fn map_update_value(
    items: &crate::util::fast_map::FastHashMap<arcstr::ArcStr, Val>,
    key: &str,
    init: &Val,
    delta: &Val,
) -> Val {
    match Val::map_get_str(&items, key) {
        None | Some(Val::Nil) => init.clone(),
        Some(Val::Int(left)) => match delta {
            Val::Int(right) => Val::Int(left + right),
            _ => match BinOp::Add.eval_vals(&Val::Int(*left), delta) {
                Ok(value) => value,
                Err(err) => {
                    eprintln!("lk_rt_map_update_int add error: {err}");
                    Val::Nil
                }
            },
        },
        Some(current) => match BinOp::Add.eval_vals(current, delta) {
            Ok(value) => value,
            Err(err) => {
                eprintln!("lk_rt_map_update_int add error: {err}");
                Val::Nil
            }
        },
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_sub_access(lhs: i64, base: i64, key: i64) -> i64 {
    lk_rt_binop_access(lhs, base, key, BinOp::Sub)
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_index(base: i64, idx: i64) -> i64 {
    with_state(|state| {
        if let Some(list) = state.handles.get_ref(base).and_then(Val::as_list)
            && let Val::Int(i) = encoding::decode_immediate(idx)
        {
            let result = if i < 0 {
                Val::Nil
            } else {
                list.get(i as usize).cloned().unwrap_or(Val::Nil)
            };
            return state.encode_value(result);
        }
        if let Some(value) = state.handles.get_ref(base).and_then(Val::as_str)
            && let Val::Int(i) = encoding::decode_immediate(idx)
        {
            let result = if i < 0 {
                Val::Nil
            } else if value.is_ascii() {
                value
                    .as_bytes()
                    .get(i as usize)
                    .map(|byte| Val::from_str(&(*byte as char).to_string()))
                    .unwrap_or(Val::Nil)
            } else {
                value
                    .chars()
                    .nth(i as usize)
                    .map(|ch| Val::from_str(&ch.to_string()))
                    .unwrap_or(Val::Nil)
            };
            return state.encode_value(result);
        }
        let base_val = state.decode_value(base);
        let idx_val = state.decode_value(idx);
        let result = index_value(&base_val, &idx_val);
        state.encode_value(result)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_index_len(base: i64, idx: i64) -> i64 {
    with_state(|state| {
        let len = if let Some(list) = state.handles.get_ref(base).and_then(Val::as_list) {
            match encoding::decode_immediate(idx) {
                Val::Int(i) if i >= 0 => list.get(i as usize).map(value_len).unwrap_or(0),
                _ => 0,
            }
        } else if let Some(value) = state.handles.get_ref(base).and_then(Val::as_str) {
            match encoding::decode_immediate(idx) {
                Val::Int(i) if i >= 0 => char_len_at(value, i as usize),
                _ => 0,
            }
        } else {
            let base_val = state.decode_value(base);
            let idx_val = state.decode_value(idx);
            let indexed = index_value(&base_val, &idx_val);
            value_len(&indexed)
        };
        state.encode_value(Val::Int(len as i64))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_in(needle: i64, haystack: i64) -> i64 {
    with_state(|state| {
        let l = state.decode_value(needle);
        let r = state.decode_value(haystack);
        match BinOp::In.cmp(&l, &r) {
            Ok(result) => state.encode_value(Val::Bool(result)),
            Err(err) => {
                eprintln!("lk_rt_in error: {err}");
                encoding::NIL_VALUE
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_len(value: i64) -> i64 {
    with_state(|state| {
        if let Some(val) = state.handles.get_ref(value) {
            return state.encode_value(Val::Int(value_len(val) as i64));
        }
        let val = state.decode_value(value);
        state.encode_value(Val::Int(value_len(&val) as i64))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_to_iter(value: i64) -> i64 {
    with_state(|state| {
        if state.handles.get_ref(value).and_then(Val::as_str).is_some() {
            return value;
        }
        let val = state.decode_value(value);
        let iter = match val {
            value if value.as_list().is_some() => value,
            Val::ShortStr(_) => val,
            value if value.as_str().is_some() => value,
            value if value.as_map().is_some() => map_to_iterable(&value.as_map().expect("checked map")),
            Val::Nil | Val::Bool(_) | Val::Int(_) | Val::Float(_) | Val::Obj(_) => Val::list(Arc::new(Vec::new())),
        };
        state.encode_value(iter)
    })
}

fn value_len(value: &Val) -> usize {
    match value {
        value if value.as_list().is_some() => value.as_list().expect("checked list").len(),
        value if value.as_map().is_some() => value.as_map().expect("checked map").len(),
        value if value.as_str().is_some() => value.as_str().unwrap().len(),
        _ => 0,
    }
}

fn char_len_at(value: &str, index: usize) -> usize {
    if value.is_ascii() {
        usize::from(index < value.len())
    } else {
        value.chars().nth(index).map(char::len_utf8).unwrap_or(0)
    }
}

fn access_list_by_immediate_index(handles: &HandleTable, base: i64, key: i64) -> Option<Val> {
    let Val::Int(i) = encoding::decode_immediate(key) else {
        return None;
    };
    let list = handles.get_ref(base).and_then(Val::as_list)?;
    Some(if i < 0 {
        Val::Nil
    } else {
        list.get(i as usize).cloned().unwrap_or(Val::Nil)
    })
}

fn access_map_by_string_handle(handles: &HandleTable, base: i64, key: i64) -> Option<Val> {
    let map = handles.get_ref(base).and_then(Val::as_map)?;
    let key = handles.get_ref(key).and_then(Val::as_str)?;
    Some(Val::map_get_str(&map, key).cloned().unwrap_or(Val::Nil))
}

fn format_str_int_key(state: &RuntimeState, prefix: *const i8, prefix_len: i64, suffix: i64) -> String {
    let prefix = read_str_int_prefix(prefix, prefix_len);
    match encoding::decode_immediate(suffix) {
        Val::Int(i) => {
            let mut key = String::with_capacity(prefix.len() + 20);
            key.push_str(prefix.as_ref());
            key.push_str(&i.to_string());
            key
        }
        Val::Bool(value) => {
            let value = bool_to_str(value);
            let mut key = String::with_capacity(prefix.len() + value.len());
            key.push_str(prefix.as_ref());
            key.push_str(value);
            key
        }
        Val::Nil => {
            let mut key = String::with_capacity(prefix.len() + 3);
            key.push_str(prefix.as_ref());
            key.push_str("nil");
            key
        }
        _ => {
            let suffix = state.decode_value(suffix).to_string();
            let mut key = String::with_capacity(prefix.len() + suffix.len());
            key.push_str(prefix.as_ref());
            key.push_str(&suffix);
            key
        }
    }
}

fn cached_str_int_key(state: &RuntimeState, prefix: *const i8, prefix_len: i64, suffix: i64) -> ArcStr {
    if matches!(
        encoding::decode_immediate(suffix),
        Val::Int(_) | Val::Bool(_) | Val::Nil
    ) {
        let cache_key = (prefix as usize, prefix_len, suffix);
        if let Some(key) = STR_INT_KEY_CACHE.with(|cache| cache.borrow().get(&cache_key).cloned()) {
            return key;
        }
        let key = Val::intern_str(&format_str_int_key(state, prefix, prefix_len, suffix));
        STR_INT_KEY_CACHE.with(|cache| {
            cache.borrow_mut().insert(cache_key, key.clone());
        });
        return key;
    }
    Val::intern_str(&format_str_int_key(state, prefix, prefix_len, suffix))
}

fn read_str_int_prefix<'a>(prefix: *const i8, prefix_len: i64) -> std::borrow::Cow<'a, str> {
    if prefix_len <= 0 || prefix.is_null() {
        return std::borrow::Cow::Borrowed("");
    }
    let len = prefix_len as usize;
    let bytes: &'a [u8] = unsafe { std::slice::from_raw_parts(prefix.cast::<u8>(), len) };
    match std::str::from_utf8(bytes) {
        Ok(value) => std::borrow::Cow::Borrowed(value),
        Err(_) => std::borrow::Cow::Owned(String::from_utf8_lossy(bytes).into_owned()),
    }
}

fn lk_rt_binop_access(lhs: i64, base: i64, key: i64, op: BinOp) -> i64 {
    with_state(|state| {
        if let Some(value) = list_int_binop_access(state, lhs, base, key, &op) {
            return value;
        }
        let left = state.decode_value(lhs);
        let base_val = state.decode_value(base);
        let key_val = state.decode_value(key);
        let right = base_val.access(&key_val).unwrap_or(Val::Nil);
        match op.eval_vals(&left, &right) {
            Ok(value) => state.encode_value(value),
            Err(err) => {
                eprintln!("{} error: {err}", access_binop_helper_name(op));
                encoding::NIL_VALUE
            }
        }
    })
}

fn list_int_binop_access(state: &mut RuntimeState, lhs: i64, base: i64, key: i64, op: &BinOp) -> Option<i64> {
    let Val::Int(left) = encoding::decode_immediate(lhs) else {
        return None;
    };
    let Val::Int(i) = encoding::decode_immediate(key) else {
        return None;
    };
    if i < 0 {
        return None;
    }
    let list = state.handles.get_ref(base).and_then(Val::as_list)?;
    let Some(Val::Int(right)) = list.get(i as usize) else {
        return None;
    };
    let value = match op {
        BinOp::Add => left + right,
        BinOp::Sub => left - right,
        BinOp::Mul => left * right,
        _ => unreachable!("unsupported access binop"),
    };
    Some(state.encode_value(Val::Int(value)))
}

fn access_binop_helper_name(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "lk_rt_add_access",
        BinOp::Sub => "lk_rt_sub_access",
        BinOp::Mul => "lk_rt_mul_access",
        _ => "lk_rt_binop_access",
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn lk_rt_list_slice(list: i64, start: i64) -> i64 {
    with_state(|state| {
        let list_val = state.decode_value(list);
        let start_val = state.decode_value(start);
        let result = match (list_val, start_val) {
            (value, Val::Int(i)) if value.as_list().is_some() => list_slice(&value.as_list().expect("checked list"), i),
            _ => Val::Nil,
        };
        state.encode_value(result)
    })
}
