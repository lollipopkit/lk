use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::val::{HeapValue, RuntimeMapKey, RuntimeVal, ShortStr, TypedList, TypedMap};
use crate::vm::analysis::{PerfIndexFact, PerfIndexTargetKind, VM_INDEX_KEY_METRIC_COUNT, VmIndexKeyMetric};

use super::{Executor, IndexTargetKind, heap_kind, record_index_key_metric, runtime_map_key_from_str};

impl Executor {
    #[inline(always)]
    pub(in crate::vm::exec) fn try_get_known_list_index(&self, target_reg: u8, key_reg: u8) -> Option<RuntimeVal> {
        let RuntimeVal::Obj(handle) = self.read_unchecked(target_reg) else {
            return None;
        };
        let RuntimeVal::Int(index) = self.read_unchecked(key_reg) else {
            return None;
        };
        let Some(HeapValue::List(list)) = self.state.heap.get(*handle) else {
            return None;
        };
        let index = if *index < 0 {
            let index = list.len() as i64 + *index;
            if index < 0 {
                return Some(RuntimeVal::Nil);
            }
            index as usize
        } else {
            *index as usize
        };
        Some(self.get_typed_list_element(list, index))
    }

    #[inline(always)]
    pub(in crate::vm::exec) fn get_index(
        &mut self,
        pc: usize,
        target_reg: u8,
        key_reg: u8,
        known_string_key: Option<&str>,
        index_fact: Option<PerfIndexFact>,
        index_key_metrics: Option<&mut [u64; VM_INDEX_KEY_METRIC_COUNT]>,
    ) -> Result<RuntimeVal> {
        // Fast path: check if key is a List object for slice indexing (rare case).
        if let RuntimeVal::Obj(h) = self.read_unchecked(key_reg)
            && let Some(HeapValue::List(list)) = self.state.heap.get(*h)
        {
            let items = list.collect_owned();
            if !items.is_empty() && items.len() <= 3 {
                let start = match &items[0] {
                    RuntimeVal::Int(i) => *i,
                    _ => 0i64,
                };
                let last = items
                    .last()
                    .and_then(|v| if let RuntimeVal::Int(i) = v { Some(*i) } else { None });
                return self.get_index_slice(target_reg, start, last.map(|i| i + 1), None);
            }
        }
        match self.read_unchecked(target_reg) {
            RuntimeVal::ShortStr(value) => {
                let value = *value;
                let idx_val = self.read_unchecked(key_reg);
                let idx = match idx_val {
                    RuntimeVal::Int(n) => {
                        let len = value.as_str().len() as i64;
                        if *n < 0 { (len + *n) as usize } else { *n as usize }
                    }
                    _ => bail!("String index must be Int"),
                };
                self.index_string_at(value.as_str(), idx)
            }
            RuntimeVal::Obj(handle) => {
                let handle = *handle;
                self.get_heap_index(pc, handle, key_reg, known_string_key, index_fact, index_key_metrics)
            }
            other => bail!("GetIndex target expected Obj, got {:?}", other.kind()),
        }
    }

