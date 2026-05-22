use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Result;
use arcstr::ArcStr;

use crate::util::fast_map::FastHashMap;

use super::{
    HeapStore, HeapValue, OwnedRuntimeList, OwnedRuntimeMap, RuntimeMapKey, RuntimeObject, RuntimeVal, ShortStr,
    TypedList, TypedMap, Val,
};

impl OwnedRuntimeList {
    pub(crate) fn to_legacy_values(&self) -> Vec<Val> {
        self.values
            .iter()
            .map(|value| owned_runtime_val_to_val(value, &self.heap))
            .collect()
    }

    pub(crate) fn copy_values_into(&self, heap: &mut HeapStore) -> Vec<RuntimeVal> {
        let mut source_heap = self.heap.clone();
        self.values
            .iter()
            .map(|value| crate::vm::copy_runtime_value(value, &mut source_heap, heap).unwrap_or(RuntimeVal::Nil))
            .collect()
    }

    pub(crate) fn copy_into_typed_list(&self, heap: &mut HeapStore) -> Result<TypedList> {
        let mut source_heap = self.heap.clone();
        let values = self
            .values
            .iter()
            .map(|value| crate::vm::copy_runtime_value(value, &mut source_heap, heap))
            .collect::<Result<Vec<_>>>()?;
        Ok(TypedList::from_runtime_values(values, heap))
    }

    pub(crate) fn slice_from(&self, start: usize) -> Self {
        Self {
            values: self.values.get(start..).unwrap_or(&[]).to_vec(),
            heap: self.heap.clone(),
        }
    }
}

impl OwnedRuntimeMap {
    pub(crate) fn to_legacy_entries(&self) -> FastHashMap<ArcStr, Val> {
        let mut out = FastHashMap::default();
        for (key, value) in &self.entries {
            if let Some(key) = key.as_str() {
                out.insert(ArcStr::from(key), owned_runtime_val_to_val(value, &self.heap));
            }
        }
        out
    }

    pub(crate) fn inline_entries(&self) -> Vec<(RuntimeMapKey, RuntimeVal)> {
        self.inline_entries_map().into_iter().collect()
    }

    pub(crate) fn inline_entries_map(&self) -> BTreeMap<RuntimeMapKey, RuntimeVal> {
        self.entries
            .iter()
            .filter_map(|(key, value)| inline_owned_runtime_value(value).map(|value| (key.clone(), value)))
            .collect()
    }

    pub(crate) fn copy_entries_into(&self, heap: &mut HeapStore) -> Result<BTreeMap<RuntimeMapKey, RuntimeVal>> {
        let mut source_heap = self.heap.clone();
        self.entries
            .iter()
            .map(|(key, value)| {
                Ok((
                    key.clone(),
                    crate::vm::copy_runtime_value(value, &mut source_heap, heap)?,
                ))
            })
            .collect()
    }

    pub(crate) fn copy_value_into(&self, key: &RuntimeMapKey, heap: &mut HeapStore) -> Result<Option<RuntimeVal>> {
        let Some(value) = self.entries.get(key) else {
            return Ok(None);
        };
        let mut source_heap = self.heap.clone();
        Ok(Some(crate::vm::copy_runtime_value(value, &mut source_heap, heap)?))
    }

    pub(crate) fn copy_str_value_into(&self, key: &str, heap: &mut HeapStore) -> Result<Option<RuntimeVal>> {
        let short = ShortStr::new(key).map(RuntimeMapKey::ShortStr);
        let long = RuntimeMapKey::String(Arc::<str>::from(key));
        let value = short
            .as_ref()
            .and_then(|key| self.entries.get(key))
            .or_else(|| self.entries.get(&long));
        let Some(value) = value else {
            return Ok(None);
        };
        let mut source_heap = self.heap.clone();
        Ok(Some(crate::vm::copy_runtime_value(value, &mut source_heap, heap)?))
    }

