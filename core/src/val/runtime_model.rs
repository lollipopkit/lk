//! New runtime value model for the VM rewrite.
//!
//! The `Val` enum remains active while the compiler and executor are migrated.
//! New VM code should target these types first.

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Result;
use arcstr::ArcStr;

use crate::util::fast_map::FastHashMap;

use super::values::{AotFunction, ChannelValue, ShortStr, StreamCursorValue, StreamValue, TaskValue, Val};

mod heap;

pub use heap::{HeapRef, HeapStore};

#[derive(Clone, Debug, PartialEq)]
pub enum RuntimeVal {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    ShortStr(ShortStr),
    Obj(HeapRef),
}

impl Default for RuntimeVal {
    #[inline]
    fn default() -> Self {
        Self::Nil
    }
}

impl RuntimeVal {
    #[inline]
    pub const fn kind(&self) -> RuntimeValKind {
        match self {
            Self::Nil => RuntimeValKind::Nil,
            Self::Bool(_) => RuntimeValKind::Bool,
            Self::Int(_) => RuntimeValKind::Int,
            Self::Float(_) => RuntimeValKind::Float,
            Self::ShortStr(_) => RuntimeValKind::ShortStr,
            Self::Obj(_) => RuntimeValKind::Obj,
        }
    }

    #[inline]
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            _ => None,
        }
    }

    #[inline]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeValKind {
    Nil,
    Bool,
    Int,
    Float,
    ShortStr,
    Obj,
}

#[derive(Clone, Debug)]
pub enum HeapValue {
    String(Arc<str>),
    List(TypedList),
    Map(TypedMap),
    Callable(CallableValue),
    Task(Arc<TaskValue>),
    Channel(Arc<ChannelValue>),
    Stream(Arc<StreamValue>),
    StreamCursor(Arc<StreamCursorValue>),
    Object(RuntimeObject),
    UpvalCell(RuntimeVal),
    ErrorVal(ErrorVal),
}

impl HeapValue {
    #[inline]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::String(_) => "String",
            Self::List(_) => "List",
            Self::Map(_) => "Map",
            Self::Callable(_) => "Function",
            Self::Task(_) => "Task",
            Self::Channel(_) => "Channel",
            Self::Stream(_) => "Stream",
            Self::StreamCursor(_) => "StreamCursor",
            Self::Object(_) => "Object",
            Self::UpvalCell(_) => "UpvalCell",
            Self::ErrorVal(_) => "Error",
        }
    }
}

#[derive(Clone, Debug)]
pub enum CallableValue {
    Closure {
        function_index: u32,
        captures: Vec<RuntimeVal>,
    },
    RuntimeNative32 {
        arity: u16,
        function: crate::vm::NativeFunction32,
    },
    Runtime32(Arc<crate::vm::RuntimeCallable32>),
    Aot(AotFunction),
}

#[derive(Clone, Debug)]
pub struct RuntimeObject {
    pub type_name: Arc<str>,
    pub fields: BTreeMap<Arc<str>, RuntimeVal>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ErrorVal {
    pub message: Arc<str>,
    pub trace: Vec<RuntimeVal>,
}

#[derive(Clone, Debug)]
pub enum TypedList {
    Mixed(Vec<RuntimeVal>),
    Int(Vec<i64>),
    Float(Vec<f64>),
    Bool(Vec<bool>),
    String(Vec<Arc<str>>),
}

impl TypedList {
    pub fn to_val_values(&self) -> Vec<Val> {
        match self {
            Self::Mixed(values) => values.iter().map(Val::object_field_to_val).collect(),
            Self::Int(values) => values.iter().copied().map(Val::Int).collect(),
            Self::Float(values) => values.iter().copied().map(Val::Float).collect(),
            Self::Bool(values) => values.iter().copied().map(Val::Bool).collect(),
            Self::String(values) => values.iter().map(|value| Val::from(value.as_ref())).collect(),
        }
    }

