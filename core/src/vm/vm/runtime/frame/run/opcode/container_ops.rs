use std::sync::Arc;

use anyhow::{Result, anyhow};
use arcstr::ArcStr;

use crate::util::fast_map::{FastHashMap, fast_hash_map_with_capacity};
use crate::val::Val;
use crate::vm::RegionPlan;
use crate::vm::alloc::{AllocationRegion, RegionAllocator};
use crate::vm::vm::Vm;
use crate::vm::vm::caches::{AccessIc, IndexIc};
use crate::vm::vm::frame::FrameState;
use crate::vm::vm::quickening::{self, QuickeningSite};

use super::super::helpers::{assign_reg, insert_map_entry, push_list_entry};
use super::super::raw_boundary::region_allocator;

mod list_ops;
mod map_ops;
mod scalar_ops;
mod string_ops;

pub(super) use list_ops::run_fold_add as run_list_fold_add;
pub(super) use map_ops::{
    run_has as run_map_has, run_has_k as run_map_has_k, run_values_fold_add as run_map_values_fold_add,
};
pub(super) use scalar_ops::{run_floor, run_len, run_list_len, run_map_len, run_str_len};

#[inline]
pub(super) fn run_access(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    access_ic: &mut Vec<Option<AccessIc>>,
    pc: usize,
    dst: u16,
    base: u16,
    field: u16,
) {
    let hit_val = match (&regs[base as usize], &regs[field as usize]) {
        (Val::List(list), Val::Int(index)) => list_ops::index_value(list, *index),
        (base_val, Val::Int(index)) if base_val.as_str().is_some() => {
            string_ops::index_value(base_val.as_str().unwrap(), *index)
        }
        (Val::Map(map), key) if key.as_str().is_some() => Val::map_get_str(map, key.as_str().unwrap()).cloned(),
        (Val::Object(object), key) if key.as_str().is_some() => {
            let fields = &object.fields;
            let object_ptr = Arc::as_ptr(fields) as usize;
            let key = key.as_str().unwrap();
            match access_ic[pc].as_mut() {
                Some(AccessIc::ObjectStr(slots)) => {
                    Vm::lookup_promote(slots, |entry| entry.obj_ptr == object_ptr && entry.key.as_str() == key)
                        .map(|entry| entry.value.clone())
                }
                _ => None,
            }
        }
        _ => None,
    };
    let result = if let Some(value) = hit_val {
        value
    } else {
        let value = regs[base as usize].access(&regs[field as usize]).unwrap_or(Val::Nil);
        if let (Val::Object(object), field_val) = (&regs[base as usize], &regs[field as usize])
            && let Some(key) = field_val.as_str()
        {
            let fields = &object.fields;
            let object_ptr = Arc::as_ptr(fields) as usize;
            Vm::update_object_ic(access_ic.as_mut_slice(), pc, object_ptr, key, &value);
        }
        value
    };
    assign_reg(frame_raw, regs, dst as usize, result);
}

#[inline]
pub(super) fn run_access_k(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    access_ic: &mut Vec<Option<AccessIc>>,
    pc: usize,
    dst: u16,
    base: u16,
    key_index: u16,
) {
    let key = &consts[key_index as usize];
    let result = if let Some(key_str) = key.as_str() {
        let (hit_value, object_ptr) = match &regs[base as usize] {
            Val::Map(map) => (Some(Val::map_get_str(map, key_str).cloned().unwrap_or(Val::Nil)), None),
            Val::Object(object) => {
                let fields = &object.fields;
                let object_ptr = Arc::as_ptr(fields) as usize;
                let hit = match access_ic[pc].as_mut() {
                    Some(AccessIc::ObjectStr(slots)) => Vm::lookup_promote(slots, |entry| {
                        entry.obj_ptr == object_ptr && entry.key.as_str() == key_str
                    })
                    .map(|entry| entry.value.clone()),
                    _ => None,
                };
                (hit, Some(object_ptr))
            }
            _ => (None, None),
        };
        if let Some(value) = hit_value {
            value
        } else {
            let value = regs[base as usize].access(key).unwrap_or(Val::Nil);
            if let Some(object_ptr) = object_ptr {
                Vm::update_object_ic(access_ic.as_mut_slice(), pc, object_ptr, key_str, &value);
            }
            value
        }
    } else {
        Val::Nil
    };
    assign_reg(frame_raw, regs, dst as usize, result);
}

