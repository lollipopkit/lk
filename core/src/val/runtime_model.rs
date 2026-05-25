//! New runtime value model for the VM rewrite.
//!
//! The `LiteralVal` enum remains active while the compiler and executor are migrated.
//! New VM code should target these types first.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::values::{ChannelValue, ShortStr, StreamCursorValue, StreamValue, TaskValue};

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
        captures: Arc<Vec<RuntimeVal>>,
    },
    RuntimeNative32 {
        name: Arc<str>,
        arity: u16,
        function: crate::vm::NativeFunction32,
    },
    Runtime32(Arc<crate::vm::RuntimeCallable32>),
}

#[derive(Clone, Debug)]
pub struct RuntimeObject {
    pub type_name: Arc<str>,
    pub fields: BTreeMap<Arc<str>, RuntimeVal>,
    pub field_slots: Vec<Arc<str>>,
}

impl RuntimeObject {
    pub fn new(type_name: Arc<str>, fields: BTreeMap<Arc<str>, RuntimeVal>) -> Self {
        let mut field_slots = Vec::with_capacity(fields.len());
        for key in fields.keys() {
            field_slots.push(Arc::clone(key));
        }
        Self {
            type_name,
            fields,
            field_slots,
        }
    }

    pub fn field_slot(&self, key: &str) -> Option<usize> {
        self.field_slots.iter().position(|candidate| candidate.as_ref() == key)
    }

    pub fn get_field(&self, key: &str) -> Option<RuntimeVal> {
        self.fields.get(key).cloned()
    }

    pub fn get_field_slot(&self, slot: usize, key: &str) -> Option<RuntimeVal> {
        let slot_key = self.field_slots.get(slot)?;
        if slot_key.as_ref() == key {
            self.fields.get(slot_key).cloned()
        } else {
            None
        }
    }

    pub fn set_field(&mut self, key: Arc<str>, value: RuntimeVal) {
        if !self.fields.contains_key(key.as_ref()) {
            self.field_slots.push(key.clone());
        }
        self.fields.insert(key, value);
    }
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

    pub fn slice_from(&self, start: usize) -> Self {
        match self {
            Self::Mixed(values) => Self::Mixed(copy_slice_tail(values, start)),
            Self::Int(values) => Self::Int(copy_slice_tail(values, start)),
            Self::Float(values) => Self::Float(copy_slice_tail(values, start)),
            Self::Bool(values) => Self::Bool(copy_slice_tail(values, start)),
            Self::String(values) => Self::String(copy_slice_tail(values, start)),
        }
    }
}

fn copy_slice_tail<T: Clone>(values: &[T], start: usize) -> Vec<T> {
    let tail = values.get(start..).unwrap_or(&[]);
    let mut out = Vec::with_capacity(tail.len());
    out.extend_from_slice(tail);
    out
}

impl PartialEq for TypedList {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Mixed(left), Self::Mixed(right)) => left == right,
            (Self::Int(left), Self::Int(right)) => left == right,
            (Self::Float(left), Self::Float(right)) => left == right,
            (Self::Bool(left), Self::Bool(right)) => left == right,
            (Self::String(left), Self::String(right)) => left == right,
            _ => typed_list_entries_equal_no_heap(self, other),
        }
    }
}

fn typed_list_entries_equal_no_heap(left: &TypedList, right: &TypedList) -> bool {
    left.len() == right.len() && (0..left.len()).all(|index| typed_list_item_equal_no_heap(left, index, right, index))
}