    pub fn from_runtime_values(values: Vec<RuntimeVal>, heap: &HeapStore) -> Self {
        if values.is_empty() {
            return Self::Mixed(values);
        }

        let mut ints = Vec::with_capacity(values.len());
        let mut floats = Vec::with_capacity(values.len());
        let mut bools = Vec::with_capacity(values.len());
        let mut strings = Vec::with_capacity(values.len());
        for value in &values {
            match value {
                RuntimeVal::Int(value) if floats.is_empty() && bools.is_empty() && strings.is_empty() => {
                    ints.push(*value);
                }
                RuntimeVal::Float(value) if ints.is_empty() && bools.is_empty() && strings.is_empty() => {
                    floats.push(*value);
                }
                RuntimeVal::Bool(value) if ints.is_empty() && floats.is_empty() && strings.is_empty() => {
                    bools.push(*value);
                }
                RuntimeVal::ShortStr(value) if ints.is_empty() && floats.is_empty() && bools.is_empty() => {
                    strings.push(Arc::<str>::from(value.as_str()));
                }
                RuntimeVal::Obj(handle) if ints.is_empty() && floats.is_empty() && bools.is_empty() => {
                    let Some(HeapValue::String(value)) = heap.get(*handle) else {
                        return Self::Mixed(values);
                    };
                    strings.push(value.clone());
                }
                _ => return Self::Mixed(values),
            }
        }

        if !ints.is_empty() {
            Self::Int(ints)
        } else if !floats.is_empty() {
            Self::Float(floats)
        } else if !bools.is_empty() {
            Self::Bool(bools)
        } else {
            Self::String(strings)
        }
    }

