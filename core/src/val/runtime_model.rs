//! New runtime value model for the VM rewrite.
//!
//! The legacy `Val` enum remains active while the compiler and executor are
//! migrated. New VM code should target these types first.

use std::collections::BTreeMap;
use std::sync::Arc;

use arcstr::ArcStr;

use crate::util::fast_map::FastHashMap;

use super::values::{AotFunction, ChannelValue, ShortStr, StreamCursorValue, StreamValue, TaskValue, Val};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HeapRef(u32);

impl HeapRef {
    #[inline]
    pub const fn new(index: u32) -> Self {
        Self(index)
    }

    #[inline]
    pub const fn index(self) -> u32 {
        self.0
    }
}

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
pub struct HeapStore {
    slots: Vec<Option<HeapValue>>,
    marks: Vec<u8>,
    free_list: Vec<u32>,
    live_len: usize,
    alloc_since_gc: u32,
    gc_threshold: u32,
}

impl HeapStore {
    const WHITE: u8 = 0;
    const BLACK: u8 = 2;
    pub const DEFAULT_GC_THRESHOLD: u32 = 1024;

    #[inline]
    pub const fn new() -> Self {
        Self {
            slots: Vec::new(),
            marks: Vec::new(),
            free_list: Vec::new(),
            live_len: 0,
            alloc_since_gc: 0,
            gc_threshold: Self::DEFAULT_GC_THRESHOLD,
        }
    }

    #[inline]
    pub fn alloc(&mut self, value: HeapValue) -> HeapRef {
        let index = if let Some(index) = self.free_list.pop() {
            self.slots[index as usize] = Some(value);
            self.marks[index as usize] = Self::WHITE;
            index
        } else {
            let index = self.slots.len();
            assert!(u32::try_from(index).is_ok(), "heap object index overflow");
            self.slots.push(Some(value));
            self.marks.push(Self::WHITE);
            index as u32
        };
        self.live_len += 1;
        self.alloc_since_gc = self.alloc_since_gc.saturating_add(1);
        HeapRef::new(index)
    }

    #[inline]
    pub fn get(&self, reference: HeapRef) -> Option<&HeapValue> {
        self.slots.get(reference.index() as usize)?.as_ref()
    }

    #[inline]
    pub fn get_mut(&mut self, reference: HeapRef) -> Option<&mut HeapValue> {
        self.slots.get_mut(reference.index() as usize)?.as_mut()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.live_len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.live_len == 0
    }

    #[inline]
    pub fn should_collect(&self) -> bool {
        self.alloc_since_gc >= self.gc_threshold
    }

    #[inline]
    pub fn gc_threshold(&self) -> u32 {
        self.gc_threshold
    }

    #[inline]
    pub fn set_gc_threshold(&mut self, threshold: u32) {
        self.gc_threshold = threshold.max(1);
    }

    #[inline]
    pub fn roots_from_values<'a>(values: impl IntoIterator<Item = &'a RuntimeVal>) -> Vec<HeapRef> {
        values
            .into_iter()
            .filter_map(|value| match value {
                RuntimeVal::Obj(reference) => Some(*reference),
                _ => None,
            })
            .collect()
    }

    pub fn collect(&mut self, roots: impl IntoIterator<Item = HeapRef>) {
        for mark in &mut self.marks {
            *mark = Self::WHITE;
        }
        for root in roots {
            self.mark_ref(root);
        }
        self.sweep();
        self.alloc_since_gc = 0;
    }

    fn mark_ref(&mut self, reference: HeapRef) {
        let index = reference.index() as usize;
        let Some(slot) = self.slots.get(index) else {
            return;
        };
        if slot.is_none() || self.marks.get(index).copied() == Some(Self::BLACK) {
            return;
        }
        self.marks[index] = Self::BLACK;
        let value = slot.as_ref().expect("checked live slot").clone();
        self.mark_heap_value(value);
    }

    fn mark_heap_value(&mut self, value: HeapValue) {
        match value {
            HeapValue::String(_)
            | HeapValue::Task(_)
            | HeapValue::Channel(_)
            | HeapValue::Stream(_)
            | HeapValue::StreamCursor(_) => {}
            HeapValue::List(values) => self.mark_typed_list(values),
            HeapValue::Map(values) => self.mark_typed_map(values),
            HeapValue::Object(object) => {
                for value in object.fields.values() {
                    self.mark_runtime_value(value);
                }
            }
            HeapValue::Callable(CallableValue::Closure { captures, .. }) => {
                for value in &captures {
                    self.mark_runtime_value(value);
                }
            }
            HeapValue::Callable(CallableValue::Runtime32(function)) => {
                if let Ok(mut state) = function.state.lock() {
                    state.collect_garbage(function.captures.iter());
                }
            }
            HeapValue::Callable(
                CallableValue::RuntimeNative32 { .. } | CallableValue::Native { .. } | CallableValue::Aot(_),
            ) => {}
            HeapValue::UpvalCell(value) => self.mark_runtime_value(&value),
            HeapValue::ErrorVal(error) => {
                for value in &error.trace {
                    self.mark_runtime_value(value);
                }
            }
        }
    }

    fn mark_typed_list(&mut self, values: TypedList) {
        if let TypedList::Mixed(values) = values {
            for value in &values {
                self.mark_runtime_value(value);
            }
        }
    }

    fn mark_typed_map(&mut self, values: TypedMap) {
        match values {
            TypedMap::Mixed(values) => {
                for (key, value) in &values {
                    self.mark_runtime_map_key(key);
                    self.mark_runtime_value(value);
                }
            }
            TypedMap::StringMixed(values) => {
                for value in values.values() {
                    self.mark_runtime_value(value);
                }
            }
            TypedMap::StringInt(_) | TypedMap::StringFloat(_) | TypedMap::StringBool(_) => {}
        }
    }

    fn mark_runtime_map_key(&mut self, key: &RuntimeMapKey) {
        if let RuntimeMapKey::Obj(reference) = key {
            self.mark_ref(*reference);
        }
    }

    fn mark_runtime_value(&mut self, value: &RuntimeVal) {
        if let RuntimeVal::Obj(reference) = value {
            self.mark_ref(*reference);
        }
    }

    fn sweep(&mut self) {
        self.free_list.clear();
        let mut live_len = 0;
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.is_none() {
                self.free_list.push(index as u32);
                continue;
            }
            if self.marks[index] == Self::BLACK {
                self.marks[index] = Self::WHITE;
                live_len += 1;
            } else {
                *slot = None;
                self.free_list.push(index as u32);
            }
        }
        self.live_len = live_len;
    }
}

