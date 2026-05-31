use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::val::{HeapRef, HeapValue, RuntimeMapKey, RuntimeObject, RuntimeVal, ShortStr, TypedList, TypedMap};

use super::{Executor32, heap_kind, push_list_value, set_list_value};
use crate::vm::{
    IndexInlineCache32,
    analysis::{PerfIndexFact, PerfIndexTargetKind, PerfValueKind},
};

mod index;

#[derive(Clone, Copy, Eq, PartialEq)]
enum IndexTargetKind {
    List,
    Map,
    Object,
    String,
}

enum SliceFromPlan {
    List(TypedList),
    String(Arc<str>),
}

enum ToIterPlan {
    ExistingList(HeapRef),
    StringChars(Vec<Arc<str>>),
    Map(TypedMapIterSnapshot),
}

enum TypedMapIterSnapshot {
    Mixed(Vec<(RuntimeMapKey, RuntimeVal)>),
    StringMixed(Vec<(Arc<str>, RuntimeVal)>),
    StringInt(Vec<(Arc<str>, i64)>),
    StringFloat(Vec<(Arc<str>, f64)>),
    StringBool(Vec<(Arc<str>, bool)>),
}

impl Executor32 {
    pub(super) fn build_int_range(&self, base: u8, inclusive: bool) -> Result<Vec<i64>> {
        let start = self.read_int(base)?;
        let end = self.read_int(base.checked_add(1).ok_or_else(|| anyhow!("range base overflow"))?)?;
        let step = self.read_int(base.checked_add(2).ok_or_else(|| anyhow!("range base overflow"))?)?;
        if step == 0 {
            bail!("Range step cannot be zero");
        }

        let mut out = Vec::new();
        let mut current = start;
        if step > 0 {
            while if inclusive { current <= end } else { current < end } {
                out.push(current);
                current = current
                    .checked_add(step)
                    .ok_or_else(|| anyhow!("Range step overflow"))?;
            }
        } else {
            while if inclusive { current >= end } else { current > end } {
                out.push(current);
                current = current
                    .checked_add(step)
                    .ok_or_else(|| anyhow!("Range step overflow"))?;
            }
        }
        Ok(out)
    }

    pub(super) fn read_map_entries(&self, base: u8, count: u8) -> Result<BTreeMap<RuntimeMapKey, RuntimeVal>> {
        let mut values = BTreeMap::new();
        for entry in 0..count {
            let key_reg = base
                .checked_add(entry.checked_mul(2).expect("map entry register overflow"))
                .ok_or_else(|| anyhow!("map key register overflow"))?;
            let value_reg = key_reg
                .checked_add(1)
                .ok_or_else(|| anyhow!("map value register overflow"))?;
            let key = self.map_key_from_register(key_reg)?;
            let value = self.read(value_reg)?.clone();
            values.insert(key, value);
        }
        Ok(values)
    }

    pub(super) fn take_map_entries(
        &mut self,
        base: u8,
        count: u8,
        move_keys: bool,
        move_values: bool,
    ) -> Result<BTreeMap<RuntimeMapKey, RuntimeVal>> {
        let mut values = BTreeMap::new();
        for entry in 0..count {
            let key_reg = base
                .checked_add(entry.checked_mul(2).expect("map entry register overflow"))
                .ok_or_else(|| anyhow!("map key register overflow"))?;
            let value_reg = key_reg
                .checked_add(1)
                .ok_or_else(|| anyhow!("map value register overflow"))?;
            let moved_key = if move_keys { Some(self.take(key_reg)?) } else { None };
            let key = self.map_key_from_register_or_value(key_reg, moved_key)?;
            let value = if move_values {
                self.take(value_reg)?
            } else {
                self.read(value_reg)?.clone()
            };
            values.insert(key, value);
        }
        Ok(values)
    }

    pub(super) fn read_object_fields(&self, base: u8, count: u8) -> Result<RuntimeObject> {
        let type_name = Arc::<str>::from(self.to_runtime_string(base)?);
        let field_base = base
            .checked_add(1)
            .ok_or_else(|| anyhow!("object field base overflow"))?;
        let mut fields = BTreeMap::new();
        for entry in 0..count {
            let offset = entry
                .checked_mul(2)
                .ok_or_else(|| anyhow!("object field register overflow"))?;
            let key_reg = field_base
                .checked_add(offset)
                .ok_or_else(|| anyhow!("object key register overflow"))?;
            let value_reg = key_reg
                .checked_add(1)
                .ok_or_else(|| anyhow!("object value register overflow"))?;
            fields.insert(
                Arc::<str>::from(self.to_runtime_string(key_reg)?),
                self.read(value_reg)?.clone(),
            );
        }
        Ok(RuntimeObject::new(type_name, fields))
    }

    fn get_index_slice(
        &mut self,
        target_reg: u8,
        start: i64,
        end: Option<i64>,
        _step: Option<i64>,
    ) -> Result<RuntimeVal> {
        match self.read(target_reg)? {
            RuntimeVal::ShortStr(value) => {
                let s: Arc<str> = Arc::<str>::from(value.as_str());
                self.slice_string_general(s, start, end)
            }
            RuntimeVal::Obj(handle) => {
                let handle = *handle;
                match self
                    .state
                    .heap
                    .get(handle)
                    .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
                {
                    HeapValue::String(value) => self.slice_string_general(Arc::clone(value), start, end),
                    HeapValue::List(list) => {
                        let items = list.collect_owned();
                        let end = end.unwrap_or(items.len() as i64);
                        let start = if start < 0 {
                            (items.len() as i64 + start).max(0)
                        } else {
                            start
                        };
                        let end = if end < 0 {
                            (items.len() as i64 + end).max(0)
                        } else {
                            end
                        };
                        let start = start as usize;
                        let end = end as usize;
                        let end = end.min(items.len());
                        let start = start.min(end);
                        let slice: Vec<RuntimeVal> = items[start..end].to_vec();
                        Ok(RuntimeVal::Obj(
                            self.alloc_heap_value(HeapValue::List(TypedList::Mixed(slice))),
                        ))
                    }
                    _ => bail!("Slice target must be string or list"),
                }
            }
            other => bail!("Slice target expected string/list, got {:?}", other.kind()),
        }
    }

