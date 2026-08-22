use alloc::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::val::{HeapRef, HeapValue, RuntimeMapKey, RuntimeVal, TypedMap};
use crate::vm::analysis::{
    PerfIndexFact, PerfIndexTargetKind, PerfValueKind, VM_INDEX_KEY_METRIC_COUNT, VmIndexKeyMetric,
};

use super::{
    Executor, record_dynamic_index_key_metric, record_index_key_metric, runtime_map_key_from_str, set_list_value,
    with_string_int_key,
};

/// A small, stack-allocated key representation that avoids String allocation
/// for ShortStr keys (which are ≤7 bytes).
enum SmallKey {
    /// No small-string key available; fall back to RuntimeMapKey.
    None,
    /// Small string key stored inline (≤7 bytes).
    Short { len: u8, data: [u8; 7] },
}

impl SmallKey {
    #[inline]
    fn from_short_str(s: &crate::val::ShortStr) -> Self {
        let src = s.as_str().as_bytes();
        let mut data = [0u8; 7];
        data[..src.len()].copy_from_slice(src);
        SmallKey::Short {
            len: src.len() as u8,
            data,
        }
    }

    #[inline]
    fn as_str(&self) -> Option<&str> {
        match self {
            SmallKey::None => None,
            SmallKey::Short { len, data } => core::str::from_utf8(&data[..*len as usize]).ok(),
        }
    }
}

/// A string key on its way into a map.
///
/// The two carry the same text and differ in what storing it costs. A typed
/// string carrier keys by `Arc<str>`, so inserting a *new* key had to allocate
/// one — and a constant key already is one, sitting in the function's const
/// pool. Passing the pooled `Arc` makes the insert a refcount bump, and the
/// map's later death a decrement rather than a free.
///
/// One parameter rather than a `&str` plus an optional `Arc` beside it: the
/// two would have to agree, and nothing would check that they did.
enum KeyText<'a> {
    /// Text the caller only borrows — a key read out of a register.
    Borrowed(&'a str),
    /// The pooled constant.
    Shared(&'a Arc<str>),
}

impl KeyText<'_> {
    fn as_str(&self) -> &str {
        match self {
            Self::Borrowed(text) => text,
            Self::Shared(text) => text,
        }
    }

    fn to_arc(&self) -> Arc<str> {
        match self {
            Self::Borrowed(text) => Arc::<str>::from(*text),
            Self::Shared(text) => Arc::clone(text),
        }
    }
}