#[inline]
pub(super) fn run_map_get_interned(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    map: u16,
    key_index: u16,
) {
    let key = consts[key_index as usize].as_str().unwrap_or("");
    let result = match &regs[map as usize] {
        Val::Map(map) => Val::map_get_str(map, key).cloned().unwrap_or(Val::Nil),
        _ => Val::Nil,
    };
    assign_reg(frame_raw, regs, dst as usize, result);
}

#[inline]
pub(super) fn run_map_get_dynamic(frame_raw: *mut FrameState<'_>, regs: &mut [Val], dst: u16, map: u16, key: u16) {
    let result = match (&regs[map as usize], regs[key as usize].as_str()) {
        (Val::Map(map), Some(key)) => Val::map_get_str(map, key).cloned().unwrap_or(Val::Nil),
        _ => Val::Nil,
    };
    assign_reg(frame_raw, regs, dst as usize, result);
}

#[inline]
pub(super) fn run_index(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    index_ic: &mut Vec<Option<IndexIc>>,
    quickening: &mut Vec<QuickeningSite>,
    pc: usize,
    dst: u16,
    base: u16,
    index: u16,
) -> Result<()> {
    if quickening::execute_index_site(quickening, pc, regs, dst, base, index)? {
        return Ok(());
    }
    let result = match (&regs[base as usize], &regs[index as usize]) {
        (Val::List(list), Val::Int(index)) => {
            if *index < 0 {
                list_ops::index_value(list, *index).unwrap_or(Val::Nil)
            } else {
                let list_ptr = Arc::as_ptr(list) as *const Val as usize;
                let hit = match index_ic[pc].as_mut() {
                    Some(IndexIc::List(slots)) => {
                        Vm::lookup_promote(slots, |entry| entry.base_ptr == list_ptr && entry.idx == *index)
                            .map(|entry| entry.value.clone())
                    }
                    _ => None,
                };
                if let Some(value) = hit {
                    value
                } else {
                    let value = list.get(*index as usize).cloned().unwrap_or(Val::Nil);
                    Vm::update_list_ic(index_ic.as_mut_slice(), pc, list_ptr, *index, &value);
                    value
                }
            }
        }
        (base_val, Val::Int(index)) if base_val.as_str().is_some() => {
            let text = base_val.as_str().unwrap();
            if *index < 0 {
                string_ops::index_value(text, *index).unwrap_or(Val::Nil)
            } else {
                let string_ptr = text.as_ptr() as usize;
                let hit = match index_ic[pc].as_mut() {
                    Some(IndexIc::Str(slots)) => {
                        Vm::lookup_promote(slots, |entry| entry.base_ptr == string_ptr && entry.idx == *index)
                            .map(|entry| entry.value.clone())
                    }
                    _ => None,
                };
                if let Some(value) = hit {
                    value
                } else {
                    let value = string_ops::index_value(text, *index).unwrap_or(Val::Nil);
                    Vm::update_str_ic(index_ic.as_mut_slice(), pc, string_ptr, *index, &value);
                    value
                }
            }
        }
        (Val::List(list), Val::List(key)) => list_ops::slice_range_value(list, key).unwrap_or(Val::Nil),
        (base_val, Val::List(key)) if base_val.as_str().is_some() => {
            string_ops::slice_range_value(base_val.as_str().unwrap(), key).unwrap_or(Val::Nil)
        }
        (base_val, key) => base_val.access(key).unwrap_or(Val::Nil),
    };
    assign_reg(frame_raw, regs, dst as usize, result);
    Ok(())
}