    #[inline(always)]
    fn get_heap_index(
        &mut self,
        pc: usize,
        handle: crate::val::HeapRef,
        key_reg: u8,
        known_string_key: Option<&str>,
        index_fact: Option<PerfIndexFact>,
        mut index_key_metrics: Option<&mut [u64; VM_INDEX_KEY_METRIC_COUNT]>,
    ) -> Result<RuntimeVal> {
        // Fast path: when index_fact confirms Map target, do direct map lookup.
        if let Some(fact) = index_fact {
            if fact.target_kind == PerfIndexTargetKind::Map {
                if let Some(key_str) = known_string_key {
                    record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::KnownStringKey);
                    record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::DirectStringKey);
                    if let Some(HeapValue::Map(map)) = self.state.heap.get(handle) {
                        if let Some(value) = get_string_map_direct(map, key_str) {
                            record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::TypedMapDirect);
                            return Ok(value);
                        }
                        record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::GenericMapLookup);
                        return Ok(map.get_str(key_str).unwrap_or(RuntimeVal::Nil));
                    }
                } else {
                    record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::DynamicRegisterKey);
                    // Dynamic key from register: avoid RuntimeMapKey construction
                    return self.get_map_index_fast(handle, key_reg, index_key_metrics);
                }
            }
            // For list with known type, skip the slow path too
            if fact.target_kind == PerfIndexTargetKind::List {
                let key_val = self.read_unchecked(key_reg);
                if let RuntimeVal::Int(n) = key_val {
                    if let Some(HeapValue::List(list)) = self.state.heap.get(handle) {
                        let index = if *n < 0 {
                            let index = list.len() as i64 + *n;
                            if index < 0 {
                                return Ok(RuntimeVal::Nil);
                            }
                            index as usize
                        } else {
                            *n as usize
                        };
                        return Ok(self.get_typed_list_element(list, index));
                    }
                }
            }
            if fact.target_kind == PerfIndexTargetKind::String {
                let key_val = self.read_unchecked(key_reg);
                if let RuntimeVal::Int(n) = key_val
                    && let Some(HeapValue::String(value)) = self.state.heap.get(handle)
                {
                    let index = if *n < 0 {
                        let index = value.len() as i64 + *n;
                        if index < 0 {
                            return Ok(RuntimeVal::Nil);
                        }
                        index as usize
                    } else {
                        *n as usize
                    };
                    return self.index_string_at(value, index);
                }
            }
        }

        self.get_heap_index_slow_path(pc, handle, key_reg, known_string_key, index_fact, index_key_metrics)
    }

    /// Read a value from a typed list by index, converting to RuntimeVal.
    /// Returns RuntimeVal::Nil for out-of-bounds or unsupported types.
    #[inline(always)]
    fn get_typed_list_element(&self, list: &TypedList, index: usize) -> RuntimeVal {
        match list {
            TypedList::Int(values) => values
                .get(index)
                .copied()
                .map(RuntimeVal::Int)
                .unwrap_or(RuntimeVal::Nil),
            TypedList::Float(values) => values
                .get(index)
                .copied()
                .map(RuntimeVal::Float)
                .unwrap_or(RuntimeVal::Nil),
            TypedList::Bool(values) => values
                .get(index)
                .copied()
                .map(RuntimeVal::Bool)
                .unwrap_or(RuntimeVal::Nil),
            TypedList::Mixed(values) => values.get(index).cloned().unwrap_or(RuntimeVal::Nil),
            TypedList::String(values) => match values.get(index) {
                Some(value) => ShortStr::new(value)
                    .map(RuntimeVal::ShortStr)
                    .unwrap_or_else(|| RuntimeVal::Nil),
                None => RuntimeVal::Nil,
            },
        }
    }

    /// Fast map index lookup that avoids RuntimeMapKey construction.
    /// Reads the key directly from register and dispatches based on runtime key type.
    #[inline(always)]
    fn get_map_index_fast(
        &mut self,
        handle: crate::val::HeapRef,
        key_reg: u8,
        mut index_key_metrics: Option<&mut [u64; VM_INDEX_KEY_METRIC_COUNT]>,
    ) -> Result<RuntimeVal> {
        let key_val = self.read_unchecked(key_reg);
        match &key_val {
            RuntimeVal::ShortStr(short) => {
                let key_str = short.as_str();
                // When value kind is known, use direct typed-map fast path
                // to avoid the generic TypedMap::get_str match overhead.
                match self.state.heap.get(handle) {
                    Some(HeapValue::Map(map)) => {
                        if let Some(value) = get_string_map_direct(map, key_str) {
                            record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::TypedMapDirect);
                            return Ok(value);
                        }
                        record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::DirectStringKey);
                        record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::GenericMapLookup);
                        Ok(map.get_str(key_str).unwrap_or(RuntimeVal::Nil))
                    }
                    Some(other) => bail!("GetIndex target object changed while indexing: {:?}", heap_kind(other)),
                    None => bail!("heap object {} out of bounds", handle.index()),
                }
            }
            RuntimeVal::Int(n) => {
                record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::RuntimeMapKey);
                let key = RuntimeMapKey::Int(*n);
                match self.state.heap.get(handle) {
                    Some(HeapValue::Map(map)) => Ok(map.get(&key).unwrap_or(RuntimeVal::Nil)),
                    Some(other) => bail!("GetIndex target object changed while indexing: {:?}", heap_kind(other)),
                    None => bail!("heap object {} out of bounds", handle.index()),
                }
            }
            RuntimeVal::Obj(_) => {
                // Long string key: need heap lookup for the key, fall back to slow path
                let _ = key_val;
                record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::RuntimeMapKey);
                let key = self.map_key_from_register(key_reg)?;
                Ok(self.lookup_map_handle(handle, &key)?.unwrap_or(RuntimeVal::Nil))
            }
            _ => {
                // Bool, Nil, Float keys - rare, fall back
                let _ = key_val;
                record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::RuntimeMapKey);
                let key = self.map_key_from_register(key_reg)?;
                Ok(self.lookup_map_handle(handle, &key)?.unwrap_or(RuntimeVal::Nil))
            }
        }
    }

    #[cold]
    fn get_heap_index_slow_path(
        &mut self,
        pc: usize,
        handle: crate::val::HeapRef,
        key_reg: u8,
        known_string_key: Option<&str>,
        index_fact: Option<PerfIndexFact>,
        mut index_key_metrics: Option<&mut [u64; VM_INDEX_KEY_METRIC_COUNT]>,
    ) -> Result<RuntimeVal> {
        record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::SlowPath);
        let index_cache = match index_fact {
            Some(_) => None,
            None => self.cached_or_observed_index_cache(pc, handle, known_string_key)?,
        };
        let index_fact = index_fact.or_else(|| index_cache.map(|cache| cache.fact));
        let observed_kind = self.index_target_kind(handle)?;
        let target_kind = match index_fact.map(|fact| fact.target_kind) {
            Some(PerfIndexTargetKind::List) => IndexTargetKind::List,
            Some(PerfIndexTargetKind::Map) => IndexTargetKind::Map,
            Some(PerfIndexTargetKind::Object) => IndexTargetKind::Object,
            Some(PerfIndexTargetKind::String) => IndexTargetKind::String,
            Some(PerfIndexTargetKind::Unknown) | None => observed_kind,
        };
        let target_kind = if target_kind == observed_kind {
            target_kind
        } else {
            observed_kind
        };

        match target_kind {
            IndexTargetKind::List => {
                if let Some(pos) = self.negative_list_index(handle, key_reg) {
                    let orig_val = self.read(key_reg)?.clone();
                    self.write(key_reg, RuntimeVal::Int(pos as i64))?;
                    let result = self.index_list_handle(handle, key_reg, index_fact.map(|fact| fact.value_kind));
                    self.write(key_reg, orig_val)?;
                    return result;
                }
                self.index_list_handle(handle, key_reg, index_fact.map(|fact| fact.value_kind))
            }
            IndexTargetKind::Map => {
                if let Some(key) = known_string_key
                    && let Some(value) =
                        self.lookup_string_map_handle(handle, key, index_fact.map(|fact| fact.value_kind))?
                {
                    record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::KnownStringKey);
                    record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::DirectStringKey);
                    return Ok(value);
                }
                let key = match known_string_key {
                    Some(key_str) => {
                        record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::KnownStringKey);
                        record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::RuntimeMapKey);
                        runtime_map_key_from_str(key_str)
                    }
                    None => {
                        record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::DynamicRegisterKey);
                        record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::RuntimeMapKey);
                        self.map_key_from_register(key_reg)?
                    }
                };
                record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::GenericMapLookup);
                Ok(self.lookup_map_handle(handle, &key)?.unwrap_or(RuntimeVal::Nil))
            }
            IndexTargetKind::Object => {
                let key = match known_string_key {
                    Some(key_str) => {
                        record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::KnownStringKey);
                        Arc::<str>::from(key_str)
                    }
                    None => {
                        record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::DynamicRegisterKey);
                        record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::ObjectKey);
                        self.object_key_from_register(key_reg)?
                    }
                };
                let field_slot = index_cache.and_then(|cache| cache.object_field_slot);
                Ok(self
                    .index_object_handle(handle, &key, field_slot)?
                    .unwrap_or(RuntimeVal::Nil))
            }
            IndexTargetKind::String => self.get_heap_string_index(handle, key_reg),
        }
    }

    #[cold]
    fn negative_list_index(&self, handle: crate::val::HeapRef, key_reg: u8) -> Option<usize> {
        let n = self.read_int(key_reg).ok()?;
        if n >= 0 {
            return None;
        }
        let len = self.state.heap.get(handle).and_then(|v| match v {
            HeapValue::List(l) => Some(l.len()),
            _ => None,
        })?;
        Some(((len as i64) + n) as usize)
    }

    #[cold]
    fn get_heap_string_index(&mut self, handle: crate::val::HeapRef, key_reg: u8) -> Result<RuntimeVal> {
        if let Some(pos) = self.negative_string_index(handle, key_reg) {
            let orig_val = self.read(key_reg)?.clone();
            self.write(key_reg, RuntimeVal::Int(pos as i64))?;
            let result = self.index_heap_string_at_key(handle, key_reg);
            self.write(key_reg, orig_val)?;
            return result;
        }
        self.index_heap_string_at_key(handle, key_reg)
    }

    #[cold]
    fn negative_string_index(&self, handle: crate::val::HeapRef, key_reg: u8) -> Option<usize> {
        let n = self.read_int(key_reg).ok()?;
        if n >= 0 {
            return None;
        }
        let s = match self.state.heap.get(handle)? {
            HeapValue::String(value) => value,
            _ => return None,
        };
        Some(((s.len() as i64) + n) as usize)
    }

    #[inline(always)]
    #[cold]
    fn index_heap_string_at_key(&self, handle: crate::val::HeapRef, key_reg: u8) -> Result<RuntimeVal> {
        match self
            .state
            .heap
            .get(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::String(value) => {
                let idx_val = self.read(key_reg)?;
                let idx = match &idx_val {
                    RuntimeVal::Int(n) => {
                        let len = value.len() as i64;
                        if *n < 0 { (len + *n) as usize } else { *n as usize }
                    }
                    _ => bail!("String index must be Int"),
                };
                self.index_string_at(value, idx)
            }
            other => bail!("GetIndex target object changed while indexing: {:?}", heap_kind(other)),
        }
    }
}

#[inline(always)]
fn get_string_map_direct(map: &TypedMap, key: &str) -> Option<RuntimeVal> {
    match map {
        TypedMap::Mixed(_) => None,
        TypedMap::StringMixed(values) => Some(values.get(key).cloned().unwrap_or(RuntimeVal::Nil)),
        TypedMap::StringInt(values) => Some(values.get(key).copied().map(RuntimeVal::Int).unwrap_or(RuntimeVal::Nil)),
        TypedMap::StringFloat(values) => Some(
            values
                .get(key)
                .copied()
                .map(RuntimeVal::Float)
                .unwrap_or(RuntimeVal::Nil),
        ),
        TypedMap::StringBool(values) => Some(
            values
                .get(key)
                .copied()
                .map(RuntimeVal::Bool)
                .unwrap_or(RuntimeVal::Nil),
        ),
    }
}