    pub fn from_runtime_slice(values: &[RuntimeVal], heap: &HeapStore) -> Self {
        if values.is_empty() {
            return Self::Mixed(Vec::new());
        }

        let mut ints = Vec::with_capacity(values.len());
        let mut floats = Vec::with_capacity(values.len());
        let mut bools = Vec::with_capacity(values.len());
        let mut strings = Vec::with_capacity(values.len());
        for value in values {
            match value {
                RuntimeVal::Int(value) if floats.is_empty() && bools.is_empty() && strings.is_empty() => {
                    ints.push(*value);
                }
                RuntimeVal::Float(value) if ints.is_empty() && bools.is_empty() && strings.is_empty() => {
                    floats.push(*value);
                }
                RuntimeVal::Bool(value) if ints.is_empty() && floats.is_empty() && strings.is_empty() => {
                    bools.push(*value);
                }
                RuntimeVal::ShortStr(value) if ints.is_empty() && floats.is_empty() && bools.is_empty() => {
                    strings.push(Arc::<str>::from(value.as_str()));
                }
                RuntimeVal::Obj(handle) if ints.is_empty() && floats.is_empty() && bools.is_empty() => {
                    let Some(HeapValue::String(value)) = heap.get(*handle) else {
                        return Self::Mixed(values.to_vec());
                    };
                    strings.push(value.clone());
                }
                _ => return Self::Mixed(values.to_vec()),
            }
        }

        if !ints.is_empty() {
            Self::Int(ints)
        } else if !floats.is_empty() {
            Self::Float(floats)
        } else if !bools.is_empty() {
            Self::Bool(bools)
        } else {
            Self::String(strings)
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        match self {
            Self::Mixed(values) => values.len(),
            Self::Int(values) => values.len(),
            Self::Float(values) => values.len(),
            Self::Bool(values) => values.len(),
            Self::String(values) => values.len(),
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn materialize_mixed(self, heap: &mut HeapStore) -> Vec<RuntimeVal> {
        match self {
            Self::Mixed(values) => values,
            Self::Int(values) => values.into_iter().map(RuntimeVal::Int).collect(),
            Self::Float(values) => values.into_iter().map(RuntimeVal::Float).collect(),
            Self::Bool(values) => values.into_iter().map(RuntimeVal::Bool).collect(),
            Self::String(values) => values
                .into_iter()
                .map(|value| {
                    if let Some(short) = ShortStr::new(&value) {
                        RuntimeVal::ShortStr(short)
                    } else {
                        RuntimeVal::Obj(heap.alloc(HeapValue::String(value)))
                    }
                })
                .collect(),
        }
    }

    pub fn runtime_values_into_heap(&self, heap: &mut HeapStore) -> Result<Vec<RuntimeVal>> {
        Ok(match self {
            Self::Mixed(values) => values.clone(),
            Self::Int(values) => values.iter().copied().map(RuntimeVal::Int).collect(),
            Self::Float(values) => values.iter().copied().map(RuntimeVal::Float).collect(),
            Self::Bool(values) => values.iter().copied().map(RuntimeVal::Bool).collect(),
            Self::String(values) => values
                .iter()
                .map(|value| {
                    if let Some(short) = ShortStr::new(value) {
                        RuntimeVal::ShortStr(short)
                    } else {
                        RuntimeVal::Obj(heap.alloc(HeapValue::String(value.clone())))
                    }
                })
                .collect(),
        })
    }

    pub fn slice_from(&self, start: usize) -> Self {
        match self {
            Self::Mixed(values) => Self::Mixed(values.get(start..).unwrap_or(&[]).to_vec()),
            Self::Int(values) => Self::Int(values.get(start..).unwrap_or(&[]).to_vec()),
            Self::Float(values) => Self::Float(values.get(start..).unwrap_or(&[]).to_vec()),
            Self::Bool(values) => Self::Bool(values.get(start..).unwrap_or(&[]).to_vec()),
            Self::String(values) => Self::String(values.get(start..).unwrap_or(&[]).to_vec()),
        }
    }
}

impl PartialEq for TypedList {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Mixed(left), Self::Mixed(right)) => left == right,
            (Self::Int(left), Self::Int(right)) => left == right,
            (Self::Float(left), Self::Float(right)) => left == right,
            (Self::Bool(left), Self::Bool(right)) => left == right,
            (Self::String(left), Self::String(right)) => left == right,
            _ => self.to_val_values() == other.to_val_values(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum TypedMap {
    Mixed(BTreeMap<RuntimeMapKey, RuntimeVal>),
    StringMixed(BTreeMap<Arc<str>, RuntimeVal>),
    StringInt(BTreeMap<Arc<str>, i64>),
    StringFloat(BTreeMap<Arc<str>, f64>),
    StringBool(BTreeMap<Arc<str>, bool>),
}

impl TypedMap {
    pub fn to_val_entries(&self) -> FastHashMap<ArcStr, Val> {
        let mut out = FastHashMap::default();
        for (key, value) in self.entries() {
            let Some(key) = key.as_str() else {
                continue;
            };
            out.insert(ArcStr::from(key), Val::object_field_to_val(&value));
        }
        out
    }

    pub fn from_runtime_entries(entries: BTreeMap<RuntimeMapKey, RuntimeVal>) -> Self {
        if entries.is_empty() {
            return Self::Mixed(entries);
        }

        let mut mixed = BTreeMap::new();
        let mut ints = BTreeMap::new();
        let mut floats = BTreeMap::new();
        let mut bools = BTreeMap::new();
        for (key, value) in &entries {
            let Some(key) = runtime_map_key_as_string(key) else {
                return Self::Mixed(entries);
            };
            match value {
                RuntimeVal::Int(value) if mixed.is_empty() && floats.is_empty() && bools.is_empty() => {
                    ints.insert(key, *value);
                }
                RuntimeVal::Float(value) if mixed.is_empty() && ints.is_empty() && bools.is_empty() => {
                    floats.insert(key, *value);
                }
                RuntimeVal::Bool(value) if mixed.is_empty() && ints.is_empty() && floats.is_empty() => {
                    bools.insert(key, *value);
                }
                value if ints.is_empty() && floats.is_empty() && bools.is_empty() => {
                    mixed.insert(key, value.clone());
                }
                _ => return Self::StringMixed(entries_to_string_mixed(entries)),
            }
        }

        if !ints.is_empty() {
            Self::StringInt(ints)
        } else if !floats.is_empty() {
            Self::StringFloat(floats)
        } else if !bools.is_empty() {
            Self::StringBool(bools)
        } else {
            Self::StringMixed(mixed)
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        match self {
            Self::Mixed(values) => values.len(),
            Self::StringMixed(values) => values.len(),
            Self::StringInt(values) => values.len(),
            Self::StringFloat(values) => values.len(),
            Self::StringBool(values) => values.len(),
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, key: &RuntimeMapKey) -> Option<RuntimeVal> {
        match self {
            Self::Mixed(values) => values.get(key).cloned(),
            Self::StringMixed(values) => key.as_str().and_then(|key| values.get(key).cloned()),
            Self::StringInt(values) => key
                .as_str()
                .and_then(|key| values.get(key).copied().map(RuntimeVal::Int)),
            Self::StringFloat(values) => key
                .as_str()
                .and_then(|key| values.get(key).copied().map(RuntimeVal::Float)),
            Self::StringBool(values) => key
                .as_str()
                .and_then(|key| values.get(key).copied().map(RuntimeVal::Bool)),
        }
    }

    pub fn get_into_heap(&self, key: &RuntimeMapKey, heap: &mut HeapStore) -> Result<Option<RuntimeVal>> {
        let _ = heap;
        Ok(self.get(key))
    }

    pub fn get_str(&self, key: &str) -> Option<RuntimeVal> {
        match self {
            Self::Mixed(values) => {
                if let Some(value) =
                    ShortStr::new(key).and_then(|key| values.get(&RuntimeMapKey::ShortStr(key)).cloned())
                {
                    return Some(value);
                }
                values.get(&RuntimeMapKey::String(Arc::<str>::from(key))).cloned()
            }
            Self::StringMixed(values) => values.get(key).cloned(),
            Self::StringInt(values) => values.get(key).copied().map(RuntimeVal::Int),
            Self::StringFloat(values) => values.get(key).copied().map(RuntimeVal::Float),
            Self::StringBool(values) => values.get(key).copied().map(RuntimeVal::Bool),
        }
    }

    pub fn get_str_into_heap(&self, key: &str, heap: &mut HeapStore) -> Result<Option<RuntimeVal>> {
        let _ = heap;
        Ok(self.get_str(key))
    }

    pub fn set(&mut self, key: RuntimeMapKey, value: RuntimeVal) {
        match self {
            Self::Mixed(values) => {
                values.insert(key, value);
            }
            Self::StringMixed(values) => {
                if let Some(key) = key.as_arc_str() {
                    values.insert(key, value);
                } else {
                    self.materialize_string_map_to_mixed(key, value);
                }
            }
            Self::StringInt(values) => {
                if let Some(key) = key.as_arc_str() {
                    match value {
                        RuntimeVal::Int(value) => {
                            values.insert(key, value);
                        }
                        value => {
                            let mut mixed = values
                                .iter()
                                .map(|(key, value)| (key.clone(), RuntimeVal::Int(*value)))
                                .collect::<BTreeMap<_, _>>();
                            mixed.insert(key, value);
                            *self = Self::StringMixed(mixed);
                        }
                    }
                } else {
                    self.materialize_string_map_to_mixed(key, value);
                }
            }
            Self::StringFloat(values) => {
                if let Some(key) = key.as_arc_str() {
                    match value {
                        RuntimeVal::Float(value) => {
                            values.insert(key, value);
                        }
                        value => {
                            let mut mixed = values
                                .iter()
                                .map(|(key, value)| (key.clone(), RuntimeVal::Float(*value)))
                                .collect::<BTreeMap<_, _>>();
                            mixed.insert(key, value);
                            *self = Self::StringMixed(mixed);
                        }
                    }
                } else {
                    self.materialize_string_map_to_mixed(key, value);
                }
            }
            Self::StringBool(values) => {
                if let Some(key) = key.as_arc_str() {
                    match value {
                        RuntimeVal::Bool(value) => {
                            values.insert(key, value);
                        }
                        value => {
                            let mut mixed = values
                                .iter()
                                .map(|(key, value)| (key.clone(), RuntimeVal::Bool(*value)))
                                .collect::<BTreeMap<_, _>>();
                            mixed.insert(key, value);
                            *self = Self::StringMixed(mixed);
                        }
                    }
                } else {
                    self.materialize_string_map_to_mixed(key, value);
                }
            }
        }
    }

    pub fn entries(&self) -> Vec<(RuntimeMapKey, RuntimeVal)> {
        match self {
            Self::Mixed(values) => values.iter().map(|(key, value)| (key.clone(), value.clone())).collect(),
            Self::StringMixed(values) => values
                .iter()
                .map(|(key, value)| (RuntimeMapKey::String(key.clone()), value.clone()))
                .collect(),
            Self::StringInt(values) => values
                .iter()
                .map(|(key, value)| (RuntimeMapKey::String(key.clone()), RuntimeVal::Int(*value)))
                .collect(),
            Self::StringFloat(values) => values
                .iter()
                .map(|(key, value)| (RuntimeMapKey::String(key.clone()), RuntimeVal::Float(*value)))
                .collect(),
            Self::StringBool(values) => values
                .iter()
                .map(|(key, value)| (RuntimeMapKey::String(key.clone()), RuntimeVal::Bool(*value)))
                .collect(),
        }
    }

    pub fn entries_into_heap(&self, heap: &mut HeapStore) -> Result<Vec<(RuntimeMapKey, RuntimeVal)>> {
        let _ = heap;
        Ok(self.entries())
    }

    /// Collect all entries as `(Arc<str>, RuntimeVal)` pairs without needing `&mut HeapStore`.
    pub fn string_entries_no_heap(&self) -> Result<Vec<(Arc<str>, RuntimeVal)>> {
        Ok(match self {
            Self::StringMixed(values) => values.iter().map(|(key, value)| (key.clone(), value.clone())).collect(),
            Self::StringInt(values) => values
                .iter()
                .map(|(key, value)| (key.clone(), RuntimeVal::Int(*value)))
                .collect(),
            Self::StringFloat(values) => values
                .iter()
                .map(|(key, value)| (key.clone(), RuntimeVal::Float(*value)))
                .collect(),
            Self::StringBool(values) => values
                .iter()
                .map(|(key, value)| (key.clone(), RuntimeVal::Bool(*value)))
                .collect(),
            Self::Mixed(values) => values
                .iter()
                .map(|(key, value)| {
                    key.as_arc_str()
                        .map(|key| (key, value.clone()))
                        .ok_or_else(|| anyhow::anyhow!("map contains non-string key"))
                })
                .collect::<Result<_>>()?,
        })
    }

    pub fn string_entries_into_heap(&self, heap: &mut HeapStore) -> Result<Vec<(Arc<str>, RuntimeVal)>> {
        let _ = heap;
        Ok(match self {
            Self::StringMixed(values) => values.iter().map(|(key, value)| (key.clone(), value.clone())).collect(),
            Self::StringInt(values) => values
                .iter()
                .map(|(key, value)| (key.clone(), RuntimeVal::Int(*value)))
                .collect(),
            Self::StringFloat(values) => values
                .iter()
                .map(|(key, value)| (key.clone(), RuntimeVal::Float(*value)))
                .collect(),
            Self::StringBool(values) => values
                .iter()
                .map(|(key, value)| (key.clone(), RuntimeVal::Bool(*value)))
                .collect(),
            Self::Mixed(values) => values
                .iter()
                .map(|(key, value)| {
                    key.as_arc_str()
                        .map(|key| (key, value.clone()))
                        .ok_or_else(|| anyhow::anyhow!("map contains non-string key"))
                })
                .collect::<Result<_>>()?,
        })
    }

    fn materialize_string_map_to_mixed(&mut self, key: RuntimeMapKey, value: RuntimeVal) {
        let mut mixed = match std::mem::replace(self, Self::Mixed(BTreeMap::new())) {
            Self::Mixed(values) => values,
            Self::StringMixed(values) => values
                .into_iter()
                .map(|(key, value)| (RuntimeMapKey::String(key), value))
                .collect(),
            Self::StringInt(values) => values
                .into_iter()
                .map(|(key, value)| (RuntimeMapKey::String(key), RuntimeVal::Int(value)))
                .collect(),
            Self::StringFloat(values) => values
                .into_iter()
                .map(|(key, value)| (RuntimeMapKey::String(key), RuntimeVal::Float(value)))
                .collect(),
            Self::StringBool(values) => values
                .into_iter()
                .map(|(key, value)| (RuntimeMapKey::String(key), RuntimeVal::Bool(value)))
                .collect(),
        };
        mixed.insert(key, value);
        *self = Self::Mixed(mixed);
    }
}

impl PartialEq for TypedMap {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Mixed(left), Self::Mixed(right)) => left == right,
            (Self::StringMixed(left), Self::StringMixed(right)) => left == right,
            (Self::StringInt(left), Self::StringInt(right)) => left == right,
            (Self::StringFloat(left), Self::StringFloat(right)) => left == right,
            (Self::StringBool(left), Self::StringBool(right)) => left == right,
            _ => self.to_val_entries() == other.to_val_entries(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuntimeMapKey {
    Nil,
    Bool(bool),
    Int(i64),
    ShortStr(ShortStr),
    String(Arc<str>),
    Obj(HeapRef),
}

impl RuntimeMapKey {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::ShortStr(value) => Some(value.as_str()),
            Self::String(value) => Some(value.as_ref()),
            _ => None,
        }
    }

    pub fn as_arc_str(&self) -> Option<Arc<str>> {
        match self {
            Self::ShortStr(value) => Some(Arc::<str>::from(value.as_str())),
            Self::String(value) => Some(value.clone()),
            _ => None,
        }
    }
}

fn runtime_map_key_as_string(key: &RuntimeMapKey) -> Option<Arc<str>> {
    key.as_arc_str()
}

fn entries_to_string_mixed(entries: BTreeMap<RuntimeMapKey, RuntimeVal>) -> BTreeMap<Arc<str>, RuntimeVal> {
    entries
        .into_iter()
        .filter_map(|(key, value)| runtime_map_key_as_string(&key).map(|key| (key, value)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_int_list_materializes_to_runtime_values() {
        let mut heap = HeapStore::new();
        let values = TypedList::Int(vec![1, 2, 3]).materialize_mixed(&mut heap);

        assert_eq!(values.len(), 3);
        assert_eq!(values[0].as_int(), Some(1));
        assert_eq!(values[2].as_int(), Some(3));
        assert!(heap.is_empty());
    }

    #[test]
    fn long_string_list_materialization_allocates_heap_object() {
        let mut heap = HeapStore::new();
        let values = TypedList::String(vec![Arc::<str>::from("short"), Arc::<str>::from("longer-than-seven")])
            .materialize_mixed(&mut heap);

        assert_eq!(values[0].kind(), RuntimeValKind::ShortStr);
        assert_eq!(values[1].kind(), RuntimeValKind::Obj);
        assert_eq!(heap.len(), 1);
    }

    #[test]
    fn runtime_values_materialize_to_typed_lists() {
        let mut heap = HeapStore::new();
        let long = heap.alloc(HeapValue::String(Arc::<str>::from("longer-than-seven")));

        assert_eq!(
            TypedList::from_runtime_values(vec![RuntimeVal::Int(1), RuntimeVal::Int(2)], &heap),
            TypedList::Int(vec![1, 2])
        );
        assert_eq!(
            TypedList::from_runtime_values(vec![RuntimeVal::Bool(true), RuntimeVal::Bool(false)], &heap),
            TypedList::Bool(vec![true, false])
        );
        assert_eq!(
            TypedList::from_runtime_values(
                vec![
                    RuntimeVal::ShortStr(ShortStr::new("short").expect("short")),
                    RuntimeVal::Obj(long),
                ],
                &heap,
            ),
            TypedList::String(vec![Arc::<str>::from("short"), Arc::<str>::from("longer-than-seven")])
        );

        assert!(matches!(
            TypedList::from_runtime_values(vec![RuntimeVal::Int(1), RuntimeVal::Bool(true)], &heap),
            TypedList::Mixed(_)
        ));
    }

    #[test]
    fn runtime_slices_materialize_to_typed_lists_without_precloning() {
        let mut heap = HeapStore::new();
        let long = heap.alloc(HeapValue::String(Arc::<str>::from("longer-than-seven")));

        assert_eq!(
            TypedList::from_runtime_slice(&[RuntimeVal::Int(1), RuntimeVal::Int(2)], &heap),
            TypedList::Int(vec![1, 2])
        );
        assert_eq!(
            TypedList::from_runtime_slice(
                &[
                    RuntimeVal::ShortStr(ShortStr::new("short").expect("short")),
                    RuntimeVal::Obj(long),
                ],
                &heap,
            ),
            TypedList::String(vec![Arc::<str>::from("short"), Arc::<str>::from("longer-than-seven")])
        );
        assert!(matches!(
            TypedList::from_runtime_slice(&[RuntimeVal::Int(1), RuntimeVal::Bool(true)], &heap),
            TypedList::Mixed(_)
        ));
    }

    #[test]
    fn runtime_entries_materialize_to_typed_string_maps() {
        let mut entries = BTreeMap::new();
        entries.insert(RuntimeMapKey::String(Arc::<str>::from("answer")), RuntimeVal::Int(42));

        assert!(matches!(
            TypedMap::from_runtime_entries(entries),
            TypedMap::StringInt(values) if values.get("answer") == Some(&42)
        ));

        let mut entries = BTreeMap::new();
        entries.insert(
            RuntimeMapKey::ShortStr(ShortStr::new("ok").expect("short")),
            RuntimeVal::Bool(true),
        );
        assert!(matches!(
            TypedMap::from_runtime_entries(entries),
            TypedMap::StringBool(values) if values.get("ok") == Some(&true)
        ));

        let mut entries = BTreeMap::new();
        entries.insert(RuntimeMapKey::Int(1), RuntimeVal::Int(42));
        assert!(matches!(TypedMap::from_runtime_entries(entries), TypedMap::Mixed(_)));
    }

    #[test]
    fn typed_map_string_entries_into_heap_rejects_non_string_keys_without_materializing_twice() {
        let mut heap = HeapStore::new();
        let entries = TypedMap::StringInt(BTreeMap::from([(Arc::<str>::from("answer"), 42)]))
            .string_entries_into_heap(&mut heap)
            .expect("string entries");
        assert_eq!(entries, vec![(Arc::<str>::from("answer"), RuntimeVal::Int(42))]);
        assert!(heap.is_empty());

        let mixed = TypedMap::Mixed(BTreeMap::from([(RuntimeMapKey::Int(1), RuntimeVal::Bool(true))]));
        assert!(mixed.string_entries_into_heap(&mut heap).is_err());
    }

    #[test]
    fn typed_map_get_and_set_preserve_specialized_backing_until_polluted() {
        let mut map = TypedMap::StringInt(BTreeMap::from([(Arc::<str>::from("answer"), 41)]));

        assert_eq!(
            map.get(&RuntimeMapKey::ShortStr(ShortStr::new("answer").expect("short"))),
            Some(RuntimeVal::Int(41))
        );

        map.set(RuntimeMapKey::String(Arc::<str>::from("answer")), RuntimeVal::Int(42));
        assert!(matches!(map, TypedMap::StringInt(_)));
        assert_eq!(map.get_str("answer"), Some(RuntimeVal::Int(42)));
        assert_eq!(
            map.get(&RuntimeMapKey::String(Arc::<str>::from("answer"))),
            Some(RuntimeVal::Int(42))
        );

        map.set(
            RuntimeMapKey::String(Arc::<str>::from("answer")),
            RuntimeVal::Bool(true),
        );
        assert!(matches!(map, TypedMap::StringMixed(_)));
        assert_eq!(
            map.get(&RuntimeMapKey::String(Arc::<str>::from("answer"))),
            Some(RuntimeVal::Bool(true))
        );
    }

    #[test]
    fn typed_map_set_materializes_to_mixed_for_non_string_key() {
        let mut map = TypedMap::StringBool(BTreeMap::from([(Arc::<str>::from("ok"), true)]));

        map.set(RuntimeMapKey::Int(7), RuntimeVal::Bool(false));

        assert!(matches!(map, TypedMap::Mixed(_)));
        assert_eq!(map.get_str("ok"), Some(RuntimeVal::Bool(true)));
        assert_eq!(map.get(&RuntimeMapKey::Int(7)), Some(RuntimeVal::Bool(false)));
        assert_eq!(
            map.get(&RuntimeMapKey::String(Arc::<str>::from("ok"))),
            Some(RuntimeVal::Bool(true))
        );
    }
}