fn typed_list_item_equal_no_heap(left: &TypedList, left_index: usize, right: &TypedList, right_index: usize) -> bool {
    match (left, right) {
        (TypedList::Mixed(left), TypedList::Mixed(right)) => left[left_index] == right[right_index],
        (TypedList::Int(left), TypedList::Int(right)) => left[left_index] == right[right_index],
        (TypedList::Float(left), TypedList::Float(right)) => left[left_index] == right[right_index],
        (TypedList::Bool(left), TypedList::Bool(right)) => left[left_index] == right[right_index],
        (TypedList::String(left), TypedList::String(right)) => left[left_index] == right[right_index],
        (TypedList::Int(left), TypedList::Mixed(right)) => right[right_index] == RuntimeVal::Int(left[left_index]),
        (TypedList::Mixed(left), TypedList::Int(right)) => left[left_index] == RuntimeVal::Int(right[right_index]),
        (TypedList::Float(left), TypedList::Mixed(right)) => right[right_index] == RuntimeVal::Float(left[left_index]),
        (TypedList::Mixed(left), TypedList::Float(right)) => left[left_index] == RuntimeVal::Float(right[right_index]),
        (TypedList::Bool(left), TypedList::Mixed(right)) => right[right_index] == RuntimeVal::Bool(left[left_index]),
        (TypedList::Mixed(left), TypedList::Bool(right)) => left[left_index] == RuntimeVal::Bool(right[right_index]),
        (TypedList::String(left), TypedList::Mixed(right)) => ShortStr::new(&left[left_index])
            .map(RuntimeVal::ShortStr)
            .is_some_and(|value| right[right_index] == value),
        (TypedList::Mixed(left), TypedList::String(right)) => ShortStr::new(&right[right_index])
            .map(RuntimeVal::ShortStr)
            .is_some_and(|value| left[left_index] == value),
        _ => false,
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
                if values.is_empty()
                    && let Some(key) = key.as_arc_str()
                {
                    *self = match value {
                        RuntimeVal::Int(value) => Self::StringInt(BTreeMap::from([(key, value)])),
                        RuntimeVal::Float(value) => Self::StringFloat(BTreeMap::from([(key, value)])),
                        RuntimeVal::Bool(value) => Self::StringBool(BTreeMap::from([(key, value)])),
                        value => Self::StringMixed(BTreeMap::from([(key, value)])),
                    };
                    return;
                }
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
                            let mut mixed = BTreeMap::new();
                            for (key, value) in values.iter() {
                                mixed.insert(key.clone(), RuntimeVal::Int(*value));
                            }
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
                            let mut mixed = BTreeMap::new();
                            for (key, value) in values.iter() {
                                mixed.insert(key.clone(), RuntimeVal::Float(*value));
                            }
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
                            let mut mixed = BTreeMap::new();
                            for (key, value) in values.iter() {
                                mixed.insert(key.clone(), RuntimeVal::Bool(*value));
                            }
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

    fn materialize_string_map_to_mixed(&mut self, key: RuntimeMapKey, value: RuntimeVal) {
        let mut mixed = match std::mem::replace(self, Self::Mixed(BTreeMap::new())) {
            Self::Mixed(values) => values,
            Self::StringMixed(values) => {
                let mut mixed = BTreeMap::new();
                for (key, value) in values {
                    mixed.insert(RuntimeMapKey::String(key), value);
                }
                mixed
            }
            Self::StringInt(values) => {
                let mut mixed = BTreeMap::new();
                for (key, value) in values {
                    mixed.insert(RuntimeMapKey::String(key), RuntimeVal::Int(value));
                }
                mixed
            }
            Self::StringFloat(values) => {
                let mut mixed = BTreeMap::new();
                for (key, value) in values {
                    mixed.insert(RuntimeMapKey::String(key), RuntimeVal::Float(value));
                }
                mixed
            }
            Self::StringBool(values) => {
                let mut mixed = BTreeMap::new();
                for (key, value) in values {
                    mixed.insert(RuntimeMapKey::String(key), RuntimeVal::Bool(value));
                }
                mixed
            }
        };
        mixed.insert(key, value);
        *self = Self::Mixed(mixed);
    }
}

pub(crate) fn typed_map_from_entries(entries: BTreeMap<RuntimeMapKey, RuntimeVal>) -> TypedMap {
    if entries.is_empty() {
        return TypedMap::Mixed(entries);
    }

    #[derive(Clone, Copy)]
    enum StringMapShape {
        Mixed,
        Int,
        Float,
        Bool,
    }

    let mut shape: Option<StringMapShape> = None;
    for (key, value) in &entries {
        if key.as_arc_str().is_none() {
            return TypedMap::Mixed(entries);
        }
        let value_shape = match value {
            RuntimeVal::Int(_) => StringMapShape::Int,
            RuntimeVal::Float(_) => StringMapShape::Float,
            RuntimeVal::Bool(_) => StringMapShape::Bool,
            _ => StringMapShape::Mixed,
        };
        shape = match (shape, value_shape) {
            (None, shape) => Some(shape),
            (Some(StringMapShape::Int), StringMapShape::Int) => Some(StringMapShape::Int),
            (Some(StringMapShape::Float), StringMapShape::Float) => Some(StringMapShape::Float),
            (Some(StringMapShape::Bool), StringMapShape::Bool) => Some(StringMapShape::Bool),
            (Some(StringMapShape::Mixed), StringMapShape::Mixed) => Some(StringMapShape::Mixed),
            _ => {
                return TypedMap::StringMixed(string_mixed_entries_from_runtime_entries(entries));
            }
        };
    }

    match shape.expect("non-empty map has a shape") {
        StringMapShape::Mixed => TypedMap::StringMixed(string_mixed_entries_from_runtime_entries(entries)),
        StringMapShape::Int => TypedMap::StringInt(string_int_entries_from_runtime_entries(entries)),
        StringMapShape::Float => TypedMap::StringFloat(string_float_entries_from_runtime_entries(entries)),
        StringMapShape::Bool => TypedMap::StringBool(string_bool_entries_from_runtime_entries(entries)),
    }
}

fn string_mixed_entries_from_runtime_entries(
    entries: BTreeMap<RuntimeMapKey, RuntimeVal>,
) -> BTreeMap<Arc<str>, RuntimeVal> {
    let mut out = BTreeMap::new();
    for (key, value) in entries {
        out.insert(key.as_arc_str().expect("validated string key"), value);
    }
    out
}

fn string_int_entries_from_runtime_entries(entries: BTreeMap<RuntimeMapKey, RuntimeVal>) -> BTreeMap<Arc<str>, i64> {
    let mut out = BTreeMap::new();
    for (key, value) in entries {
        let RuntimeVal::Int(value) = value else {
            unreachable!("validated int map value");
        };
        out.insert(key.as_arc_str().expect("validated string key"), value);
    }
    out
}

fn string_float_entries_from_runtime_entries(entries: BTreeMap<RuntimeMapKey, RuntimeVal>) -> BTreeMap<Arc<str>, f64> {
    let mut out = BTreeMap::new();
    for (key, value) in entries {
        let RuntimeVal::Float(value) = value else {
            unreachable!("validated float map value");
        };
        out.insert(key.as_arc_str().expect("validated string key"), value);
    }
    out
}

fn string_bool_entries_from_runtime_entries(entries: BTreeMap<RuntimeMapKey, RuntimeVal>) -> BTreeMap<Arc<str>, bool> {
    let mut out = BTreeMap::new();
    for (key, value) in entries {
        let RuntimeVal::Bool(value) = value else {
            unreachable!("validated bool map value");
        };
        out.insert(key.as_arc_str().expect("validated string key"), value);
    }
    out
}

impl PartialEq for TypedMap {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Mixed(left), Self::Mixed(right)) => left == right,
            (Self::StringMixed(left), Self::StringMixed(right)) => left == right,
            (Self::StringInt(left), Self::StringInt(right)) => left == right,
            (Self::StringFloat(left), Self::StringFloat(right)) => left == right,
            (Self::StringBool(left), Self::StringBool(right)) => left == right,
            _ => typed_map_entries_equal(self, other),
        }
    }
}