    fn slice_string_general(&mut self, value: Arc<str>, start: i64, end: Option<i64>) -> Result<RuntimeVal> {
        let s_len = value.len() as i64;
        let start = if start < 0 {
            (s_len + start).max(0)
        } else {
            start.min(s_len)
        } as usize;
        let end = match end {
            Some(e) => {
                if e < 0 {
                    (s_len + e).max(0)
                } else {
                    e.min(s_len)
                }
            }
            None => s_len,
        } as usize;
        let end = end.min(value.len());
        let start = start.min(end);
        let sliced: &str = &value[start..end];
        if let Some(short) = ShortStr::new(sliced) {
            Ok(RuntimeVal::ShortStr(short))
        } else {
            Ok(RuntimeVal::Obj(
                self.alloc_heap_value(HeapValue::String(Arc::<str>::from(sliced))),
            ))
        }
    }

    fn index_target_kind(&self, handle: HeapRef) -> Result<IndexTargetKind> {
        match self
            .state
            .heap
            .get(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::List(_) => Ok(IndexTargetKind::List),
            HeapValue::Map(_) => Ok(IndexTargetKind::Map),
            HeapValue::Object(_) => Ok(IndexTargetKind::Object),
            HeapValue::String(_) => Ok(IndexTargetKind::String),
            other => bail!("GetIndex target object is not indexable: {:?}", heap_kind(other)),
        }
    }