impl Executor {
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub(in crate::vm::exec) fn set_index(
        &mut self,
        pc: usize,
        target_reg: u8,
        key_reg: u8,
        value_reg: u8,
        move_key: bool,
        move_value: bool,
        known_string_key: Option<&Arc<str>>,
        index_fact: Option<PerfIndexFact>,
        mut index_key_metrics: Option<&mut [u64; VM_INDEX_KEY_METRIC_COUNT]>,
    ) -> Result<()> {
        let handle = {
            let target = self.read(target_reg)?;
            let RuntimeVal::Obj(handle) = target else {
                bail!("{} cannot be indexed for assignment", self.value_type_name(target));
            };
            *handle
        };
        let moved_key = if move_key && known_string_key.is_none() {
            Some(self.take(key_reg)?)
        } else {
            None
        };
        let value = if move_value {
            self.take(value_reg)?
        } else {
            *self.read(value_reg)?
        };
        let has_static_fact = index_fact.is_some();
        let index_fact = match index_fact {
            Some(fact) => Some(fact),
            None => self
                .cached_or_observed_index_cache(pc, handle, known_string_key)?
                .map(|cache| cache.fact),
        };

        match index_fact.map(|fact| fact.target_kind) {
            Some(PerfIndexTargetKind::List) => {
                return self.set_list_index_handle(
                    handle,
                    key_reg,
                    moved_key,
                    value,
                    index_fact.map(|fact| fact.value_kind),
                    has_static_fact,
                );
            }
            Some(PerfIndexTargetKind::Map) => {
                return self.set_map_index_handle(
                    handle,
                    key_reg,
                    moved_key,
                    value,
                    known_string_key,
                    index_fact.map(|fact| fact.value_kind),
                    has_static_fact,
                    index_key_metrics,
                );
            }
            Some(PerfIndexTargetKind::Object) => {
                return self.set_object_index_handle(
                    handle,
                    key_reg,
                    moved_key,
                    value,
                    known_string_key,
                    has_static_fact,
                    index_key_metrics,
                );
            }
            Some(PerfIndexTargetKind::String | PerfIndexTargetKind::Unknown) | None => {}
        }

        let key = match known_string_key {
            Some(key_str) => {
                record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::KnownStringKey);
                record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::RuntimeMapKey);
                runtime_map_key_from_str(key_str)
            }
            None => {
                match moved_key.as_ref() {
                    Some(key) => record_dynamic_index_key_metric(index_key_metrics.as_deref_mut(), key),
                    None => record_dynamic_index_key_metric(index_key_metrics.as_deref_mut(), self.read(key_reg)?),
                }
                record_index_key_metric(index_key_metrics, VmIndexKeyMetric::RuntimeMapKey);
                self.map_key_from_register_or_value(key_reg, moved_key)?
            }
        };

        let key = self.resolve_negative_list_key(handle, key)?;

        if let Some(done) = self.try_set_string_list(handle, &key, value)? {
            self.maybe_bump_shape(handle, has_static_fact);
            return Ok(done);
        }

        match self
            .state
            .heap
            .get_mut(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::List(list) => {
                let RuntimeMapKey::Int(index) = key else {
                    bail!("SetIndex list key must be Int");
                };
                let index = usize::try_from(index).map_err(|_| anyhow!("list index {index} out of bounds"))?;
                set_list_value(list, index, value)
            }
            HeapValue::Map(map) => {
                map.set(key, value);
                Ok::<(), anyhow::Error>(())
            }
            other => bail!(
                "SetIndex target object changed while writing: {:?}",
                HeapValue::type_name(other)
            ),
        }?;
        self.maybe_bump_shape(handle, has_static_fact);
        Ok(())
    }

    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub(in crate::vm::exec) fn set_string_int_map_index(
        &mut self,
        target_reg: u8,
        suffix_reg: u8,
        value_reg: u8,
        prefix: &str,
        move_value: bool,
        known_value_kind: Option<PerfValueKind>,
        mut index_key_metrics: Option<&mut [u64; VM_INDEX_KEY_METRIC_COUNT]>,
    ) -> Result<()> {
        let RuntimeVal::Obj(handle) = self.read(target_reg)? else {
            bail!("SetIndexStrI target expected Obj");
        };
        let handle = *handle;
        let RuntimeVal::Int(suffix) = self.read(suffix_reg)? else {
            bail!("SetIndexStrI suffix must be Int");
        };
        let suffix = *suffix;
        let value = if move_value {
            self.take(value_reg)?
        } else {
            *self.read(value_reg)?
        };
        record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::DynamicRegisterKey);
        record_index_key_metric(
            index_key_metrics.as_deref_mut(),
            VmIndexKeyMetric::DynamicShortStringKey,
        );
        record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::DirectStringKey);
        with_string_int_key(prefix, suffix, |key| {
            if self.try_set_typed_string_map(handle, KeyText::Borrowed(key), &value, known_value_kind)? {
                record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::TypedMapDirect);
                return Ok(());
            }
            record_index_key_metric(index_key_metrics, VmIndexKeyMetric::GenericMapLookup);
            let key = runtime_map_key_from_str(key);
            match self
                .state
                .heap
                .get_mut(handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::Map(map) => {
                    map.set(key, value);
                    Ok(())
                }
                other => bail!(
                    "SetIndexStrI target object changed while writing map: {:?}",
                    HeapValue::type_name(other)
                ),
            }
        })??;
        self.maybe_bump_shape(handle, true);
        Ok(())
    }

    #[inline(always)]
    fn maybe_bump_shape(&mut self, handle: HeapRef, has_static_fact: bool) {
        // When we have a static index_fact, the following GetIndex operations
        // will use the fact directly and don't rely on shape generation for
        // inline cache validation. Skip the bump to avoid invalidating caches
        // in hot loops (e.g., histogram workloads with repeated map.set + map.get).
        if !has_static_fact {
            self.state.heap.bump_shape_generation(handle);
        }
    }

    /// A write index resolved against the list's length: **negative counts from
    /// the end**, exactly as the read does.
    ///
    /// `xs[-1]` read the last element and `xs[-1] = 9` answered "list index must
    /// be non-negative" — the same expression, one direction. Out of range
    /// after resolving is still an error: writing past the end is not something
    /// you can mean, unlike reading past it (which is nil).
    ///
    /// The length lookup happens only for a negative index, so the ordinary
    /// write pays one predictable compare.
    ///
    /// **Out of range is reported here, naming the index as written.** The
    /// resolved value is what the rest of the write path carries, so raising
    /// further down could only say `-6` for `xs.set(-9, v)` on a three-element
    /// list — a number the program never wrote, and one the native build
    /// (which still has the original) did not print either. Both ends of the
    /// range now say `list index N out of bounds` with the same `N`.
    #[inline]
    pub(super) fn negative_list_index_from_end(&self, handle: HeapRef, index: i64) -> Result<i64> {
        if index >= 0 {
            return Ok(index);
        }
        match self.state.heap.get(handle) {
            Some(HeapValue::List(list)) => {
                let resolved = index + list.len() as i64;
                if resolved < 0 {
                    bail!("list index {index} out of bounds");
                }
                Ok(resolved)
            }
            // Not a list: the caller's own dispatch reports what it is.
            _ => Ok(index),
        }
    }

    /// [`Self::negative_list_index_from_end`] for the dynamic path, where the
    /// key has already been built and the target may not be a list at all.
    #[inline]
    fn resolve_negative_list_key(&self, handle: HeapRef, key: RuntimeMapKey) -> Result<RuntimeMapKey> {
        match key {
            RuntimeMapKey::Int(index) if index < 0 => {
                Ok(RuntimeMapKey::Int(self.negative_list_index_from_end(handle, index)?))
            }
            other => Ok(other),
        }
    }

    pub(super) fn set_list_index_handle(
        &mut self,
        handle: HeapRef,
        key_reg: u8,
        moved_key: Option<RuntimeVal>,
        value: RuntimeVal,
        known_value_kind: Option<PerfValueKind>,
        has_static_fact: bool,
    ) -> Result<()> {
        let index =
            self.negative_list_index_from_end(handle, self.int_key_from_register_or_value(key_reg, moved_key)?)?;
        let key = RuntimeMapKey::Int(index);
        if matches!(
            self.state.heap.get(handle),
            Some(HeapValue::List(crate::val::TypedList::String(_)))
        ) && let Some(done) = self.try_set_string_list(handle, &key, value)?
        {
            self.maybe_bump_shape(handle, has_static_fact);
            return Ok(done);
        }
        let index = usize::try_from(index).map_err(|_| anyhow!("list index {index} out of bounds"))?;
        if self.try_set_typed_list_index(handle, index, &value, known_value_kind)? {
            return Ok(());
        }
        match self
            .state
            .heap
            .get_mut(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::List(list) => set_list_value(list, index, value),
            other => bail!(
                "SetIndex target object changed while writing list: {:?}",
                HeapValue::type_name(other)
            ),
        }?;
        self.maybe_bump_shape(handle, has_static_fact);
        Ok(())
    }

    #[allow(dead_code)]
    pub(super) fn set_list_index_handle_no_fact(
        &mut self,
        handle: HeapRef,
        key_reg: u8,
        moved_key: Option<RuntimeVal>,
        value: RuntimeVal,
        known_value_kind: Option<PerfValueKind>,
    ) -> Result<()> {
        self.set_list_index_handle(handle, key_reg, moved_key, value, known_value_kind, false)
    }

    pub(super) fn try_set_typed_list_index(
        &mut self,
        handle: HeapRef,
        index: usize,
        value: &RuntimeVal,
        known_value_kind: Option<PerfValueKind>,
    ) -> Result<bool> {
        match (
            known_value_kind.unwrap_or_default(),
            self.state
                .heap
                .get_mut(handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?,
            value,
        ) {
            (PerfValueKind::Int, HeapValue::List(crate::val::TypedList::Int(values)), RuntimeVal::Int(value)) => {
                let Some(slot) = values.get_mut(index) else {
                    bail!("list index {} out of bounds", index);
                };
                *slot = *value;
                Ok(true)
            }
            (PerfValueKind::Float, HeapValue::List(crate::val::TypedList::Float(values)), RuntimeVal::Float(value)) => {
                let Some(slot) = values.get_mut(index) else {
                    bail!("list index {} out of bounds", index);
                };
                *slot = *value;
                Ok(true)
            }
            (PerfValueKind::Bool, HeapValue::List(crate::val::TypedList::Bool(values)), RuntimeVal::Bool(value)) => {
                let Some(slot) = values.get_mut(index) else {
                    bail!("list index {} out of bounds", index);
                };
                *slot = *value;
                Ok(true)
            }
            (PerfValueKind::Unknown, _, _) | (_, HeapValue::List(_), _) => Ok(false),
            (_, other, _) => bail!(
                "SetIndex target object changed while writing list: {:?}",
                HeapValue::type_name(other)
            ),
        }
    }

    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn set_map_index_handle(
        &mut self,
        handle: HeapRef,
        key_reg: u8,
        moved_key: Option<RuntimeVal>,
        value: RuntimeVal,
        known_string_key: Option<&Arc<str>>,
        known_value_kind: Option<PerfValueKind>,
        has_static_fact: bool,
        mut index_key_metrics: Option<&mut [u64; VM_INDEX_KEY_METRIC_COUNT]>,
    ) -> Result<()> {
        // Fast path: when key is a known string, use the &str-based setter directly,
        // avoiding RuntimeMapKey construction entirely.
        if let Some(key_str) = known_string_key {
            record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::KnownStringKey);
            record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::DirectStringKey);
            if self.try_set_typed_string_map(handle, KeyText::Shared(key_str), &value, known_value_kind)? {
                record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::TypedMapDirect);
                return Ok(());
            }
        }

        // Fast path: when no known_string_key but key is a ShortStr in register or moved_key,
        // use the SmallKey to avoid String allocation from ShortStr::as_str().to_owned().
        if known_string_key.is_none() {
            let small_key: SmallKey = match &moved_key {
                Some(RuntimeVal::ShortStr(s)) => SmallKey::from_short_str(s),
                Some(RuntimeVal::Int(_)) => SmallKey::None, // Int keys handled via RuntimeMapKey
                _ => match self.read_unchecked(key_reg) {
                    RuntimeVal::ShortStr(s) => SmallKey::from_short_str(s),
                    _ => SmallKey::None,
                },
            };
            if let Some(key_str) = small_key.as_str() {
                record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::DynamicRegisterKey);
                record_index_key_metric(
                    index_key_metrics.as_deref_mut(),
                    VmIndexKeyMetric::DynamicShortStringKey,
                );
                record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::DirectStringKey);
                if self.try_set_typed_string_map(handle, KeyText::Borrowed(key_str), &value, known_value_kind)? {
                    record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::TypedMapDirect);
                    return Ok(());
                }
            }
        }

        let key = match known_string_key {
            Some(key_str) => {
                record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::KnownStringKey);
                record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::RuntimeMapKey);
                runtime_map_key_from_str(key_str)
            }
            None => {
                match moved_key.as_ref() {
                    Some(key) => record_dynamic_index_key_metric(index_key_metrics.as_deref_mut(), key),
                    None => record_dynamic_index_key_metric(index_key_metrics.as_deref_mut(), self.read(key_reg)?),
                }
                record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::RuntimeMapKey);
                self.map_key_from_register_or_value(key_reg, moved_key)?
            }
        };
        if known_string_key.is_none() && self.try_set_typed_string_map_index(handle, &key, &value, known_value_kind)? {
            record_index_key_metric(index_key_metrics.as_deref_mut(), VmIndexKeyMetric::TypedMapDirect);
            return Ok(());
        }
        record_index_key_metric(index_key_metrics, VmIndexKeyMetric::GenericMapLookup);
        match self
            .state
            .heap
            .get_mut(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::Map(map) => {
                map.set(key, value);
                Ok::<(), anyhow::Error>(())
            }
            other => bail!(
                "SetIndex target object changed while writing map: {:?}",
                HeapValue::type_name(other)
            ),
        }?;
        self.maybe_bump_shape(handle, has_static_fact);
        Ok(())
    }

    #[inline(always)]
    #[allow(dead_code)]
    pub(super) fn set_map_index_handle_no_fact(
        &mut self,
        handle: HeapRef,
        key_reg: u8,
        moved_key: Option<RuntimeVal>,
        value: RuntimeVal,
        known_string_key: Option<&Arc<str>>,
        known_value_kind: Option<PerfValueKind>,
    ) -> Result<()> {
        self.set_map_index_handle(
            handle,
            key_reg,
            moved_key,
            value,
            known_string_key,
            known_value_kind,
            false,
            None,
        )
    }

    #[inline(always)]
    fn try_set_typed_string_map_index(
        &mut self,
        handle: HeapRef,
        key: &RuntimeMapKey,
        value: &RuntimeVal,
        known_value_kind: Option<PerfValueKind>,
    ) -> Result<bool> {
        let Some(key_str) = key.as_str() else {
            return Ok(false);
        };
        match (
            known_value_kind.unwrap_or_default(),
            self.state
                .heap
                .get_mut(handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?,
            value,
        ) {
            (PerfValueKind::Int, HeapValue::Map(TypedMap::StringInt(values)), RuntimeVal::Int(iv)) => {
                if let Some(existing) = values.get_mut(key_str) {
                    *existing = *iv;
                } else {
                    values.insert(Arc::<str>::from(key_str), *iv);
                }
                Ok(true)
            }
            (PerfValueKind::Float, HeapValue::Map(TypedMap::StringFloat(values)), RuntimeVal::Float(fv)) => {
                if let Some(existing) = values.get_mut(key_str) {
                    *existing = *fv;
                } else {
                    values.insert(Arc::<str>::from(key_str), *fv);
                }
                Ok(true)
            }
            (PerfValueKind::Bool, HeapValue::Map(TypedMap::StringBool(values)), RuntimeVal::Bool(bv)) => {
                if let Some(existing) = values.get_mut(key_str) {
                    *existing = *bv;
                } else {
                    values.insert(Arc::<str>::from(key_str), *bv);
                }
                Ok(true)
            }
            (PerfValueKind::Unknown, HeapValue::Map(TypedMap::StringInt(values)), RuntimeVal::Int(iv)) => {
                if let Some(existing) = values.get_mut(key_str) {
                    *existing = *iv;
                } else {
                    values.insert(Arc::<str>::from(key_str), *iv);
                }
                Ok(true)
            }
            (PerfValueKind::Unknown, HeapValue::Map(TypedMap::StringFloat(values)), RuntimeVal::Float(fv)) => {
                if let Some(existing) = values.get_mut(key_str) {
                    *existing = *fv;
                } else {
                    values.insert(Arc::<str>::from(key_str), *fv);
                }
                Ok(true)
            }
            (PerfValueKind::Unknown, HeapValue::Map(TypedMap::StringBool(values)), RuntimeVal::Bool(bv)) => {
                if let Some(existing) = values.get_mut(key_str) {
                    *existing = *bv;
                } else {
                    values.insert(Arc::<str>::from(key_str), *bv);
                }
                Ok(true)
            }
            (PerfValueKind::Unknown, _, _) | (_, HeapValue::Map(_), _) => Ok(false),
            (_, other, _) => bail!(
                "SetIndex target object changed while writing map: {:?}",
                HeapValue::type_name(other)
            ),
        }
    }

    /// Like try_set_typed_string_map_index but takes a &str key directly,
    /// avoiding the RuntimeMapKey construction round-trip.
    #[inline(always)]
    fn try_set_typed_string_map(
        &mut self,
        handle: HeapRef,
        key: KeyText<'_>,
        value: &RuntimeVal,
        known_value_kind: Option<PerfValueKind>,
    ) -> Result<bool> {
        let key_str = key.as_str();
        match (
            known_value_kind.unwrap_or_default(),
            self.state
                .heap
                .get_mut(handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?,
            value,
        ) {
            (PerfValueKind::Int, HeapValue::Map(TypedMap::StringInt(values)), RuntimeVal::Int(iv)) => {
                if let Some(existing) = values.get_mut(key_str) {
                    *existing = *iv;
                } else {
                    values.insert(key.to_arc(), *iv);
                }
                Ok(true)
            }
            (PerfValueKind::Float, HeapValue::Map(TypedMap::StringFloat(values)), RuntimeVal::Float(fv)) => {
                if let Some(existing) = values.get_mut(key_str) {
                    *existing = *fv;
                } else {
                    values.insert(key.to_arc(), *fv);
                }
                Ok(true)
            }
            (PerfValueKind::Bool, HeapValue::Map(TypedMap::StringBool(values)), RuntimeVal::Bool(bv)) => {
                if let Some(existing) = values.get_mut(key_str) {
                    *existing = *bv;
                } else {
                    values.insert(key.to_arc(), *bv);
                }
                Ok(true)
            }
            (PerfValueKind::Unknown, HeapValue::Map(TypedMap::StringInt(values)), RuntimeVal::Int(iv)) => {
                if let Some(existing) = values.get_mut(key_str) {
                    *existing = *iv;
                } else {
                    values.insert(key.to_arc(), *iv);
                }
                Ok(true)
            }
            (PerfValueKind::Unknown, HeapValue::Map(TypedMap::StringFloat(values)), RuntimeVal::Float(fv)) => {
                if let Some(existing) = values.get_mut(key_str) {
                    *existing = *fv;
                } else {
                    values.insert(key.to_arc(), *fv);
                }
                Ok(true)
            }
            (PerfValueKind::Unknown, HeapValue::Map(TypedMap::StringBool(values)), RuntimeVal::Bool(bv)) => {
                if let Some(existing) = values.get_mut(key_str) {
                    *existing = *bv;
                } else {
                    values.insert(key.to_arc(), *bv);
                }
                Ok(true)
            }
            (PerfValueKind::Unknown, _, _) | (_, HeapValue::Map(_), _) => Ok(false),
            (_, other, _) => bail!(
                "SetIndex target object changed while writing map: {:?}",
                HeapValue::type_name(other)
            ),
        }
    }
}