#[inline]
pub(super) fn run_index_k(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    consts: &[Val],
    dst: u16,
    base: u16,
    key_index: u16,
) {
    let key = &consts[key_index as usize];
    let result = if let Val::Int(index) = key {
        match &regs[base as usize] {
            Val::List(list) => list_ops::index_value(list, *index).unwrap_or(Val::Nil),
            value if value.as_str().is_some() => {
                string_ops::index_value(value.as_str().unwrap(), *index).unwrap_or(Val::Nil)
            }
            _ => Val::Nil,
        }
    } else {
        Val::Nil
    };
    assign_reg(frame_raw, regs, dst as usize, result);
}

#[inline]
pub(super) fn run_list_index_i(frame_raw: *mut FrameState<'_>, regs: &mut [Val], dst: u16, base: u16, index: i16) {
    let result = match &regs[base as usize] {
        Val::List(list) => list_ops::index_value(list, index as i64).unwrap_or(Val::Nil),
        _ => Val::Nil,
    };
    assign_reg(frame_raw, regs, dst as usize, result);
}

#[inline]
pub(super) fn run_str_index_i(frame_raw: *mut FrameState<'_>, regs: &mut [Val], dst: u16, base: u16, index: i16) {
    let result = match &regs[base as usize] {
        value if value.as_str().is_some() => {
            string_ops::index_value(value.as_str().unwrap(), index as i64).unwrap_or(Val::Nil)
        }
        _ => Val::Nil,
    };
    assign_reg(frame_raw, regs, dst as usize, result);
}

#[inline]
fn use_thread_local(region_plan: Option<&RegionPlan>, dst: u16) -> bool {
    region_plan
        .map(|plan| plan.region_for(dst as usize) == AllocationRegion::ThreadLocal)
        .unwrap_or(false)
}

#[inline]
pub(super) fn run_to_iter(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    dst: u16,
    src: u16,
    region_plan: Option<&RegionPlan>,
    region_allocator_ptr: *const RegionAllocator,
) {
    let out = match &regs[src as usize] {
        v if matches!(v, Val::List(_)) || v.as_str().is_some() => regs[src as usize].clone(),
        Val::Map(m) => {
            let mut entries: Vec<_> = m.iter().collect();
            entries.sort_by(|(left, _), (right, _)| left.as_str().cmp(right.as_str()));
            if use_thread_local(region_plan, dst) && !entries.is_empty() {
                let allocator = region_allocator(region_allocator_ptr);
                allocator.with_val_buffer(entries.len(), |scratch| {
                    for (key, value) in entries.iter() {
                        scratch.push(Val::List(vec![Val::from_str(key.as_str()), (*value).clone()].into()));
                    }
                    let data = scratch.split_off(0);
                    Val::List(data.into())
                })
            } else {
                let mut pairs = Vec::with_capacity(entries.len());
                for (key, value) in entries {
                    pairs.push(Val::List(vec![Val::from_str(key.as_str()), value.clone()].into()));
                }
                Val::List(pairs.into())
            }
        }
        _ => Val::List(Vec::<Val>::new().into()),
    };
    assign_reg(frame_raw, regs, dst as usize, out);
}

#[inline]
pub(super) fn run_build_list(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    dst: u16,
    base: u16,
    len: u16,
    region_plan: Option<&RegionPlan>,
    region_allocator_ptr: *const RegionAllocator,
) {
    let start = base as usize;
    let n = len as usize;
    if use_thread_local(region_plan, dst) {
        let allocator = region_allocator(region_allocator_ptr);
        let list_val = allocator.with_val_buffer(n, |scratch| {
            scratch.extend((0..n).map(|i| regs[start + i].clone()));
            let data = scratch.split_off(0);
            Val::List(data.into())
        });
        assign_reg(frame_raw, regs, dst as usize, list_val);
    } else {
        let mut values = Vec::with_capacity(n);
        for i in 0..n {
            values.push(regs[start + i].clone());
        }
        assign_reg(frame_raw, regs, dst as usize, Val::List(values.into()));
    }
}