    pub(super) fn len_value(&self, register: u8) -> Result<usize> {
        let index = self.stack_index(register)?;
        match &self.state.stack[index] {
            RuntimeVal::ShortStr(value) => Ok(string_char_len(value.as_str())),
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::String(value) => Ok(string_char_len(value)),
                HeapValue::List(value) => Ok(value.len()),
                HeapValue::Map(value) => Ok(value.len()),
                other => bail!("Len target object is not sized: {:?}", heap_kind(other)),
            },
            other => bail!("Len target expected string/list/map, got {:?}", other.kind()),
        }
    }

    pub(super) fn contains_value(&self, needle_reg: u8, haystack_reg: u8) -> Result<bool> {
        let (needle_index, haystack_index) = self.stack_bc_indices(needle_reg, haystack_reg)?;
        let needle = &self.state.stack[needle_index];
        match &self.state.stack[haystack_index] {
            RuntimeVal::ShortStr(haystack) => {
                let Some(needle) = self.runtime_value_to_string(&needle)? else {
                    return Ok(false);
                };
                Ok(haystack.as_str().contains(needle.as_ref()))
            }
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::String(haystack) => {
                    let Some(needle) = self.runtime_value_to_string(&needle)? else {
                        return Ok(false);
                    };
                    Ok(haystack.contains(needle.as_ref()))
                }
                HeapValue::List(values) => self.list_contains(values, &needle),
                HeapValue::Map(values) => self.map_contains(values, &needle),
                other => bail!("Contains haystack object is not searchable: {:?}", heap_kind(other)),
            },
            other => bail!("Contains haystack expected string/list/map, got {:?}", other.kind()),
        }
    }

    pub(super) fn slice_from(&mut self, target_reg: u8, start_reg: u8) -> Result<RuntimeVal> {
        let start = usize::try_from(self.read_int(start_reg)?)
            .map_err(|_| anyhow!("SliceFrom start index must be non-negative"))?;
        match self.read(target_reg)?.clone() {
            RuntimeVal::ShortStr(value) => self.slice_string_from(Arc::<str>::from(value.as_str()), start),
            RuntimeVal::Obj(handle) => {
                let plan = match self
                    .state
                    .heap
                    .get(handle)
                    .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
                {
                    HeapValue::List(values) => SliceFromPlan::List(values.slice_from(start)),
                    HeapValue::String(value) => SliceFromPlan::String(Arc::clone(value)),
                    other => bail!("SliceFrom target object is not sliceable: {:?}", heap_kind(other)),
                };
                match plan {
                    SliceFromPlan::List(values) => Ok(RuntimeVal::Obj(self.alloc_heap_value(HeapValue::List(values)))),
                    SliceFromPlan::String(value) => self.slice_string_from(value, start),
                }
            }
            other => bail!("SliceFrom target expected string/list object, got {:?}", other.kind()),
        }
    }

    fn slice_string_from(&mut self, value: Arc<str>, start: usize) -> Result<RuntimeVal> {
        let mut suffix = String::new();
        for ch in value.chars().skip(start) {
            suffix.push(ch);
        }
        Ok(self.runtime_value_from_string(Arc::<str>::from(suffix)))
    }

    pub(super) fn map_rest(&mut self, base: u8, key_count: u8) -> Result<RuntimeVal> {
        let RuntimeVal::Obj(handle) = self.read(base)?.clone() else {
            bail!("MapRest base expected map object");
        };
        let source = match self
            .state
            .heap
            .get(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::Map(map) => map,
            other => bail!("MapRest source object is not a map: {:?}", heap_kind(other)),
        };

        let mut removed_keys = Vec::with_capacity(usize::from(key_count));
        for offset in 0..key_count {
            let key_reg = base
                .checked_add(1)
                .and_then(|reg| reg.checked_add(offset))
                .ok_or_else(|| anyhow!("MapRest key register overflow"))?;
            removed_keys.push(self.map_key_from_register(key_reg)?);
        }
        let map = typed_map_without_keys(source, &removed_keys);
        Ok(RuntimeVal::Obj(self.alloc_heap_value(HeapValue::Map(map))))
    }

    fn list_contains(&self, values: &TypedList, needle: &RuntimeVal) -> Result<bool> {
        Ok(match values {
            TypedList::Mixed(values) => values.iter().any(|value| value == needle),
            TypedList::Int(values) => matches!(needle, RuntimeVal::Int(needle) if values.contains(needle)),
            TypedList::Float(values) => matches!(needle, RuntimeVal::Float(needle) if values.contains(needle)),
            TypedList::Bool(values) => matches!(needle, RuntimeVal::Bool(needle) if values.contains(needle)),
            TypedList::String(values) => {
                let Some(needle) = self.runtime_value_to_string(needle)? else {
                    return Ok(false);
                };
                values.iter().any(|value| value.as_ref() == needle.as_ref())
            }
        })
    }

    fn map_contains(&self, values: &TypedMap, needle: &RuntimeVal) -> Result<bool> {
        Ok(match values {
            TypedMap::Mixed(values) => {
                let key = self.runtime_map_key_from_value(needle)?;
                values.contains_key(&key)
            }
            TypedMap::StringMixed(values) => self.string_map_contains_key(values, needle)?,
            TypedMap::StringInt(values) => self.string_map_contains_key(values, needle)?,
            TypedMap::StringFloat(values) => self.string_map_contains_key(values, needle)?,
            TypedMap::StringBool(values) => self.string_map_contains_key(values, needle)?,
        })
    }

    pub(super) fn to_iter(&mut self, register: u8) -> Result<RuntimeVal> {
        match self.read(register)?.clone() {
            RuntimeVal::ShortStr(value) => {
                let list = string_chars_to_list(value.as_str());
                self.finish_to_iter_plan(ToIterPlan::StringChars(list))
            }
            RuntimeVal::Obj(handle) => {
                let plan = match self
                    .state
                    .heap
                    .get(handle)
                    .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
                {
                    HeapValue::List(_) => ToIterPlan::ExistingList(handle),
                    HeapValue::String(value) => ToIterPlan::StringChars(string_chars_to_list(value)),
                    HeapValue::Map(map) => ToIterPlan::Map(typed_map_iter_snapshot(map)),
                    other => bail!("ToIter target object is not iterable: {:?}", heap_kind(other)),
                };
                self.finish_to_iter_plan(plan)
            }
            other => bail!("ToIter target expected string/list/map, got {:?}", other.kind()),
        }
    }

    fn finish_to_iter_plan(&mut self, plan: ToIterPlan) -> Result<RuntimeVal> {
        match plan {
            ToIterPlan::ExistingList(handle) => Ok(RuntimeVal::Obj(handle)),
            ToIterPlan::StringChars(list) => Ok(RuntimeVal::Obj(
                self.alloc_heap_value(HeapValue::List(TypedList::String(list))),
            )),
            ToIterPlan::Map(snapshot) => self.map_entries_to_iter_list(snapshot),
        }
    }

    fn map_entries_to_iter_list(&mut self, entries: TypedMapIterSnapshot) -> Result<RuntimeVal> {
        let mut pairs = Vec::with_capacity(entries.len());
        match entries {
            TypedMapIterSnapshot::Mixed(entries) => {
                for (key, value) in entries {
                    let key = self.runtime_map_key_to_value(key);
                    self.push_iter_pair(&mut pairs, key, value);
                }
            }
            TypedMapIterSnapshot::StringMixed(entries) => {
                for (key, value) in entries {
                    let key = self.runtime_string_key_to_value(key);
                    self.push_iter_pair(&mut pairs, key, value);
                }
            }
            TypedMapIterSnapshot::StringInt(entries) => {
                for (key, value) in entries {
                    let key = self.runtime_string_key_to_value(key);
                    self.push_iter_pair(&mut pairs, key, RuntimeVal::Int(value));
                }
            }
            TypedMapIterSnapshot::StringFloat(entries) => {
                for (key, value) in entries {
                    let key = self.runtime_string_key_to_value(key);
                    self.push_iter_pair(&mut pairs, key, RuntimeVal::Float(value));
                }
            }
            TypedMapIterSnapshot::StringBool(entries) => {
                for (key, value) in entries {
                    let key = self.runtime_string_key_to_value(key);
                    self.push_iter_pair(&mut pairs, key, RuntimeVal::Bool(value));
                }
            }
        }
        Ok(RuntimeVal::Obj(
            self.alloc_heap_value(HeapValue::List(TypedList::Mixed(pairs))),
        ))
    }

    fn push_iter_pair(&mut self, pairs: &mut Vec<RuntimeVal>, key: RuntimeVal, value: RuntimeVal) {
        let pair = HeapValue::List(TypedList::Mixed(vec![key, value]));
        pairs.push(RuntimeVal::Obj(self.alloc_heap_value(pair)));
    }

    fn runtime_map_key_to_value(&mut self, key: RuntimeMapKey) -> RuntimeVal {
        match key {
            RuntimeMapKey::Nil => RuntimeVal::Nil,
            RuntimeMapKey::Bool(value) => RuntimeVal::Bool(value),
            RuntimeMapKey::Int(value) => RuntimeVal::Int(value),
            RuntimeMapKey::ShortStr(value) => RuntimeVal::ShortStr(value),
            RuntimeMapKey::String(value) => {
                if let Some(short) = ShortStr::new(&value) {
                    RuntimeVal::ShortStr(short)
                } else {
                    RuntimeVal::Obj(self.alloc_heap_value(HeapValue::String(value)))
                }
            }
            RuntimeMapKey::Obj(value) => RuntimeVal::Obj(value),
        }
    }

    fn runtime_string_key_to_value(&mut self, value: Arc<str>) -> RuntimeVal {
        if let Some(short) = ShortStr::new(&value) {
            RuntimeVal::ShortStr(short)
        } else {
            RuntimeVal::Obj(self.alloc_heap_value(HeapValue::String(value)))
        }
    }

    pub(super) fn set_index(
        &mut self,
        pc: usize,
        target_reg: u8,
        key_reg: u8,
        value_reg: u8,
        move_key: bool,
        move_value: bool,
        known_string_key: Option<Arc<str>>,
        index_fact: Option<PerfIndexFact>,
    ) -> Result<()> {
        let handle = {
            let target = self.read(target_reg)?;
            let RuntimeVal::Obj(handle) = target else {
                bail!("SetIndex target expected Obj, got {:?}", target.kind());
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
            self.read(value_reg)?.clone()
        };
        let index_fact = match index_fact {
            Some(fact) => Some(fact),
            None => self
                .cached_or_observed_index_cache(pc, handle, known_string_key.as_ref())?
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
                );
            }
            Some(PerfIndexTargetKind::Object) => {
                return self.set_object_index_handle(handle, key_reg, moved_key, value, known_string_key);
            }
            Some(PerfIndexTargetKind::String | PerfIndexTargetKind::Unknown) | None => {}
        }

        let key = match known_string_key {
            Some(key) => runtime_map_string_key(key),
            None => self.map_key_from_register_or_value(key_reg, moved_key)?,
        };

        if let Some(done) = self.try_set_string_list(handle, &key, value.clone())? {
            self.state.heap.bump_shape_generation(handle);
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
                let index = usize::try_from(index).map_err(|_| anyhow!("list index must be non-negative"))?;
                set_list_value(list, index, value)
            }
            HeapValue::Map(map) => {
                map.set(key, value);
                Ok::<(), anyhow::Error>(())
            }
            HeapValue::Object(object) => {
                let Some(key) = key.as_arc_str() else {
                    bail!("SetIndex object key must be string");
                };
                object.set_field(key, value);
                Ok(())
            }
            other => bail!("SetIndex target object is not writable: {:?}", heap_kind(other)),
        }?;
        self.state.heap.bump_shape_generation(handle);
        Ok(())
    }

    pub(super) fn push_list(&mut self, target_reg: u8, value_reg: u8, move_value: bool) -> Result<()> {
        let handle = {
            let target = self.read(target_reg)?;
            let RuntimeVal::Obj(handle) = target else {
                bail!("ListPush target expected Obj, got {:?}", target.kind());
            };
            *handle
        };
        let value = if move_value {
            self.take(value_reg)?
        } else {
            self.read(value_reg)?.clone()
        };
        let string_value = self.runtime_value_to_string(&value)?;

        if string_value.is_none() && matches!(self.state.heap.get(handle), Some(HeapValue::List(TypedList::String(_))))
        {
            self.push_string_list_polluted(handle, value)?;
        } else {
            let Some(HeapValue::List(list)) = self.state.heap.get_mut(handle) else {
                bail!("ListPush target object is not a list");
            };
            push_list_value(list, value, string_value)?;
        }

        self.state.heap.bump_shape_generation(handle);
        Ok(())
    }

    fn push_string_list_polluted(&mut self, handle: HeapRef, value: RuntimeVal) -> Result<()> {
        let values = match self
            .state
            .heap
            .get(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::List(TypedList::String(values)) => values.clone(),
            other => bail!(
                "ListPush target object changed while materializing string list: {:?}",
                heap_kind(other)
            ),
        };
        let mut mixed = Vec::with_capacity(values.len() + 1);
        for value in values {
            mixed.push(self.runtime_string_key_to_value(value));
        }
        mixed.push(value);
        let Some(HeapValue::List(list)) = self.state.heap.get_mut(handle) else {
            bail!("heap object {} changed while materializing string list", handle.index());
        };
        *list = TypedList::Mixed(mixed);
        Ok(())
    }

    fn index_fact_from_heap(&self, handle: HeapRef) -> Result<PerfIndexFact> {
        match self
            .state
            .heap
            .get(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::List(list) => Ok(PerfIndexFact {
                target_kind: PerfIndexTargetKind::List,
                value_kind: list_value_kind(list),
            }),
            HeapValue::Map(map) => Ok(PerfIndexFact {
                target_kind: PerfIndexTargetKind::Map,
                value_kind: map_value_kind(map),
            }),
            HeapValue::Object(_) => Ok(PerfIndexFact {
                target_kind: PerfIndexTargetKind::Object,
                value_kind: PerfValueKind::Unknown,
            }),
            HeapValue::String(_) => Ok(PerfIndexFact {
                target_kind: PerfIndexTargetKind::String,
                value_kind: PerfValueKind::Unknown,
            }),
            other => bail!("index target object is not indexable: {:?}", heap_kind(other)),
        }
    }

    fn cached_or_observed_index_cache(
        &mut self,
        pc: usize,
        handle: HeapRef,
        known_string_key: Option<&Arc<str>>,
    ) -> Result<Option<IndexInlineCache32>> {
        let generation = self
            .state
            .heap
            .shape_generation(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
        if let Some(cache) = self.state.inline_caches.index(pc, handle, generation) {
            return Ok(Some(cache));
        }
        let fact = self.index_fact_from_heap(handle)?;
        let object_field_slot = self.object_field_slot_from_heap(handle, known_string_key)?;
        self.state
            .inline_caches
            .set_index(pc, handle, generation, fact, object_field_slot);
        Ok(self.state.inline_caches.index(pc, handle, generation))
    }

    fn object_field_slot_from_heap(&self, handle: HeapRef, key: Option<&Arc<str>>) -> Result<Option<u16>> {
        let Some(key) = key else {
            return Ok(None);
        };
        match self
            .state
            .heap
            .get(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::Object(object) => Ok(object.field_slot(key).and_then(|slot| u16::try_from(slot).ok())),
            _ => Ok(None),
        }
    }

    fn set_list_index_handle(
        &mut self,
        handle: HeapRef,
        key_reg: u8,
        moved_key: Option<RuntimeVal>,
        value: RuntimeVal,
        known_value_kind: Option<PerfValueKind>,
    ) -> Result<()> {
        let index = self.int_key_from_register_or_value(key_reg, moved_key)?;
        let key = RuntimeMapKey::Int(index);
        if matches!(self.state.heap.get(handle), Some(HeapValue::List(TypedList::String(_)))) {
            if let Some(done) = self.try_set_string_list(handle, &key, value.clone())? {
                self.state.heap.bump_shape_generation(handle);
                return Ok(done);
            }
        }
        let index = usize::try_from(index).map_err(|_| anyhow!("list index must be non-negative"))?;
        if self.try_set_typed_list_index(handle, index, &value, known_value_kind)? {
            self.state.heap.bump_shape_generation(handle);
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
                heap_kind(other)
            ),
        }?;
        self.state.heap.bump_shape_generation(handle);
        Ok(())
    }

    fn try_set_typed_list_index(
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
            (PerfValueKind::Int, HeapValue::List(TypedList::Int(values)), RuntimeVal::Int(value)) => {
                let Some(slot) = values.get_mut(index) else {
                    bail!("list index {} out of bounds", index);
                };
                *slot = *value;
                Ok(true)
            }
            (PerfValueKind::Float, HeapValue::List(TypedList::Float(values)), RuntimeVal::Float(value)) => {
                let Some(slot) = values.get_mut(index) else {
                    bail!("list index {} out of bounds", index);
                };
                *slot = *value;
                Ok(true)
            }
            (PerfValueKind::Bool, HeapValue::List(TypedList::Bool(values)), RuntimeVal::Bool(value)) => {
                let Some(slot) = values.get_mut(index) else {
                    bail!("list index {} out of bounds", index);
                };
                *slot = *value;
                Ok(true)
            }
            (PerfValueKind::Unknown, _, _) | (_, HeapValue::List(_), _) => Ok(false),
            (_, other, _) => bail!(
                "SetIndex target object changed while writing list: {:?}",
                heap_kind(other)
            ),
        }
    }

    fn set_map_index_handle(
        &mut self,
        handle: HeapRef,
        key_reg: u8,
        moved_key: Option<RuntimeVal>,
        value: RuntimeVal,
        known_string_key: Option<Arc<str>>,
        known_value_kind: Option<PerfValueKind>,
    ) -> Result<()> {
        let key = match known_string_key {
            Some(key) => runtime_map_string_key(key),
            None => self.map_key_from_register_or_value(key_reg, moved_key)?,
        };
        if self.try_set_typed_string_map_index(handle, &key, &value, known_value_kind)? {
            self.state.heap.bump_shape_generation(handle);
            return Ok(());
        }
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
                heap_kind(other)
            ),
        }?;
        self.state.heap.bump_shape_generation(handle);
        Ok(())
    }

    fn try_set_typed_string_map_index(
        &mut self,
        handle: HeapRef,
        key: &RuntimeMapKey,
        value: &RuntimeVal,
        known_value_kind: Option<PerfValueKind>,
    ) -> Result<bool> {
        let Some(key) = key.as_arc_str() else {
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
            (PerfValueKind::Int, HeapValue::Map(TypedMap::StringInt(values)), RuntimeVal::Int(value)) => {
                values.insert(key, *value);
                Ok(true)
            }
            (PerfValueKind::Float, HeapValue::Map(TypedMap::StringFloat(values)), RuntimeVal::Float(value)) => {
                values.insert(key, *value);
                Ok(true)
            }
            (PerfValueKind::Bool, HeapValue::Map(TypedMap::StringBool(values)), RuntimeVal::Bool(value)) => {
                values.insert(key, *value);
                Ok(true)
            }
            (PerfValueKind::Unknown, _, _) | (_, HeapValue::Map(_), _) => Ok(false),
            (_, other, _) => bail!(
                "SetIndex target object changed while writing map: {:?}",
                heap_kind(other)
            ),
        }
    }

    fn set_object_index_handle(
        &mut self,
        handle: HeapRef,
        key_reg: u8,
        moved_key: Option<RuntimeVal>,
        value: RuntimeVal,
        known_string_key: Option<Arc<str>>,
    ) -> Result<()> {
        let key = match known_string_key {
            Some(key) => key,
            None => self.object_key_from_register_or_value(key_reg, moved_key)?,
        };
        match self
            .state
            .heap
            .get_mut(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::Object(object) => {
                object.set_field(key, value);
                Ok::<(), anyhow::Error>(())
            }
            other => bail!(
                "SetIndex target object changed while writing object: {:?}",
                heap_kind(other)
            ),
        }?;
        self.state.heap.bump_shape_generation(handle);
        Ok(())
    }

    fn object_key_from_register(&self, register: u8) -> Result<Arc<str>> {
        self.object_key_from_value(self.read(register)?)
    }

    fn object_key_from_register_or_value(&self, register: u8, moved_key: Option<RuntimeVal>) -> Result<Arc<str>> {
        match moved_key {
            Some(value) => self.object_key_from_value(&value),
            None => self.object_key_from_register(register),
        }
    }

    fn object_key_from_value(&self, value: &RuntimeVal) -> Result<Arc<str>> {
        match value {
            RuntimeVal::ShortStr(value) => Ok(Arc::<str>::from(value.as_str())),
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::String(value) => Ok(value.clone()),
                other => bail!("object field key cannot be object: {:?}", heap_kind(other)),
            },
            other => bail!("object field key must be string, got {:?}", other.kind()),
        }
    }

    fn try_set_string_list(
        &mut self,
        handle: crate::val::HeapRef,
        key: &RuntimeMapKey,
        value: RuntimeVal,
    ) -> Result<Option<()>> {
        let RuntimeMapKey::Int(index) = key else {
            return Ok(None);
        };
        let Some(HeapValue::List(TypedList::String(values))) = self.state.heap.get(handle) else {
            return Ok(None);
        };
        let index = usize::try_from(*index).map_err(|_| anyhow!("list index must be non-negative"))?;
        if index >= values.len() {
            bail!("list index {} out of bounds", index);
        }

        if let Some(value) = self.runtime_value_to_string(&value)? {
            let Some(HeapValue::List(TypedList::String(values))) = self.state.heap.get_mut(handle) else {
                bail!("heap object {} changed while writing string list", handle.index());
            };
            values[index] = value;
            return Ok(Some(()));
        }

        let Some(HeapValue::List(TypedList::String(values))) = self.state.heap.get_mut(handle) else {
            bail!("heap object {} changed while taking string list", handle.index());
        };
        let strings = std::mem::take(values);
        let mut mixed = Vec::with_capacity(strings.len());
        for value in strings {
            mixed.push(self.runtime_value_from_string(value));
        }
        mixed[index] = value;
        let Some(HeapValue::List(list)) = self.state.heap.get_mut(handle) else {
            bail!("heap object {} changed while materializing string list", handle.index());
        };
        *list = TypedList::Mixed(mixed);
        Ok(Some(()))
    }

    fn index_list_handle(
        &mut self,
        handle: HeapRef,
        key_reg: u8,
        known_value_kind: Option<PerfValueKind>,
    ) -> Result<RuntimeVal> {
        let index = usize::try_from(self.read_int(key_reg)?).map_err(|_| anyhow!("list index must be non-negative"))?;
        if let Some(value) = self.index_typed_list_handle(handle, index, known_value_kind)? {
            return Ok(value);
        }
        let long_string = match self
            .state
            .heap
            .get(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::List(TypedList::Mixed(values)) => {
                return Ok(values.get(index).cloned().unwrap_or(RuntimeVal::Nil));
            }
            HeapValue::List(TypedList::Int(values)) => {
                return Ok(values
                    .get(index)
                    .copied()
                    .map(RuntimeVal::Int)
                    .unwrap_or(RuntimeVal::Nil));
            }
            HeapValue::List(TypedList::Float(values)) => {
                return Ok(values
                    .get(index)
                    .copied()
                    .map(RuntimeVal::Float)
                    .unwrap_or(RuntimeVal::Nil));
            }
            HeapValue::List(TypedList::Bool(values)) => {
                return Ok(values
                    .get(index)
                    .copied()
                    .map(RuntimeVal::Bool)
                    .unwrap_or(RuntimeVal::Nil));
            }
            HeapValue::List(TypedList::String(values)) => {
                let Some(value) = values.get(index) else {
                    return Ok(RuntimeVal::Nil);
                };
                if let Some(short) = ShortStr::new(value) {
                    return Ok(RuntimeVal::ShortStr(short));
                }
                value.clone()
            }
            other => bail!("GetIndex target object changed while indexing: {:?}", heap_kind(other)),
        };
        Ok(RuntimeVal::Obj(self.alloc_heap_value(HeapValue::String(long_string))))
    }

    fn index_typed_list_handle(
        &mut self,
        handle: HeapRef,
        index: usize,
        known_value_kind: Option<PerfValueKind>,
    ) -> Result<Option<RuntimeVal>> {
        match (
            known_value_kind.unwrap_or_default(),
            self.state
                .heap
                .get(handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?,
        ) {
            (PerfValueKind::Int, HeapValue::List(TypedList::Int(values))) => Ok(Some(
                values
                    .get(index)
                    .copied()
                    .map(RuntimeVal::Int)
                    .unwrap_or(RuntimeVal::Nil),
            )),
            (PerfValueKind::Float, HeapValue::List(TypedList::Float(values))) => Ok(Some(
                values
                    .get(index)
                    .copied()
                    .map(RuntimeVal::Float)
                    .unwrap_or(RuntimeVal::Nil),
            )),
            (PerfValueKind::Bool, HeapValue::List(TypedList::Bool(values))) => Ok(Some(
                values
                    .get(index)
                    .copied()
                    .map(RuntimeVal::Bool)
                    .unwrap_or(RuntimeVal::Nil),
            )),
            (PerfValueKind::String, HeapValue::List(TypedList::String(values))) => {
                let Some(value) = values.get(index) else {
                    return Ok(Some(RuntimeVal::Nil));
                };
                if let Some(short) = ShortStr::new(value) {
                    return Ok(Some(RuntimeVal::ShortStr(short)));
                }
                Ok(None)
            }
            (PerfValueKind::Unknown, _) => Ok(None),
            (_, HeapValue::List(_)) => Ok(None),
            (_, other) => bail!("GetIndex target object changed while indexing: {:?}", heap_kind(other)),
        }
    }

    fn index_string_at(&self, value: &str, index: usize) -> Result<RuntimeVal> {
        if value.is_ascii() {
            let Some(byte) = value.as_bytes().get(index).copied() else {
                return Ok(RuntimeVal::Nil);
            };
            let mut buf = [0_u8; 4];
            let ch = (byte as char).encode_utf8(&mut buf);
            return Ok(RuntimeVal::ShortStr(ShortStr::new(ch).expect("ascii char is short")));
        }
        let Some(ch) = value.chars().nth(index) else {
            return Ok(RuntimeVal::Nil);
        };
        let mut buf = [0_u8; 4];
        let ch = ch.encode_utf8(&mut buf);
        if let Some(short) = ShortStr::new(ch) {
            Ok(RuntimeVal::ShortStr(short))
        } else {
            Ok(RuntimeVal::Nil)
        }
    }

    fn lookup_map_handle(&self, handle: HeapRef, key: &RuntimeMapKey) -> Result<Option<RuntimeVal>> {
        match self
            .state
            .heap
            .get(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::Map(map) => Ok(map.get(key)),
            other => bail!("GetIndex target object changed while indexing: {:?}", heap_kind(other)),
        }
    }

    fn lookup_string_map_handle(
        &self,
        handle: HeapRef,
        key: &Arc<str>,
        known_value_kind: Option<PerfValueKind>,
    ) -> Result<Option<RuntimeVal>> {
        match (
            known_value_kind.unwrap_or_default(),
            self.state
                .heap
                .get(handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?,
        ) {
            (PerfValueKind::Int, HeapValue::Map(TypedMap::StringInt(values))) => Ok(Some(
                values
                    .get(key.as_ref())
                    .copied()
                    .map(RuntimeVal::Int)
                    .unwrap_or(RuntimeVal::Nil),
            )),
            (PerfValueKind::Float, HeapValue::Map(TypedMap::StringFloat(values))) => Ok(Some(
                values
                    .get(key.as_ref())
                    .copied()
                    .map(RuntimeVal::Float)
                    .unwrap_or(RuntimeVal::Nil),
            )),
            (PerfValueKind::Bool, HeapValue::Map(TypedMap::StringBool(values))) => Ok(Some(
                values
                    .get(key.as_ref())
                    .copied()
                    .map(RuntimeVal::Bool)
                    .unwrap_or(RuntimeVal::Nil),
            )),
            (PerfValueKind::Unknown, _) => Ok(None),
            (_, HeapValue::Map(_)) => Ok(None),
            (_, other) => bail!("GetIndex target object changed while indexing: {:?}", heap_kind(other)),
        }
    }

    fn index_object_handle(
        &self,
        handle: HeapRef,
        key: &Arc<str>,
        cached_slot: Option<u16>,
    ) -> Result<Option<RuntimeVal>> {
        match self
            .state
            .heap
            .get(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::Object(object) => {
                if let Some(slot) = cached_slot
                    && let Some(value) = object.get_field_slot(slot as usize, key)
                {
                    return Ok(Some(value));
                }
                Ok(object.get_field(key))
            }
            other => bail!("GetIndex target object changed while indexing: {:?}", heap_kind(other)),
        }
    }

    fn map_key_from_register(&self, register: u8) -> Result<RuntimeMapKey> {
        self.runtime_map_key_from_value(self.read(register)?)
    }

    fn map_key_from_register_or_value(&self, register: u8, moved_key: Option<RuntimeVal>) -> Result<RuntimeMapKey> {
        match moved_key {
            Some(value) => self.runtime_map_key_from_value(&value),
            None => self.map_key_from_register(register),
        }
    }

    fn int_key_from_register_or_value(&self, register: u8, moved_key: Option<RuntimeVal>) -> Result<i64> {
        match moved_key {
            Some(RuntimeVal::Int(value)) => Ok(value),
            Some(other) => bail!("SetIndex list key must be Int, got {:?}", other.kind()),
            None => self.read_int(register),
        }
    }

    pub(super) fn runtime_map_key_from_value(&self, value: &RuntimeVal) -> Result<RuntimeMapKey> {
        match value {
            RuntimeVal::Nil => Ok(RuntimeMapKey::Nil),
            RuntimeVal::Bool(value) => Ok(RuntimeMapKey::Bool(*value)),
            RuntimeVal::Int(value) => Ok(RuntimeMapKey::Int(*value)),
            RuntimeVal::ShortStr(value) => Ok(RuntimeMapKey::ShortStr(*value)),
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::String(value) => Ok(RuntimeMapKey::String(value.clone())),
                other => bail!("object cannot be used as map key: {:?}", heap_kind(other)),
            },
            RuntimeVal::Float(_) => bail!("Float cannot be used as RuntimeMapKey"),
        }
    }

    fn runtime_value_to_key_string(&self, value: &RuntimeVal) -> Result<Option<Arc<str>>> {
        Ok(match value {
            RuntimeVal::Bool(value) => Some(Arc::<str>::from(value.to_string())),
            RuntimeVal::Int(value) => Some(Arc::<str>::from(value.to_string())),
            RuntimeVal::Float(value) => Some(Arc::<str>::from(value.to_string())),
            RuntimeVal::ShortStr(value) => Some(Arc::<str>::from(value.as_str())),
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::String(value) => Some(value.clone()),
                _ => None,
            },
            RuntimeVal::Nil => None,
        })
    }

    fn string_map_contains_key<T>(&self, values: &BTreeMap<Arc<str>, T>, needle: &RuntimeVal) -> Result<bool> {
        let Some(key) = self.runtime_value_to_key_string(needle)? else {
            return Ok(false);
        };
        Ok(values.contains_key(key.as_ref()))
    }
}

