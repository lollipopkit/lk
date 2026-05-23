use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::val::{HeapRef, HeapValue, RuntimeMapKey, RuntimeObject, RuntimeVal, ShortStr, TypedList, TypedMap};

use super::{Executor32, heap_kind, remove_runtime_entry, set_list_value};
use crate::vm::analysis::{PerfIndexFact, PerfIndexTargetKind, PerfValueKind};

#[derive(Clone, Copy)]
enum IndexTargetKind {
    List,
    Map,
    Object,
    String,
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
        Ok(RuntimeObject { type_name, fields })
    }

    pub(super) fn get_index(
        &mut self,
        pc: usize,
        target_reg: u8,
        key_reg: u8,
        known_string_key: Option<Arc<str>>,
        index_fact: Option<PerfIndexFact>,
    ) -> Result<RuntimeVal> {
        match self.read(target_reg)?.clone() {
            RuntimeVal::ShortStr(value) => {
                let value = Arc::<str>::from(value.as_str());
                self.index_string(&value, key_reg)
            }
            RuntimeVal::Obj(handle) => {
                let index_fact = match index_fact {
                    Some(fact) => Some(fact),
                    None => {
                        let fact = self.index_fact_from_heap(handle)?;
                        self.state.inline_caches.set_index(pc, fact);
                        Some(fact)
                    }
                };
                let target_kind = match index_fact.map(|fact| fact.target_kind) {
                    Some(PerfIndexTargetKind::List) => IndexTargetKind::List,
                    Some(PerfIndexTargetKind::Map) => IndexTargetKind::Map,
                    Some(PerfIndexTargetKind::Object) => IndexTargetKind::Object,
                    Some(PerfIndexTargetKind::String) => IndexTargetKind::String,
                    Some(PerfIndexTargetKind::Unknown) | None => self.index_target_kind(handle)?,
                };

                match target_kind {
                    IndexTargetKind::List => {
                        self.index_list_handle(handle, key_reg, index_fact.map(|fact| fact.value_kind))
                    }
                    IndexTargetKind::Map => {
                        if let Some(key) = known_string_key.as_ref()
                            && let Some(value) =
                                self.lookup_string_map_handle(handle, key, index_fact.map(|fact| fact.value_kind))?
                        {
                            return Ok(value);
                        }
                        let key = match known_string_key.as_ref() {
                            Some(key) => runtime_map_string_key(key.clone()),
                            None => self.map_key_from_register(key_reg)?,
                        };
                        Ok(self.lookup_map_handle(handle, &key)?.unwrap_or(RuntimeVal::Nil))
                    }
                    IndexTargetKind::Object => {
                        let key = match known_string_key {
                            Some(key) => key,
                            None => self.object_key_from_register(key_reg)?,
                        };
                        Ok(self.index_object_handle(handle, &key)?.unwrap_or(RuntimeVal::Nil))
                    }
                    IndexTargetKind::String => {
                        let value = match self
                            .state
                            .heap
                            .get(handle)
                            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
                        {
                            HeapValue::String(value) => value.clone(),
                            other => bail!("GetIndex target object changed while indexing: {:?}", heap_kind(other)),
                        };
                        self.index_string(&value, key_reg)
                    }
                }
            }
            other => bail!("GetIndex target expected Obj, got {:?}", other.kind()),
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
        match self.read(register)? {
            RuntimeVal::ShortStr(value) => Ok(value.as_str().chars().count()),
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::String(value) => Ok(value.chars().count()),
                HeapValue::List(value) => Ok(value.len()),
                HeapValue::Map(value) => Ok(value.len()),
                other => bail!("Len target object is not sized: {:?}", heap_kind(other)),
            },
            other => bail!("Len target expected string/list/map, got {:?}", other.kind()),
        }
    }

    pub(super) fn contains_value(&self, needle_reg: u8, haystack_reg: u8) -> Result<bool> {
        let needle = self.read(needle_reg)?.clone();
        match self.read(haystack_reg)?.clone() {
            RuntimeVal::ShortStr(haystack) => {
                let Some(needle) = self.runtime_value_to_string(&needle)? else {
                    return Ok(false);
                };
                Ok(haystack.as_str().contains(needle.as_ref()))
            }
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(handle)
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
                let value = self
                    .state
                    .heap
                    .get(handle)
                    .cloned()
                    .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
                match value {
                    HeapValue::List(values) => Ok(RuntimeVal::Obj(
                        self.state.heap.alloc(HeapValue::List(values.slice_from(start))),
                    )),
                    HeapValue::String(value) => self.slice_string_from(value.clone(), start),
                    other => bail!("SliceFrom target object is not sliceable: {:?}", heap_kind(&other)),
                }
            }
            other => bail!("SliceFrom target expected string/list object, got {:?}", other.kind()),
        }
    }

    fn slice_string_from(&mut self, value: Arc<str>, start: usize) -> Result<RuntimeVal> {
        let suffix = value.chars().skip(start).collect::<String>();
        Ok(self.runtime_value_from_string(Arc::<str>::from(suffix)))
    }

    pub(super) fn map_rest(&mut self, base: u8, key_count: u8) -> Result<RuntimeVal> {
        let RuntimeVal::Obj(handle) = self.read(base)?.clone() else {
            bail!("MapRest base expected map object");
        };
        let source = self
            .state
            .heap
            .get(handle)
            .cloned()
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
        let HeapValue::Map(map) = &source else {
            bail!("MapRest source object is not a map: {:?}", heap_kind(&source));
        };

        let mut entries = map
            .entries_into_heap(&mut self.state.heap)?
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        for offset in 0..key_count {
            let key_reg = base
                .checked_add(1)
                .and_then(|reg| reg.checked_add(offset))
                .ok_or_else(|| anyhow!("MapRest key register overflow"))?;
            let key = self.map_key_from_register(key_reg)?;
            remove_runtime_entry(&mut entries, &key);
        }
        Ok(RuntimeVal::Obj(
            self.state
                .heap
                .alloc(HeapValue::Map(TypedMap::from_runtime_entries(entries))),
        ))
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
                let list = value
                    .as_str()
                    .chars()
                    .map(|ch| Arc::<str>::from(ch.to_string()))
                    .collect();
                Ok(RuntimeVal::Obj(
                    self.state.heap.alloc(HeapValue::List(TypedList::String(list))),
                ))
            }
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(handle)
                .cloned()
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::List(_) => Ok(RuntimeVal::Obj(handle)),
                HeapValue::String(value) => {
                    let list = value.chars().map(|ch| Arc::<str>::from(ch.to_string())).collect();
                    Ok(RuntimeVal::Obj(
                        self.state.heap.alloc(HeapValue::List(TypedList::String(list))),
                    ))
                }
                HeapValue::Map(map) => self.map_to_iter_list(&map),
                other => bail!("ToIter target object is not iterable: {:?}", heap_kind(&other)),
            },
            other => bail!("ToIter target expected string/list/map, got {:?}", other.kind()),
        }
    }

    fn map_to_iter_list(&mut self, map: &TypedMap) -> Result<RuntimeVal> {
        let mut pairs = Vec::with_capacity(map.len());
        for (key, value) in map.entries_into_heap(&mut self.state.heap)? {
            let key = self.runtime_map_key_to_value(key);
            let pair = HeapValue::List(TypedList::from_runtime_values(vec![key, value], &self.state.heap));
            pairs.push(RuntimeVal::Obj(self.state.heap.alloc(pair)));
        }
        Ok(RuntimeVal::Obj(
            self.state.heap.alloc(HeapValue::List(TypedList::Mixed(pairs))),
        ))
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
                    RuntimeVal::Obj(self.state.heap.alloc(HeapValue::String(value)))
                }
            }
            RuntimeMapKey::Obj(value) => RuntimeVal::Obj(value),
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
        let target = self.read(target_reg)?.clone();
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
        let RuntimeVal::Obj(handle) = target else {
            bail!("SetIndex target expected Obj, got {:?}", target.kind());
        };
        let index_fact = match index_fact {
            Some(fact) => Some(fact),
            None => {
                let fact = self.index_fact_from_heap(handle)?;
                self.state.inline_caches.set_index(pc, fact);
                Some(fact)
            }
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
                Ok(())
            }
            HeapValue::Object(object) => {
                let Some(key) = key.as_arc_str() else {
                    bail!("SetIndex object key must be string");
                };
                object.fields.insert(key, value);
                Ok(())
            }
            other => bail!("SetIndex target object is not writable: {:?}", heap_kind(other)),
        }
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
                return Ok(done);
            }
        }
        let index = usize::try_from(index).map_err(|_| anyhow!("list index must be non-negative"))?;
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
                heap_kind(other)
            ),
        }
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
                Ok(())
            }
            other => bail!(
                "SetIndex target object changed while writing map: {:?}",
                heap_kind(other)
            ),
        }
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
                object.fields.insert(key, value);
                Ok(())
            }
            other => bail!(
                "SetIndex target object changed while writing object: {:?}",
                heap_kind(other)
            ),
        }
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

        let strings = values.clone();
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
        Ok(RuntimeVal::Obj(self.state.heap.alloc(HeapValue::String(long_string))))
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

    fn index_string(&self, value: &Arc<str>, key_reg: u8) -> Result<RuntimeVal> {
        let index =
            usize::try_from(self.read_int(key_reg)?).map_err(|_| anyhow!("string index must be non-negative"))?;
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

    fn index_object_handle(&self, handle: HeapRef, key: &Arc<str>) -> Result<Option<RuntimeVal>> {
        match self
            .state
            .heap
            .get(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::Object(object) => Ok(object.fields.get(key).cloned()),
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

fn map_value_kind(map: &TypedMap) -> PerfValueKind {
    match map {
        TypedMap::StringInt(_) => PerfValueKind::Int,
        TypedMap::StringFloat(_) => PerfValueKind::Float,
        TypedMap::StringBool(_) => PerfValueKind::Bool,
        TypedMap::Mixed(_) | TypedMap::StringMixed(_) => PerfValueKind::Unknown,
    }
}