#[inline]
pub(super) fn run_build_map(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    dst: u16,
    base: u16,
    len: u16,
    region_plan: Option<&RegionPlan>,
    region_allocator_ptr: *const RegionAllocator,
) -> Result<()> {
    let start = base as usize;
    let n = len as usize;
    if use_thread_local(region_plan, dst) {
        let allocator = region_allocator(region_allocator_ptr);
        let map_val = allocator.with_map_entries(n, |entries| {
            for i in 0..n {
                let key_val = &regs[start + 2 * i];
                let value = regs[start + 2 * i + 1].clone();
                let key_arc: ArcStr = key_val
                    .primitive_key_arcstr()
                    .ok_or_else(|| anyhow!("Map key must be a primitive type, got: {:?}", key_val))?;
                entries.push((key_arc, value));
            }
            let mut map: FastHashMap<ArcStr, Val> = fast_hash_map_with_capacity(entries.len());
            for (key, value) in entries.drain(..) {
                Val::map_insert_arcstr(&mut map, key, value);
            }
            Ok::<Val, anyhow::Error>(Val::Map(Arc::new(map)))
        })?;
        assign_reg(frame_raw, regs, dst as usize, map_val);
    } else {
        let mut map: FastHashMap<ArcStr, Val> = fast_hash_map_with_capacity(n);
        for i in 0..n {
            let key = &regs[start + 2 * i];
            let value = regs[start + 2 * i + 1].clone();
            let key_arc: ArcStr = key
                .primitive_key_arcstr()
                .ok_or_else(|| anyhow!("Map key must be a primitive type, got: {:?}", key))?;
            Val::map_insert_arcstr(&mut map, key_arc, value);
        }
        assign_reg(frame_raw, regs, dst as usize, Val::Map(Arc::new(map)));
    }
    Ok(())
}

#[inline]
pub(super) fn run_list_slice(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    dst: u16,
    src: u16,
    start: u16,
    region_plan: Option<&RegionPlan>,
    region_allocator_ptr: *const RegionAllocator,
) -> Result<()> {
    let (list, start_idx) = match (&regs[src as usize], &regs[start as usize]) {
        (Val::List(list), Val::Int(index)) => (list, *index),
        (left, right) => return Err(anyhow!("ListSlice expects (List, Int), got ({:?}, {:?})", left, right)),
    };
    if start_idx <= 0 {
        assign_reg(frame_raw, regs, dst as usize, Val::List(list.clone()));
    } else {
        let start = start_idx as usize;
        if start >= list.len() {
            assign_reg(frame_raw, regs, dst as usize, Val::List(Vec::<Val>::new().into()));
        } else if use_thread_local(region_plan, dst) {
            let allocator = region_allocator(region_allocator_ptr);
            let slice_val = allocator.with_val_buffer(list.len() - start, |scratch| {
                scratch.extend(list[start..].iter().cloned());
                let data = scratch.split_off(0);
                Val::List(data.into())
            });
            assign_reg(frame_raw, regs, dst as usize, slice_val);
        } else {
            assign_reg(
                frame_raw,
                regs,
                dst as usize,
                Val::List((list[start..]).to_vec().into()),
            );
        }
    }
    Ok(())
}

#[inline]
pub(super) fn run_list_push(regs: &mut [Val], list: u16, val: u16) -> Result<()> {
    let pushed_val = regs[val as usize].clone();
    match &mut regs[list as usize] {
        Val::List(arc) => {
            push_list_entry(arc, pushed_val);
            Ok(())
        }
        _ => Err(anyhow!("ListPush target is not a List")),
    }
}

#[inline]
pub(super) fn run_list_push_move(regs: &mut [Val], list: u16, val: u16) -> Result<()> {
    let list_idx = list as usize;
    let val_idx = val as usize;
    if list_idx == val_idx {
        return run_list_push(regs, list, val);
    }
    if !matches!(regs[list_idx], Val::List(_)) {
        return Err(anyhow!("ListPush target is not a List"));
    }
    let pushed_val = std::mem::replace(&mut regs[val_idx], Val::Nil);
    match &mut regs[list_idx] {
        Val::List(arc) => {
            push_list_entry(arc, pushed_val);
            Ok(())
        }
        _ => unreachable!("ListPush target was checked before moving value"),
    }
}

