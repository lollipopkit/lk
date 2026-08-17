#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use alloc::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::val::{HeapValue, RuntimeMapKey, RuntimeVal, ShortStr, ShortStrOrStr, TypedList, TypedMap};
use crate::vm::analysis::{PerfIndexFact, PerfIndexTargetKind, VM_INDEX_KEY_METRIC_COUNT, VmIndexKeyMetric};

use super::{
    Executor, IndexTargetKind, record_dynamic_index_key_metric, record_index_key_metric, runtime_map_key_from_str,
};

impl Executor {
    #[inline(always)]
    pub(in crate::vm::exec) fn get_list_index(&mut self, target_reg: u8, key_reg: u8) -> Result<RuntimeVal> {
        let RuntimeVal::Obj(handle) = self.read_unchecked(target_reg) else {
            bail!("GetList target expected Obj");
        };
        let RuntimeVal::Int(index) = self.read_unchecked(key_reg) else {
            bail!("GetList key must be Int");
        };
        let Some(HeapValue::List(list)) = self.state.heap.get(*handle) else {
            bail!("GetList target object changed while reading list");
        };
        let index = if *index < 0 {
            let index = list.len() as i64 + *index;
            if index < 0 {
                return Ok(RuntimeVal::Nil);
            }
            index as usize
        } else {
            *index as usize
        };
        match self.get_typed_list_element(list, index) {
            Some(value) => Ok(value),
            // A long string element: read it again where allocation is allowed.
            None => Ok(self.get_typed_list_element_allocating(*handle, index)),
        }
    }

    #[inline(always)]
    pub(in crate::vm::exec) fn get_string_int_map_index(
        &mut self,
        target_reg: u8,
        suffix_reg: u8,
        prefix: &str,
        index_key_metrics: Option<&mut [u64; VM_INDEX_KEY_METRIC_COUNT]>,
    ) -> Result<RuntimeVal> {
        let RuntimeVal::Obj(handle) = self.read_unchecked(target_reg) else {
            bail!("GetIndexStrI target expected Obj");
        };
        let RuntimeVal::Int(suffix) = self.read_unchecked(suffix_reg) else {
            bail!("GetIndexStrI suffix must be Int");
        };
        self.get_string_int_map_handle(*handle, prefix, *suffix, index_key_metrics)
    }