impl Default for HeapStore {
    fn default() -> Self {
        Self::new()
    }
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
    Native {
        function_index: u32,
        arity: u16,
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

#[derive(Clone, Debug, PartialEq)]
pub enum TypedList {
    Mixed(Vec<RuntimeVal>),
    Int(Vec<i64>),
    Float(Vec<f64>),
    Bool(Vec<bool>),
    String(Vec<Arc<str>>),
}

impl TypedList {
    pub fn from_legacy_values(values: &[Val]) -> Self {
        if values.is_empty() {
            return Self::Mixed(Vec::new());
        }

        let mut ints = Vec::with_capacity(values.len());
        let mut floats = Vec::with_capacity(values.len());
        let mut bools = Vec::with_capacity(values.len());
        let mut strings = Vec::with_capacity(values.len());
        for value in values {
            match value {
                Val::Int(value) if floats.is_empty() && bools.is_empty() && strings.is_empty() => ints.push(*value),
                Val::Float(value) if ints.is_empty() && bools.is_empty() && strings.is_empty() => floats.push(*value),
                Val::Bool(value) if ints.is_empty() && floats.is_empty() && strings.is_empty() => bools.push(*value),
                value if ints.is_empty() && floats.is_empty() && bools.is_empty() => {
                    let Some(value) = value.as_str() else {
                        return Self::Mixed(values.iter().cloned().map(Val::val_to_object_field).collect());
                    };
                    strings.push(Arc::<str>::from(value));
                }
                _ => return Self::Mixed(values.iter().cloned().map(Val::val_to_object_field).collect()),
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

    pub fn to_legacy_values(&self) -> Vec<Val> {
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

#[derive(Clone, Debug, PartialEq)]
pub enum TypedMap {
    Mixed(BTreeMap<RuntimeMapKey, RuntimeVal>),
    StringMixed(BTreeMap<Arc<str>, RuntimeVal>),
    StringInt(BTreeMap<Arc<str>, i64>),
    StringFloat(BTreeMap<Arc<str>, f64>),
    StringBool(BTreeMap<Arc<str>, bool>),
}

impl TypedMap {
    pub fn from_legacy_entries(values: &FastHashMap<ArcStr, Val>) -> Self {
        let entries = values
            .iter()
            .map(|(key, value)| {
                (
                    RuntimeMapKey::String(Arc::<str>::from(key.as_str())),
                    Val::val_to_object_field(value.clone()),
                )
            })
            .collect();
        Self::from_runtime_entries(entries)
    }

    pub fn to_legacy_entries(&self) -> FastHashMap<ArcStr, Val> {
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
    fn heap_store_returns_stable_refs() {
        let mut heap = HeapStore::new();
        let name = heap.alloc(HeapValue::String(Arc::<str>::from("customer")));

        assert_eq!(name.index(), 0);
        assert_eq!(heap.len(), 1);
        assert!(matches!(heap.get(name), Some(HeapValue::String(text)) if text.as_ref() == "customer"));
    }

    #[test]
    fn heap_store_reuses_collected_slots_and_dangling_refs_return_none() {
        let mut heap = HeapStore::new();
        let live = heap.alloc(HeapValue::String(Arc::<str>::from("live")));
        let dead = heap.alloc(HeapValue::String(Arc::<str>::from("dead")));

        heap.collect([live]);

        assert_eq!(heap.len(), 1);
        assert!(heap.get(live).is_some());
        assert_eq!(heap.get(dead).map(HeapValue::type_name), None);

        let reused = heap.alloc(HeapValue::String(Arc::<str>::from("reused")));
        assert_eq!(reused.index(), dead.index());
        assert_eq!(heap.len(), 2);
        assert!(matches!(heap.get(reused), Some(HeapValue::String(value)) if value.as_ref() == "reused"));
    }

    #[test]
    fn heap_store_tracks_gc_threshold_without_collecting_implicitly() {
        let mut heap = HeapStore::new();
        heap.set_gc_threshold(2);

        assert_eq!(heap.gc_threshold(), 2);
        assert!(!heap.should_collect());

        let first = heap.alloc(HeapValue::String(Arc::<str>::from("first")));
        assert!(!heap.should_collect());
        let second = heap.alloc(HeapValue::String(Arc::<str>::from("second")));
        assert!(heap.should_collect());

        heap.collect([first, second]);

        assert!(!heap.should_collect());
        assert_eq!(heap.len(), 2);
    }

    #[test]
    fn heap_store_gc_marks_nested_runtime_refs() {
        let mut heap = HeapStore::new();
        let leaf = heap.alloc(HeapValue::String(Arc::<str>::from("leaf")));
        let list = heap.alloc(HeapValue::List(TypedList::Mixed(vec![RuntimeVal::Obj(leaf)])));
        let map = heap.alloc(HeapValue::Map(TypedMap::StringMixed(BTreeMap::from([(
            Arc::<str>::from("list"),
            RuntimeVal::Obj(list),
        )]))));
        let object = heap.alloc(HeapValue::Object(RuntimeObject {
            type_name: Arc::<str>::from("Box"),
            fields: BTreeMap::from([(Arc::<str>::from("map"), RuntimeVal::Obj(map))]),
        }));
        let closure = heap.alloc(HeapValue::Callable(CallableValue::Closure {
            function_index: 7,
            captures: vec![RuntimeVal::Obj(object)],
        }));
        let cell = heap.alloc(HeapValue::UpvalCell(RuntimeVal::Obj(closure)));
        let error = heap.alloc(HeapValue::ErrorVal(ErrorVal {
            message: Arc::<str>::from("boom"),
            trace: vec![RuntimeVal::Obj(cell)],
        }));
        let garbage = heap.alloc(HeapValue::String(Arc::<str>::from("garbage")));

        heap.collect([error]);

        for handle in [leaf, list, map, object, closure, cell, error] {
            assert!(
                heap.get(handle).is_some(),
                "live handle {} should survive",
                handle.index()
            );
        }
        assert!(heap.get(garbage).is_none());
    }

    #[test]
    fn heap_store_gc_marks_mixed_map_object_keys() {
        let mut heap = HeapStore::new();
        let key_object = heap.alloc(HeapValue::String(Arc::<str>::from("key-object")));
        let map = heap.alloc(HeapValue::Map(TypedMap::Mixed(BTreeMap::from([(
            RuntimeMapKey::Obj(key_object),
            RuntimeVal::Int(1),
        )]))));

        heap.collect([map]);

        assert!(heap.get(map).is_some());
        assert!(heap.get(key_object).is_some());
    }

    #[test]
    fn heap_store_gc_collects_runtime32_callable_shared_state_without_marking_dest_heap_captures() {
        let mut source_heap = HeapStore::new();
        let source_capture = source_heap.alloc(HeapValue::String(Arc::<str>::from("source-capture")));
        let source_garbage = source_heap.alloc(HeapValue::String(Arc::<str>::from("source-garbage")));
        let callable = crate::vm::RuntimeCallable32::new(
            Arc::new(crate::vm::Module32::default()),
            0,
            vec![RuntimeVal::Obj(source_capture)],
            source_heap,
            Vec::new(),
        );

        let mut dest_heap = HeapStore::new();
        let same_index_garbage = dest_heap.alloc(HeapValue::String(Arc::<str>::from("dest-garbage")));
        assert_eq!(same_index_garbage.index(), source_capture.index());
        let callable_handle = dest_heap.alloc(HeapValue::Callable(CallableValue::Runtime32(Arc::new(
            callable.clone(),
        ))));

        dest_heap.collect([callable_handle]);
        let state = callable.state.lock().expect("runtime callable state");

        assert!(dest_heap.get(callable_handle).is_some());
        assert!(
            dest_heap.get(same_index_garbage).is_none(),
            "source capture handle must not mark same-index object in destination heap"
        );
        assert!(state.heap.get(source_capture).is_some());
        assert!(state.heap.get(source_garbage).is_none());
    }

    #[test]
    fn heap_store_roots_from_values_extracts_object_refs() {
        let values = vec![
            RuntimeVal::Int(1),
            RuntimeVal::Obj(HeapRef::new(3)),
            RuntimeVal::Nil,
            RuntimeVal::Obj(HeapRef::new(5)),
        ];

        assert_eq!(
            HeapStore::roots_from_values(&values),
            vec![HeapRef::new(3), HeapRef::new(5)]
        );
    }

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