    pub(crate) fn copy_into_typed_map(&self, heap: &mut HeapStore) -> Result<TypedMap> {
        let mut source_heap = self.heap.clone();
        let entries = self
            .entries
            .iter()
            .map(|(key, value)| {
                Ok((
                    key.clone(),
                    crate::vm::copy_runtime_value(value, &mut source_heap, heap)?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        Ok(TypedMap::from_runtime_entries(entries))
    }

    pub(crate) fn get(&self, key: &RuntimeMapKey) -> Option<RuntimeVal> {
        self.entries.get(key).cloned()
    }

    pub(crate) fn get_str(&self, key: &str) -> Option<RuntimeVal> {
        if let Some(value) = ShortStr::new(key).and_then(|key| self.entries.get(&RuntimeMapKey::ShortStr(key)).cloned())
        {
            return Some(value);
        }
        self.entries.get(&RuntimeMapKey::String(Arc::<str>::from(key))).cloned()
    }

    pub(crate) fn set(&mut self, key: RuntimeMapKey, value: RuntimeVal) {
        self.entries.insert(key, value);
    }
}

pub(super) fn legacy_val_to_inline_runtime(value: &Val) -> Option<RuntimeVal> {
    match value {
        Val::Nil => Some(RuntimeVal::Nil),
        Val::Bool(value) => Some(RuntimeVal::Bool(*value)),
        Val::Int(value) => Some(RuntimeVal::Int(*value)),
        Val::Float(value) => Some(RuntimeVal::Float(*value)),
        Val::ShortStr(value) => Some(RuntimeVal::ShortStr(*value)),
        Val::Obj(value) => match value.as_ref() {
            HeapValue::String(value) => ShortStr::new(value.as_ref()).map(RuntimeVal::ShortStr),
            _ => None,
        },
    }
}

fn inline_owned_runtime_value(value: &RuntimeVal) -> Option<RuntimeVal> {
    match value {
        RuntimeVal::Nil => Some(RuntimeVal::Nil),
        RuntimeVal::Bool(value) => Some(RuntimeVal::Bool(*value)),
        RuntimeVal::Int(value) => Some(RuntimeVal::Int(*value)),
        RuntimeVal::Float(value) => Some(RuntimeVal::Float(*value)),
        RuntimeVal::ShortStr(value) => Some(RuntimeVal::ShortStr(*value)),
        RuntimeVal::Obj(_) => None,
    }
}

pub(super) fn owned_runtime_list_from_legacy(values: &[Val]) -> TypedList {
    let mut heap = HeapStore::new();
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        out.push(legacy_val_to_runtime_owned(value, &mut heap));
    }
    TypedList::OwnedRuntime(OwnedRuntimeList { values: out, heap })
}

pub(super) fn owned_runtime_map_from_legacy(values: &FastHashMap<ArcStr, Val>) -> TypedMap {
    let mut heap = HeapStore::new();
    let mut entries = BTreeMap::new();
    for (key, value) in values {
        entries.insert(
            RuntimeMapKey::String(Arc::<str>::from(key.as_str())),
            legacy_val_to_runtime_owned(value, &mut heap),
        );
    }
    TypedMap::OwnedRuntime(OwnedRuntimeMap { entries, heap })
}

fn legacy_val_to_runtime_owned(value: &Val, heap: &mut HeapStore) -> RuntimeVal {
    match value {
        Val::Nil => RuntimeVal::Nil,
        Val::Bool(value) => RuntimeVal::Bool(*value),
        Val::Int(value) => RuntimeVal::Int(*value),
        Val::Float(value) => RuntimeVal::Float(*value),
        Val::ShortStr(value) => RuntimeVal::ShortStr(*value),
        Val::Obj(value) => match value.as_ref() {
            HeapValue::String(value) => {
                if let Some(short) = ShortStr::new(value) {
                    RuntimeVal::ShortStr(short)
                } else {
                    RuntimeVal::Obj(heap.alloc(HeapValue::String(value.clone())))
                }
            }
            HeapValue::List(values) => {
                let values = values
                    .to_legacy_values()
                    .iter()
                    .map(|value| legacy_val_to_runtime_owned(value, heap))
                    .collect::<Vec<_>>();
                RuntimeVal::Obj(heap.alloc(HeapValue::List(TypedList::from_runtime_values(values, heap))))
            }
            HeapValue::Map(values) => {
                let mut entries = BTreeMap::new();
                for (key, value) in values.to_legacy_entries() {
                    entries.insert(
                        RuntimeMapKey::String(Arc::<str>::from(key.as_str())),
                        legacy_val_to_runtime_owned(&value, heap),
                    );
                }
                RuntimeVal::Obj(heap.alloc(HeapValue::Map(TypedMap::from_runtime_entries(entries))))
            }
            other => RuntimeVal::Obj(heap.alloc(other.clone())),
        },
    }
}

fn owned_runtime_val_to_val(value: &RuntimeVal, heap: &HeapStore) -> Val {
    match value {
        RuntimeVal::Nil => Val::Nil,
        RuntimeVal::Bool(value) => Val::Bool(*value),
        RuntimeVal::Int(value) => Val::Int(*value),
        RuntimeVal::Float(value) => Val::Float(*value),
        RuntimeVal::ShortStr(value) => Val::from(value.as_str()),
        RuntimeVal::Obj(handle) => heap
            .get(*handle)
            .map(|value| owned_heap_value_to_val_with_heap(value, heap))
            .unwrap_or(Val::Nil),
    }
}

fn owned_heap_value_to_val_with_heap(value: &HeapValue, heap: &HeapStore) -> Val {
    match value {
        HeapValue::String(value) => Val::from(value.as_ref()),
        HeapValue::List(values) => owned_typed_list_to_val(values, heap),
        HeapValue::Map(values) => owned_typed_map_to_val(values, heap),
        HeapValue::Object(RuntimeObject { type_name, fields }) => {
            let fields = fields
                .iter()
                .map(|(key, value)| (key.to_string(), owned_runtime_val_to_val(value, heap)))
                .collect();
            Val::object(type_name.as_ref(), fields)
        }
        other => Val::Obj(Arc::new(other.clone())),
    }
}

fn owned_typed_list_to_val(values: &TypedList, heap: &HeapStore) -> Val {
    let values = match values {
        TypedList::Mixed(values) => values
            .iter()
            .map(|value| owned_runtime_val_to_val(value, heap))
            .collect(),
        TypedList::Int(values) => values.iter().copied().map(Val::Int).collect(),
        TypedList::Float(values) => values.iter().copied().map(Val::Float).collect(),
        TypedList::Bool(values) => values.iter().copied().map(Val::Bool).collect(),
        TypedList::String(values) => values.iter().map(|value| Val::from(value.as_ref())).collect(),
        TypedList::OwnedRuntime(values) => values.to_legacy_values(),
    };
    Val::from(values)
}

fn owned_typed_map_to_val(values: &TypedMap, heap: &HeapStore) -> Val {
    let mut out = FastHashMap::default();
    for (key, value) in values.entries() {
        if let Some(key) = key.as_str() {
            out.insert(ArcStr::from(key), owned_runtime_val_to_val(&value, heap));
        }
    }
    Val::map(Arc::new(out))
}