    #[inline(always)]
    fn get_string_int_map_handle(
        &mut self,
        handle: crate::val::HeapRef,
        prefix: &str,
        suffix: i64,
        mut index_key_metrics: Option<&mut [u64; VM_INDEX_KEY_METRIC_COUNT]>,
    ) -> Result<RuntimeVal> {
        record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::DynamicRegisterKey);
        record_index_key_metric(
            index_key_metrics.as_deref_mut(),
            VmIndexKeyMetric::DynamicShortStringKey,
        );
        record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::DirectStringKey);
        match self.state.heap.get(handle) {
            Some(HeapValue::Map(map)) => with_string_int_key(prefix, suffix, |key| {
                if let Some(value) = get_string_map_direct(map, key) {
                    record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::TypedMapDirect);
                    return Ok(value);
                }
                record_index_key_metric(index_key_metrics, VmIndexKeyMetric::GenericMapLookup);
                Ok(map.get_str(key).unwrap_or(RuntimeVal::Nil))
            })?,
            Some(other) => bail!(
                "GetIndexStrI target object changed while indexing: {:?}",
                HeapValue::type_name(other)
            ),
            None => bail!("heap object {} out of bounds", handle.index()),
        }
    }

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
        self.get_typed_list_element(list, index)
    }

    #[inline(always)]
    pub(in crate::vm::exec) fn try_get_known_int_list_index(&self, target_reg: u8, key_reg: u8) -> Option<RuntimeVal> {
        let RuntimeVal::Obj(handle) = self.read_unchecked(target_reg) else {
            return None;
        };
        let RuntimeVal::Int(index) = self.read_unchecked(key_reg) else {
            return None;
        };
        let Some(HeapValue::List(TypedList::Int(values))) = self.state.heap.get(*handle) else {
            return None;
        };
        let index = if *index < 0 {
            let index = values.len() as i64 + *index;
            if index < 0 {
                return Some(RuntimeVal::Nil);
            }
            index as usize
        } else {
            *index as usize
        };
        Some(
            values
                .get(index)
                .copied()
                .map(RuntimeVal::Int)
                .unwrap_or(RuntimeVal::Nil),
        )
    }

    #[inline(always)]
    pub(in crate::vm::exec) fn read_known_int_list_index(&self, target_reg: u8, key_reg: u8) -> Result<i64> {
        let RuntimeVal::Obj(handle) = self.read_unchecked(target_reg) else {
            bail!("AddListInt/SubListInt target expected Obj");
        };
        let RuntimeVal::Int(index) = self.read_unchecked(key_reg) else {
            bail!("AddListInt/SubListInt key must be Int");
        };
        let Some(HeapValue::List(TypedList::Int(values))) = self.state.heap.get(*handle) else {
            bail!("AddListInt/SubListInt target object changed while reading int list");
        };
        let index = if *index < 0 {
            let index = values.len() as i64 + *index;
            if index < 0 {
                bail!("AddListInt/SubListInt list index out of bounds");
            }
            index as usize
        } else {
            *index as usize
        };
        values
            .get(index)
            .copied()
            .ok_or_else(|| anyhow!("AddListInt/SubListInt list index out of bounds"))
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
        // A List key is a materialized range (`s[1..3]` compiles to
        // `NewRange` + `GetIndex`): slice from its first/last elements. An
        // empty range (`s[3..2]`) slices to the empty prefix. (Formerly
        // gated on `len <= 3`, which broke every slice spanning more than
        // three elements — `s[8..20]` errored out.)
        if let RuntimeVal::Obj(h) = self.read_unchecked(key_reg)
            && let Some(HeapValue::List(list)) = self.state.heap.get(*h)
        {
            // A materialized range: its elements are the integers `NewRange`
            // produced, so this never declines. `unwrap_or_default` rather than
            // an expect because an empty answer is already handled below.
            let items = list.collect_owned().unwrap_or_default();
            if items.is_empty() {
                return self.get_index_slice(target_reg, 0, Some(0), None);
            }
            let start = match &items[0] {
                RuntimeVal::Int(i) => *i,
                _ => 0i64,
            };
            let last = items
                .last()
                .and_then(|v| if let RuntimeVal::Int(i) = v { Some(*i) } else { None });
            return self.get_index_slice(target_reg, start, last.map(|i| i + 1), None);
        }
        match self.read_unchecked(target_reg) {
            RuntimeVal::ShortStr(value) => {
                let value = *value;
                let idx_val = self.read_unchecked(key_reg);
                let idx = match idx_val {
                    RuntimeVal::Int(n) => *n,
                    _ => bail!("a string index must be Int"),
                };
                self.index_string_at(value.as_str(), idx)
            }
            RuntimeVal::Obj(handle) => {
                let handle = *handle;
                self.get_heap_index(pc, handle, key_reg, known_string_key, index_fact, index_key_metrics)
            }
            other => bail!("{} is not indexable", self.value_type_name(other)),
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
                    // Dynamic key from register: avoid RuntimeMapKey construction
                    return self.get_map_index_fast(handle, key_reg, index_key_metrics);
                }
            }
            // For list with known type, skip the slow path too
            if fact.target_kind == PerfIndexTargetKind::List {
                let key_val = self.read_unchecked(key_reg);
                if let RuntimeVal::Int(n) = key_val
                    && let Some(HeapValue::List(list)) = self.state.heap.get(handle)
                {
                    let index = if *n < 0 {
                        let index = list.len() as i64 + *n;
                        if index < 0 {
                            return Ok(RuntimeVal::Nil);
                        }
                        index as usize
                    } else {
                        *n as usize
                    };
                    if let Some(value) = self.get_typed_list_element(list, index) {
                        return Ok(value);
                    }
                    return Ok(self.get_typed_list_element_allocating(handle, index));
                }
            }
            if fact.target_kind == PerfIndexTargetKind::String {
                let key_val = self.read_unchecked(key_reg);
                if let RuntimeVal::Int(n) = key_val
                    && let Some(HeapValue::String(value)) = self.state.heap.get(handle)
                {
                    return self.index_string_at(value, *n);
                }
            }
        }

        self.get_heap_index_slow_path(pc, handle, key_reg, known_string_key, index_fact, index_key_metrics)
    }

    /// Read a value from a typed list by index, without allocating.
    ///
    /// `None` means *this path cannot answer* — not that the element is
    /// missing. A `TypedList::String` element longer than a `ShortStr` needs a
    /// heap allocation, and this runs behind `&self` on the index fast path.
    /// Out of bounds is `Some(Nil)`, which is an answer.
    ///
    /// Returning `Nil` for the too-long case, which is what this used to do,
    /// made `xs[0]` answer nil for an element that was plainly there — while
    /// `xs.first()`, which allocates, answered correctly. Same list, two
    /// answers, and only for strings over seven bytes.
    #[inline(always)]
    fn get_typed_list_element(&self, list: &TypedList, index: usize) -> Option<RuntimeVal> {
        Some(match list {
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
                Some(value) => RuntimeVal::ShortStr(ShortStr::new(value)?),
                None => RuntimeVal::Nil,
            },
        })
    }

    /// One element of a window, by its position *within the window*.
    ///
    /// Negative indices count from the window's end, as they do for a list.
    /// Out of range is nil.
    pub(in crate::vm::exec) fn slice_element(&mut self, handle: crate::val::HeapRef, index: i64) -> RuntimeVal {
        let Some(HeapValue::Slice(slice)) = self.state.heap.get(handle) else {
            return RuntimeVal::Nil;
        };
        let (source, start, recorded_len) = (slice.source, slice.start, slice.len);
        // A negative index counts back from the window's end, and where that
        // end *is* depends on whether the source shrank — so only this case
        // pays for the extra look at the source. A non-negative index does not
        // need to know: past the source, the element read below answers nil on
        // its own, which is the same answer clamping would give.
        let index = if index < 0 {
            slice.live_len(&self.state.heap) as i64 + index
        } else {
            index
        };
        if index < 0 || index as usize >= recorded_len {
            return RuntimeVal::Nil;
        }
        let RuntimeVal::Obj(source) = source else {
            return RuntimeVal::Nil;
        };
        self.get_typed_list_element_allocating(source, start + index as usize)
    }

    /// One byte of a `Bytes`, as an `Int`.
    ///
    /// Same index rule as every other sequence: a negative counts from the end,
    /// outside is nil. `Bytes` was not indexable at all until it had this —
    /// `b[0]` answered "index target object is not indexable".
    pub(in crate::vm::exec) fn byte_element(&mut self, handle: crate::val::HeapRef, index: i64) -> RuntimeVal {
        let Some(HeapValue::Bytes(bytes)) = self.state.heap.get(handle) else {
            return RuntimeVal::Nil;
        };
        let index = if index < 0 { bytes.len() as i64 + index } else { index };
        if index < 0 {
            return RuntimeVal::Nil;
        }
        bytes
            .get(index as usize)
            .map_or(RuntimeVal::Nil, |byte| RuntimeVal::Int(*byte as i64))
    }

    /// The same read, allowed to allocate. Used where the fast path declines.
    fn get_typed_list_element_allocating(&mut self, handle: crate::val::HeapRef, index: usize) -> RuntimeVal {
        let Some(HeapValue::List(list)) = self.state.heap.get(handle) else {
            return RuntimeVal::Nil;
        };
        if let Some(value) = self.get_typed_list_element(list, index) {
            return value;
        }
        // Only a long `TypedList::String` element reaches here.
        let Some(HeapValue::List(TypedList::String(values))) = self.state.heap.get(handle) else {
            return RuntimeVal::Nil;
        };
        let Some(text) = values.get(index).cloned() else {
            return RuntimeVal::Nil;
        };
        RuntimeVal::Obj(self.alloc_heap_value(HeapValue::String(text)))
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
        record_dynamic_index_key_metric(index_key_metrics.as_deref_mut(), key_val);
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
                    Some(other) => bail!(
                        "GetIndex target object changed while indexing: {:?}",
                        HeapValue::type_name(other)
                    ),
                    None => bail!("heap object {} out of bounds", handle.index()),
                }
            }
            RuntimeVal::Int(n) => {
                record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::RuntimeMapKey);
                let key = RuntimeMapKey::Int(*n);
                match self.state.heap.get(handle) {
                    Some(HeapValue::Map(map)) => Ok(map.get(&key).unwrap_or(RuntimeVal::Nil)),
                    Some(other) => bail!(
                        "GetIndex target object changed while indexing: {:?}",
                        HeapValue::type_name(other)
                    ),
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
                record_index_key_metric(index_key_metrics, VmIndexKeyMetric::RuntimeMapKey);
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
            IndexTargetKind::Slice => {
                let RuntimeVal::Int(index) = *self.read(key_reg)? else {
                    bail!("slice index must be Int");
                };
                Ok(self.slice_element(handle, index))
            }
            IndexTargetKind::Bytes => {
                let RuntimeVal::Int(index) = *self.read(key_reg)? else {
                    bail!("bytes index must be Int");
                };
                Ok(self.byte_element(handle, index))
            }
            IndexTargetKind::List => {
                if let Some(pos) = self.negative_list_index(handle, key_reg) {
                    let orig_val = *self.read(key_reg)?;
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
                let Some(key_str) = known_string_key else {
                    // A key in a *register* is the ordinary way to look
                    // something up (`counts.get(word)`), and it took the long
                    // way round: build a `RuntimeMapKey` — an `Arc` clone for a
                    // heap string — and then hand it to the generic lookup,
                    // which for a typed map immediately asks it for the `&str`
                    // it started from. `get_map_index_fast` is that same
                    // question answered once, and it was reachable only when
                    // the *target* had been proven a map at compile time. A
                    // map behind a parameter has no such proof — which is
                    // exactly where a lookup keyed by a variable lives.
                    return self.get_map_index_fast(handle, key_reg, index_key_metrics);
                };
                record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::KnownStringKey);
                record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::RuntimeMapKey);
                let key = runtime_map_key_from_str(key_str);
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
                        record_dynamic_index_key_metric(index_key_metrics.as_deref_mut(), self.read(key_reg)?);
                        record_index_key_metric(index_key_metrics, VmIndexKeyMetric::ObjectKey);
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
            let orig_val = *self.read(key_reg)?;
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
                    RuntimeVal::Int(n) => *n,
                    _ => bail!("a string index must be Int"),
                };
                self.index_string_at(value, idx)
            }
            other => bail!(
                "GetIndex target object changed while indexing: {:?}",
                HeapValue::type_name(other)
            ),
        }
    }
}