fn typed_map_entries_equal(left: &TypedMap, right: &TypedMap) -> bool {
    left.len() == right.len()
        && typed_map_entries_all(left, |key, value| {
            typed_map_entry_value(right, &key).is_some_and(|right| right == value)
        })
}

fn typed_map_entries_all(map: &TypedMap, mut visit: impl FnMut(RuntimeMapKey, RuntimeVal) -> bool) -> bool {
    match map {
        TypedMap::Mixed(entries) => entries.iter().all(|(key, value)| visit(key.clone(), value.clone())),
        TypedMap::StringMixed(entries) => entries
            .iter()
            .all(|(key, value)| visit(RuntimeMapKey::String(key.clone()), value.clone())),
        TypedMap::StringInt(entries) => entries
            .iter()
            .all(|(key, value)| visit(RuntimeMapKey::String(key.clone()), RuntimeVal::Int(*value))),
        TypedMap::StringFloat(entries) => entries
            .iter()
            .all(|(key, value)| visit(RuntimeMapKey::String(key.clone()), RuntimeVal::Float(*value))),
        TypedMap::StringBool(entries) => entries
            .iter()
            .all(|(key, value)| visit(RuntimeMapKey::String(key.clone()), RuntimeVal::Bool(*value))),
    }
}

fn typed_map_entry_value(map: &TypedMap, key: &RuntimeMapKey) -> Option<RuntimeVal> {
    match map {
        TypedMap::Mixed(entries) => entries.get(key).cloned(),
        TypedMap::StringMixed(entries) => {
            let RuntimeMapKey::String(key) = key else {
                return None;
            };
            entries.get(key).cloned()
        }
        TypedMap::StringInt(entries) => {
            let RuntimeMapKey::String(key) = key else {
                return None;
            };
            entries.get(key).copied().map(RuntimeVal::Int)
        }
        TypedMap::StringFloat(entries) => {
            let RuntimeMapKey::String(key) = key else {
                return None;
            };
            entries.get(key).copied().map(RuntimeVal::Float)
        }
        TypedMap::StringBool(entries) => {
            let RuntimeMapKey::String(key) = key else {
                return None;
            };
            entries.get(key).copied().map(RuntimeVal::Bool)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_entries_materialize_to_typed_string_maps() {
        let mut entries = BTreeMap::new();
        entries.insert(RuntimeMapKey::String(Arc::<str>::from("answer")), RuntimeVal::Int(42));

        assert!(matches!(
            typed_map_from_entries(entries),
            TypedMap::StringInt(values) if values.get("answer") == Some(&42)
        ));

        let mut entries = BTreeMap::new();
        entries.insert(
            RuntimeMapKey::ShortStr(ShortStr::new("ok").expect("short")),
            RuntimeVal::Bool(true),
        );
        assert!(matches!(
            typed_map_from_entries(entries),
            TypedMap::StringBool(values) if values.get("ok") == Some(&true)
        ));

        let mut entries = BTreeMap::new();
        entries.insert(RuntimeMapKey::Int(1), RuntimeVal::Int(42));
        assert!(matches!(typed_map_from_entries(entries), TypedMap::Mixed(_)));
    }

    #[test]
    fn typed_list_equality_compares_backing_without_runtime_value_vector() {
        let short = ShortStr::new("short").expect("short");
        let typed_int = TypedList::Int(vec![1, 2]);
        let mixed_int = TypedList::Mixed(vec![RuntimeVal::Int(1), RuntimeVal::Int(2)]);
        let typed_short_string = TypedList::String(vec![Arc::<str>::from("short")]);
        let mixed_short_string = TypedList::Mixed(vec![RuntimeVal::ShortStr(short)]);
        let typed_long_string = TypedList::String(vec![Arc::<str>::from("longer-than-short")]);
        let mixed_long_string = TypedList::Mixed(vec![RuntimeVal::Obj(HeapRef::new(7))]);

        assert_eq!(typed_int, mixed_int);
        assert_eq!(typed_short_string, mixed_short_string);
        assert_ne!(typed_long_string, mixed_long_string);
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
    fn empty_mixed_map_set_with_string_key_specializes_backing() {
        let mut map = TypedMap::Mixed(BTreeMap::new());

        map.set(
            RuntimeMapKey::ShortStr(ShortStr::new("answer").expect("short")),
            RuntimeVal::Int(42),
        );

        assert!(matches!(map, TypedMap::StringInt(_)));
        assert_eq!(
            map.get(&RuntimeMapKey::String(Arc::<str>::from("answer"))),
            Some(RuntimeVal::Int(42))
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

    #[test]
    fn typed_map_equality_compares_entries_without_materializing_vector() {
        let typed = TypedMap::StringInt(BTreeMap::from([(Arc::<str>::from("answer"), 42)]));
        let string_mixed = TypedMap::StringMixed(BTreeMap::from([(Arc::<str>::from("answer"), RuntimeVal::Int(42))]));
        let exact_mixed = TypedMap::Mixed(BTreeMap::from([(
            RuntimeMapKey::String(Arc::<str>::from("answer")),
            RuntimeVal::Int(42),
        )]));
        let short_key_mixed = TypedMap::Mixed(BTreeMap::from([(
            RuntimeMapKey::ShortStr(ShortStr::new("answer").expect("short")),
            RuntimeVal::Int(42),
        )]));

        assert_eq!(typed, string_mixed);
        assert_eq!(typed, exact_mixed);
        assert_ne!(typed, short_key_mixed);
    }
}