fn runtime_map_string_key(value: Arc<str>) -> RuntimeMapKey {
    if let Some(short) = ShortStr::new(&value) {
        RuntimeMapKey::ShortStr(short)
    } else {
        RuntimeMapKey::String(value)
    }
}

fn list_value_kind(list: &TypedList) -> PerfValueKind {
    match list {
        TypedList::Int(_) => PerfValueKind::Int,
        TypedList::Float(_) => PerfValueKind::Float,
        TypedList::Bool(_) => PerfValueKind::Bool,
        TypedList::String(_) => PerfValueKind::String,
        TypedList::Mixed(_) => PerfValueKind::Unknown,
    }
}

fn typed_map_without_keys(map: &TypedMap, removed_keys: &[RuntimeMapKey]) -> TypedMap {
    match map {
        TypedMap::Mixed(entries) => {
            let mut out = BTreeMap::new();
            for (key, value) in entries {
                if !typed_map_key_removed(key, removed_keys) {
                    out.insert(key.clone(), value.clone());
                }
            }
            TypedMap::Mixed(out)
        }
        TypedMap::StringMixed(entries) => {
            let mut out = BTreeMap::new();
            for (key, value) in entries {
                if !string_map_key_removed(key, removed_keys) {
                    out.insert(Arc::clone(key), value.clone());
                }
            }
            TypedMap::StringMixed(out)
        }
        TypedMap::StringInt(entries) => {
            let mut out = BTreeMap::new();
            for (key, value) in entries {
                if !string_map_key_removed(key, removed_keys) {
                    out.insert(Arc::clone(key), *value);
                }
            }
            TypedMap::StringInt(out)
        }
        TypedMap::StringFloat(entries) => {
            let mut out = BTreeMap::new();
            for (key, value) in entries {
                if !string_map_key_removed(key, removed_keys) {
                    out.insert(Arc::clone(key), *value);
                }
            }
            TypedMap::StringFloat(out)
        }
        TypedMap::StringBool(entries) => {
            let mut out = BTreeMap::new();
            for (key, value) in entries {
                if !string_map_key_removed(key, removed_keys) {
                    out.insert(Arc::clone(key), *value);
                }
            }
            TypedMap::StringBool(out)
        }
    }
}