#[inline(always)]
fn get_string_map_direct(map: &TypedMap, key: &str) -> Option<RuntimeVal> {
    match map {
        TypedMap::Mixed(values) => {
            if values.is_empty() {
                Some(RuntimeVal::Nil)
            } else {
                None
            }
        }
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

#[inline(always)]
pub(in crate::vm::exec) fn with_string_int_key<R>(prefix: &str, suffix: i64, f: impl FnOnce(&str) -> R) -> Result<R> {
    let Some(prefix) = ShortStr::new(prefix) else {
        let key = format!("{prefix}{suffix}");
        return Ok(f(&key));
    };
    Ok(match prefix.concat_int(suffix) {
        ShortStrOrStr::Short(key) => f(key.as_str()),
        ShortStrOrStr::Str(key) => f(&key),
    })
}

#[cfg(test)]
mod tests {
    use super::get_string_map_direct;
    use crate::val::{RuntimeMapKey, RuntimeVal, ShortStr, TypedMap};

    #[test]
    fn direct_string_map_lookup_returns_nil_for_empty_mixed_map() {
        let map = TypedMap::Mixed(crate::util::value_map::value_map_new());

        assert_eq!(get_string_map_direct(&map, "missing"), Some(RuntimeVal::Nil));
    }

    #[test]
    fn direct_string_map_lookup_keeps_non_empty_mixed_map_on_generic_path() {
        let key = RuntimeMapKey::ShortStr(ShortStr::new("present").expect("short key"));
        let map = TypedMap::Mixed(crate::util::value_map::value_map_from_iter([(key, RuntimeVal::Int(1))]));

        assert_eq!(get_string_map_direct(&map, "missing"), None);
        assert_eq!(get_string_map_direct(&map, "present"), None);
    }
}