#[inline]
pub(super) fn run_list_set_i(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    dst: u16,
    list: u16,
    index: i16,
    val: u16,
) -> Result<()> {
    if index < 0 {
        return Err(anyhow!("set() index must be non-negative"));
    }
    let Val::List(items) = &regs[list as usize] else {
        return Err(anyhow!("set() first argument must be a list"));
    };
    let index = index as usize;
    let Some(old) = items.get(index).cloned() else {
        return Err(anyhow!("index {} out of bounds for len {}", index, items.len()));
    };
    let mut updated = Vec::with_capacity(items.len());
    updated.extend(items.iter().cloned());
    updated[index] = regs[val as usize].clone();
    let out = Val::List(vec![Val::List(Arc::new(updated)), old].into());
    assign_reg(frame_raw, regs, dst as usize, out);
    Ok(())
}

#[inline]
pub(super) fn run_map_set(regs: &mut [Val], map: u16, key: u16, val: u16) -> Result<()> {
    let key_arc = regs[key as usize]
        .string_key_arcstr()
        .ok_or_else(|| anyhow!("MapSet key must be a String"))?;
    let pushed_val = regs[val as usize].clone();
    match &mut regs[map as usize] {
        Val::Map(arc) => {
            insert_map_entry(arc, key_arc, pushed_val);
            Ok(())
        }
        _ => Err(anyhow!("MapSet target is not a Map")),
    }
}

#[inline]
pub(super) fn run_map_set_interned(regs: &mut [Val], consts: &[Val], map: u16, kidx: u16, val: u16) -> Result<()> {
    let key_arc = consts[kidx as usize]
        .string_key_arcstr()
        .ok_or_else(|| anyhow!("MapSetInterned key must be a String"))?;
    let pushed_val = regs[val as usize].clone();
    match &mut regs[map as usize] {
        Val::Map(arc) => {
            insert_map_entry(arc, key_arc, pushed_val);
            Ok(())
        }
        _ => Err(anyhow!("MapSet target is not a Map")),
    }
}

#[inline]
pub(super) fn run_map_set_interned_move(regs: &mut [Val], consts: &[Val], map: u16, kidx: u16, val: u16) -> Result<()> {
    let map_idx = map as usize;
    let val_idx = val as usize;
    if map_idx == val_idx {
        return run_map_set_interned(regs, consts, map, kidx, val);
    }
    let key_arc = consts[kidx as usize]
        .string_key_arcstr()
        .ok_or_else(|| anyhow!("MapSetInterned key must be a String"))?;
    if !matches!(regs[map_idx], Val::Map(_)) {
        return Err(anyhow!("MapSet target is not a Map"));
    }
    let pushed_val = std::mem::replace(&mut regs[val_idx], Val::Nil);
    match &mut regs[map_idx] {
        Val::Map(arc) => {
            insert_map_entry(arc, key_arc, pushed_val);
            Ok(())
        }
        _ => unreachable!("MapSet target was checked before moving value"),
    }
}

#[inline]
pub(super) fn run_map_set_move(regs: &mut [Val], map: u16, key: u16, val: u16) -> Result<()> {
    let map_idx = map as usize;
    let key_idx = key as usize;
    let val_idx = val as usize;
    if map_idx == key_idx || map_idx == val_idx || key_idx == val_idx {
        return run_map_set(regs, map, key, val);
    }
    if !matches!(regs[map_idx], Val::Map(_)) {
        return Err(anyhow!("MapSet target is not a Map"));
    }
    let key_val = std::mem::replace(&mut regs[key_idx], Val::Nil);
    let key_arc = match key_val.string_key_arcstr() {
        Some(key_arc) => key_arc,
        None => {
            regs[key_idx] = key_val;
            return Err(anyhow!("MapSet key must be a String"));
        }
    };
    let pushed_val = std::mem::replace(&mut regs[val_idx], Val::Nil);
    match &mut regs[map_idx] {
        Val::Map(arc) => {
            insert_map_entry(arc, key_arc, pushed_val);
            Ok(())
        }
        _ => unreachable!("MapSet target was checked before moving key/value"),
    }
}