fn typed_map_key_removed(key: &RuntimeMapKey, removed_keys: &[RuntimeMapKey]) -> bool {
    removed_keys.iter().any(|removed| runtime_map_keys_match(key, removed))
}

fn string_map_key_removed(key: &Arc<str>, removed_keys: &[RuntimeMapKey]) -> bool {
    removed_keys
        .iter()
        .any(|removed| removed.as_str().is_some_and(|removed| removed == key.as_ref()))
}

fn runtime_map_keys_match(left: &RuntimeMapKey, right: &RuntimeMapKey) -> bool {
    left == right
        || left
            .as_str()
            .zip(right.as_str())
            .is_some_and(|(left, right)| left == right)
}

fn typed_map_iter_snapshot(map: &TypedMap) -> TypedMapIterSnapshot {
    match map {
        TypedMap::Mixed(entries) => {
            let mut out = Vec::with_capacity(entries.len());
            for (key, value) in entries {
                out.push((key.clone(), value.clone()));
            }
            TypedMapIterSnapshot::Mixed(out)
        }
        TypedMap::StringMixed(entries) => {
            let mut out = Vec::with_capacity(entries.len());
            for (key, value) in entries {
                out.push((Arc::clone(key), value.clone()));
            }
            TypedMapIterSnapshot::StringMixed(out)
        }
        TypedMap::StringInt(entries) => {
            let mut out = Vec::with_capacity(entries.len());
            for (key, value) in entries {
                out.push((Arc::clone(key), *value));
            }
            TypedMapIterSnapshot::StringInt(out)
        }
        TypedMap::StringFloat(entries) => {
            let mut out = Vec::with_capacity(entries.len());
            for (key, value) in entries {
                out.push((Arc::clone(key), *value));
            }
            TypedMapIterSnapshot::StringFloat(out)
        }
        TypedMap::StringBool(entries) => {
            let mut out = Vec::with_capacity(entries.len());
            for (key, value) in entries {
                out.push((Arc::clone(key), *value));
            }
            TypedMapIterSnapshot::StringBool(out)
        }
    }
}

fn string_chars_to_list(value: &str) -> Vec<Arc<str>> {
    let mut out = Vec::new();
    for ch in value.chars() {
        out.push(Arc::<str>::from(ch.to_string()));
    }
    out
}

#[inline]
fn string_char_len(value: &str) -> usize {
    if value.is_ascii() {
        value.len()
    } else {
        value.chars().count()
    }
}

impl TypedMapIterSnapshot {
    fn len(&self) -> usize {
        match self {
            Self::Mixed(entries) => entries.len(),
            Self::StringMixed(entries) => entries.len(),
            Self::StringInt(entries) => entries.len(),
            Self::StringFloat(entries) => entries.len(),
            Self::StringBool(entries) => entries.len(),
        }
    }
}

fn map_value_kind(map: &TypedMap) -> PerfValueKind {
    match map {
        TypedMap::StringInt(_) => PerfValueKind::Int,
        TypedMap::StringFloat(_) => PerfValueKind::Float,
        TypedMap::StringBool(_) => PerfValueKind::Bool,
        TypedMap::Mixed(_) | TypedMap::StringMixed(_) => PerfValueKind::Unknown,
    }
}
