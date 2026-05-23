use std::{
    collections::{BTreeMap, HashMap},
    fmt::Debug,
    sync::Arc,
};

use arcstr::ArcStr;

use crate::util::fast_map::FastHashMap;

// Using standard HashMap for maps and environments

use super::runtime_model::{CallableValue, HeapValue, RuntimeMapKey, RuntimeObject, RuntimeVal, TypedList, TypedMap};

#[cfg(test)]
use crate::vm::NativeFunction32;
use crate::vm::{RuntimeCallable32, VmContext};

mod call;
mod clone;
mod convert;
mod intern;
mod map_key_cache;
mod ops;
mod serde_impl;
mod strings;
mod types;

pub use types::{FunctionNamedParamType, ShortStr, Type};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AotFunction {
    pub ptr: usize,
    pub arity: u8,
}

#[derive(Debug, Clone)]
pub struct TaskValue {
    pub id: u64,
    pub value: Option<crate::rt::RuntimePayload>,
}

#[derive(Debug, Clone)]
pub struct ChannelValue {
    pub id: u64,
    pub capacity: Option<i64>,
    pub inner_type: Type,
}

#[derive(Debug, Clone)]
pub struct StreamValue {
    pub id: u64,
    pub inner_type: Type,
}

#[derive(Debug, Clone)]
pub struct StreamCursorValue {
    pub id: u64,
    pub stream_id: u64,
}

#[derive(Debug, Default)]
pub enum Val {
    /// 内联短字符串（≤7 字节），零堆分配，实现 Copy
    ShortStr(ShortStr),
    Int(i64),
    Float(f64),
    Bool(bool),
    /// Heap object backed by the new runtime value model.
    Obj(Arc<HeapValue>),
    #[default]
    Nil,
}

impl Val {
    #[inline]
    pub fn type_name(&self) -> &'static str {
        match self {
            Val::ShortStr(_) => "String",
            Val::Int(_) => "Int",
            Val::Float(_) => "Float",
            Val::Bool(_) => "Bool",
            Val::Obj(value) => heap_value_type_name(value.as_ref()),
            Val::Nil => "Nil",
        }
    }

    #[inline]
    pub fn is_callable(&self) -> bool {
        matches!(self, Self::Obj(value) if matches!(value.as_ref(), HeapValue::Callable(_)))
    }

    #[inline]
    pub fn list(values: Arc<Vec<Val>>) -> Self {
        let values = values
            .iter()
            .cloned()
            .map(Self::val_to_object_field)
            .collect::<Vec<_>>();
        Self::Obj(Arc::new(HeapValue::List(TypedList::Mixed(values))))
    }

    #[inline]
    pub fn as_list(&self) -> Option<Arc<Vec<Val>>> {
        match self {
            Self::Obj(value) => match value.as_ref() {
                HeapValue::List(value) => Some(Arc::new(value.to_val_values())),
                _ => None,
            },
            _ => None,
        }
    }

    #[inline]
    pub fn map(values: Arc<FastHashMap<ArcStr, Val>>) -> Self {
        let mut entries = BTreeMap::new();
        for (key, value) in values.iter() {
            entries.insert(
                RuntimeMapKey::String(Arc::<str>::from(key.as_str())),
                Self::val_to_object_field(value.clone()),
            );
        }
        Self::Obj(Arc::new(HeapValue::Map(TypedMap::from_runtime_entries(entries))))
    }

    pub fn string_map_from_hashmap(values: HashMap<String, Val>) -> Self {
        let mut out = crate::util::fast_map::fast_hash_map_with_capacity(values.len());
        for (key, value) in values {
            out.insert(Self::intern_str(&key), value);
        }
        Self::map(Arc::new(out))
    }

    #[cfg(test)]
    pub(crate) fn test_list_from_values<T>(values: Vec<T>) -> Self
    where
        T: Into<Val>,
    {
        Self::list(Arc::new(values.into_iter().map(Into::into).collect()))
    }

    #[cfg(test)]
    pub(crate) fn test_string_map_from_hashmap<S, V, H>(values: HashMap<S, V, H>) -> Self
    where
        S: AsRef<str>,
        V: TestIntoVal,
        H: core::hash::BuildHasher,
    {
        let mut out = crate::util::fast_map::fast_hash_map_with_capacity(values.len());
        for (key, value) in values {
            out.insert(Self::intern_str(key.as_ref()), value.into_test_val());
        }
        Self::map(Arc::new(out))
    }

    #[cfg(test)]
    pub(crate) fn test_from<T>(value: T) -> Self
    where
        T: TestIntoVal,
    {
        value.into_test_val()
    }

    #[inline]
    pub fn as_map(&self) -> Option<Arc<FastHashMap<ArcStr, Val>>> {
        match self {
            Self::Obj(value) => match value.as_ref() {
                HeapValue::Map(value) => Some(Arc::new(value.to_val_entries())),
                _ => None,
            },
            _ => None,
        }
    }

    #[inline]
    pub fn as_runtime_callable32(&self) -> Option<&Arc<RuntimeCallable32>> {
        match self {
            Self::Obj(value) => match value.as_ref() {
                HeapValue::Callable(CallableValue::Runtime32(value)) => Some(value),
                _ => None,
            },
            _ => None,
        }
    }

    #[inline]
    pub fn task(value: Arc<TaskValue>) -> Self {
        Self::Obj(Arc::new(HeapValue::Task(value)))
    }

    #[inline]
    pub fn as_task(&self) -> Option<&Arc<TaskValue>> {
        match self {
            Self::Obj(value) => match value.as_ref() {
                HeapValue::Task(value) => Some(value),
                _ => None,
            },
            _ => None,
        }
    }

    #[inline]
    pub fn channel(value: Arc<ChannelValue>) -> Self {
        Self::Obj(Arc::new(HeapValue::Channel(value)))
    }

    #[inline]
    pub fn as_channel(&self) -> Option<&Arc<ChannelValue>> {
        match self {
            Self::Obj(value) => match value.as_ref() {
                HeapValue::Channel(value) => Some(value),
                _ => None,
            },
            _ => None,
        }
    }

    #[inline]
    pub fn stream(value: Arc<StreamValue>) -> Self {
        Self::Obj(Arc::new(HeapValue::Stream(value)))
    }

    #[inline]
    pub fn as_stream(&self) -> Option<&Arc<StreamValue>> {
        match self {
            Self::Obj(value) => match value.as_ref() {
                HeapValue::Stream(value) => Some(value),
                _ => None,
            },
            _ => None,
        }
    }

    #[inline]
    pub fn stream_cursor(value: Arc<StreamCursorValue>) -> Self {
        Self::Obj(Arc::new(HeapValue::StreamCursor(value)))
    }

    #[inline]
    pub fn as_stream_cursor(&self) -> Option<&Arc<StreamCursorValue>> {
        match self {
            Self::Obj(value) => match value.as_ref() {
                HeapValue::StreamCursor(value) => Some(value),
                _ => None,
            },
            _ => None,
        }
    }

    #[inline]
    #[cfg(test)]
    pub(crate) fn runtime_native32_for_test(function: NativeFunction32, arity: u16) -> Self {
        Self::Obj(Arc::new(HeapValue::Callable(CallableValue::RuntimeNative32 {
            arity,
            function,
        })))
    }

    #[inline]
    pub fn runtime_callable32(function: Arc<RuntimeCallable32>) -> Self {
        Self::Obj(Arc::new(HeapValue::Callable(CallableValue::Runtime32(function))))
    }

    #[inline]
    pub fn aot_function(function: AotFunction) -> Self {
        Self::Obj(Arc::new(HeapValue::Callable(CallableValue::Aot(function))))
    }

    /// Construct a runtime object of a named custom type.
    ///
    /// This is now backed by `RuntimeObject`; heap-backed field values
    /// are intentionally not preserved through this old constructor.
    #[inline]
    pub fn object<T: AsRef<str>>(type_name: T, fields: HashMap<String, Val>) -> Val {
        let fields = fields
            .into_iter()
            .map(|(key, value)| (Arc::<str>::from(key), Self::val_to_object_field(value)))
            .collect();
        Val::Obj(Arc::new(HeapValue::Object(RuntimeObject {
            type_name: Arc::<str>::from(type_name.as_ref()),
            fields,
        })))
    }

    #[inline]
    pub fn as_object(&self) -> Option<&RuntimeObject> {
        match self {
            Self::Obj(value) => match value.as_ref() {
                HeapValue::Object(value) => Some(value),
                _ => None,
            },
            _ => None,
        }
    }

    #[inline]
    pub fn val_to_object_field(value: Val) -> RuntimeVal {
        match value {
            Val::Nil => RuntimeVal::Nil,
            Val::Bool(value) => RuntimeVal::Bool(value),
            Val::Int(value) => RuntimeVal::Int(value),
            Val::Float(value) => RuntimeVal::Float(value),
            Val::ShortStr(value) => RuntimeVal::ShortStr(value),
            Val::Obj(value) => match value.as_ref() {
                HeapValue::String(value) => ShortStr::new(value.as_ref()).map_or(RuntimeVal::Nil, RuntimeVal::ShortStr),
                _ => RuntimeVal::Nil,
            },
        }
    }

    #[inline]
    pub fn object_field_to_val(value: &RuntimeVal) -> Val {
        match value {
            RuntimeVal::Nil => Val::Nil,
            RuntimeVal::Bool(value) => Val::Bool(*value),
            RuntimeVal::Int(value) => Val::Int(*value),
            RuntimeVal::Float(value) => Val::Float(*value),
            RuntimeVal::ShortStr(value) => Val::from(value.as_str()),
            RuntimeVal::Obj(_) => Val::Nil,
        }
    }

    #[inline]
    pub(crate) fn access(&self, field: &Val) -> Option<Val> {
        self.access_impl(field, None)
    }

    #[inline]
    fn access_impl(&self, field: &Val, collect_metrics: Option<bool>) -> Option<Val> {
        let _ = collect_metrics;
        match (self, field) {
            // String indexing and metadata
            (lhs, Val::Int(i)) if lhs.as_str().is_some() => {
                let s_str = lhs.as_str().unwrap();
                let len = if s_str.is_ascii() {
                    s_str.len()
                } else {
                    s_str.chars().count()
                };
                let idx = if *i < 0 {
                    len.checked_sub(i.unsigned_abs() as usize)?
                } else {
                    *i as usize
                };
                if s_str.is_ascii() {
                    let bs = s_str.as_bytes();
                    if idx < bs.len() {
                        Some(Val::ascii_char_value(bs[idx]))
                    } else {
                        None
                    }
                } else {
                    let ch = s_str.chars().nth(idx)?;
                    Some(Val::from_str(&ch.to_string()))
                }
            }
            (lhs, key) if lhs.as_str().is_some() && key.as_str() == Some("len") => {
                Some(Val::Int(lhs.as_str().unwrap().len() as i64))
            }
            (value, key) if value.as_object().is_some() && key.as_str().is_some() => {
                let key = Arc::<str>::from(key.as_str().unwrap());
                value
                    .as_object()
                    .expect("checked object")
                    .fields
                    .get(&key)
                    .map(Self::object_field_to_val)
            }
            (value, key) if value.as_channel().is_some() => match key.as_str() {
                Some("capacity") => Some(Val::Int(
                    value.as_channel().expect("checked channel").capacity.unwrap_or(0),
                )),
                Some("type") => Some(Val::from_str(&format!(
                    "{:?}",
                    value.as_channel().expect("checked channel").inner_type
                ))),
                _ => None,
            },
            _ => None,
        }
    }
}

#[cfg(test)]
mod callable_model_tests {
    use super::*;
    use crate::vm::{Function32, Module32};
    use anyhow::Result;

    fn dummy_native32(
        _args: crate::vm::NativeArgs32<'_>,
        _runtime: &mut crate::vm::NativeRuntime32<'_>,
    ) -> Result<RuntimeVal> {
        Ok(RuntimeVal::Nil)
    }

    #[test]
    fn runtime_callable32_is_stored_as_callable_heap_value() {
        let callable = Arc::new(RuntimeCallable32::new(
            Arc::new(Module32::single(Function32::default())),
            0,
            Vec::new(),
            crate::val::HeapStore::new(),
            Vec::new(),
        ));
        let value = Val::runtime_callable32(callable.clone());

        assert!(value.is_callable());
        assert!(matches!(
            value,
            Val::Obj(ref object)
                if matches!(object.as_ref(), HeapValue::Callable(CallableValue::Runtime32(function)) if Arc::ptr_eq(function, &callable))
        ));
        assert!(value.as_runtime_callable32().is_some());
    }

    #[test]
    fn runtime_native32_for_test_is_stored_as_callable_heap_value() {
        let value = Val::runtime_native32_for_test(crate::vm::NativeFunction32::Plain(dummy_native32), 0);

        assert!(value.is_callable());
        assert!(matches!(
            value,
            Val::Obj(ref object)
                if matches!(object.as_ref(), HeapValue::Callable(CallableValue::RuntimeNative32 { arity: 0, .. }))
        ));
        assert!(value.as_runtime_callable32().is_none());
    }
}

impl PartialEq for Val {
    fn eq(&self, other: &Self) -> bool {
        // Unify string comparisons across ShortStr and Str variants
        if let (Some(a), Some(b)) = (self.as_str(), other.as_str()) {
            return a == b;
        }
        match (self, other) {
            (Val::Int(a), Val::Int(b)) => a == b,
            (Val::Float(a), Val::Float(b)) => a == b,
            (Val::Bool(a), Val::Bool(b)) => a == b,
            (Val::Obj(a), Val::Obj(b)) => heap_values_eq(a.as_ref(), b.as_ref()),
            (Val::Nil, Val::Nil) => true,
            _ => false,
        }
    }
}

impl PartialOrd for Val {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (Val::Int(a), Val::Int(b)) => a.partial_cmp(b),
            (Val::Float(a), Val::Float(b)) => a.partial_cmp(b),
            (Val::Int(a), Val::Float(b)) => (*a as f64).partial_cmp(b),
            (Val::Float(a), Val::Int(b)) => a.partial_cmp(&(*b as f64)),
            _ => match (self.as_str(), other.as_str()) {
                (Some(a), Some(b)) => a.partial_cmp(b),
                _ => None,
            },
        }
    }
}

#[cfg(test)]
pub(crate) trait TestIntoVal {
    fn into_test_val(self) -> Val;
}

#[cfg(test)]
impl TestIntoVal for Val {
    fn into_test_val(self) -> Val {
        self
    }
}

#[cfg(test)]
impl TestIntoVal for i64 {
    fn into_test_val(self) -> Val {
        Val::Int(self)
    }
}

#[cfg(test)]
impl TestIntoVal for i32 {
    fn into_test_val(self) -> Val {
        Val::Int(i64::from(self))
    }
}

#[cfg(test)]
impl TestIntoVal for f64 {
    fn into_test_val(self) -> Val {
        Val::Float(self)
    }
}

#[cfg(test)]
impl TestIntoVal for bool {
    fn into_test_val(self) -> Val {
        Val::Bool(self)
    }
}

#[cfg(test)]
impl TestIntoVal for &str {
    fn into_test_val(self) -> Val {
        Val::from_str(self)
    }
}

#[cfg(test)]
impl TestIntoVal for String {
    fn into_test_val(self) -> Val {
        Val::from_str(&self)
    }
}

#[cfg(test)]
impl<T> TestIntoVal for Vec<T>
where
    T: TestIntoVal,
{
    fn into_test_val(self) -> Val {
        Val::list(Arc::new(self.into_iter().map(TestIntoVal::into_test_val).collect()))
    }
}

#[cfg(test)]
impl<T> TestIntoVal for Option<T>
where
    T: TestIntoVal,
{
    fn into_test_val(self) -> Val {
        match self {
            Some(value) => value.into_test_val(),
            None => Val::Nil,
        }
    }
}

#[cfg(test)]
impl<S, V, H> TestIntoVal for HashMap<S, V, H>
where
    S: AsRef<str>,
    V: TestIntoVal,
    H: core::hash::BuildHasher,
{
    fn into_test_val(self) -> Val {
        let mut out = crate::util::fast_map::fast_hash_map_with_capacity(self.len());
        for (key, value) in self {
            out.insert(Val::intern_str(key.as_ref()), value.into_test_val());
        }
        Val::map(Arc::new(out))
    }
}

impl core::fmt::Display for Val {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Val::Int(i) => write!(f, "{i}"),
            Val::Float(fl) => write!(f, "{fl}"),
            Val::Bool(b) => write!(f, "{b}"),
            Val::ShortStr(s) => f.write_str(s.as_str()),
            Val::Obj(value) => match value.as_ref() {
                HeapValue::List(values) => match serde_json::to_string(&values.to_val_values()) {
                    Ok(s) => write!(f, "{}", s),
                    Err(_) => write!(f, "{:?}", values),
                },
                HeapValue::Map(values) => match serde_json::to_string(&values.to_val_entries()) {
                    Ok(s) => write!(f, "{}", s),
                    Err(_) => write!(f, "{:?}", values),
                },
                value => display_heap_value(value, f),
            },
            Val::Nil => write!(f, "nil"),
        }
    }
}

#[inline]
fn heap_value_type_name(value: &HeapValue) -> &'static str {
    match value {
        HeapValue::String(_) => "String",
        HeapValue::List(_) => "List",
        HeapValue::Map(_) => "Map",
        HeapValue::Callable(_) => "Function",
        HeapValue::Task(_) => "Task",
        HeapValue::Channel(_) => "Channel",
        HeapValue::Stream(_) => "Stream",
        HeapValue::StreamCursor(_) => "StreamCursor",
        HeapValue::Object(_) => "Object",
        HeapValue::UpvalCell(_) => "UpvalCell",
        HeapValue::ErrorVal(_) => "Error",
    }
}

fn heap_values_eq(left: &HeapValue, right: &HeapValue) -> bool {
    match (left, right) {
        (HeapValue::String(left), HeapValue::String(right)) => left == right,
        (HeapValue::List(left), HeapValue::List(right)) => left == right,
        (HeapValue::Map(left), HeapValue::Map(right)) => left == right,
        (HeapValue::Task(left), HeapValue::Task(right)) => left.id == right.id,
        (HeapValue::Channel(left), HeapValue::Channel(right)) => {
            left.id == right.id && left.capacity == right.capacity && left.inner_type == right.inner_type
        }
        (HeapValue::Stream(left), HeapValue::Stream(right)) => {
            left.id == right.id && left.inner_type == right.inner_type
        }
        (HeapValue::StreamCursor(left), HeapValue::StreamCursor(right)) => {
            left.id == right.id && left.stream_id == right.stream_id
        }
        (HeapValue::Object(left), HeapValue::Object(right)) => {
            left.type_name == right.type_name && left.fields == right.fields
        }
        (HeapValue::UpvalCell(left), HeapValue::UpvalCell(right)) => left == right,
        (HeapValue::ErrorVal(left), HeapValue::ErrorVal(right)) => left == right,
        _ => std::ptr::eq(left, right),
    }
}

fn display_heap_value(value: &HeapValue, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
    match value {
        HeapValue::String(value) => f.write_str(value.as_ref()),
        HeapValue::List(values) => match serde_json::to_string(&values.to_val_values()) {
            Ok(s) => write!(f, "{}", s),
            Err(_) => write!(f, "{:?}", values),
        },
        HeapValue::Map(values) => match serde_json::to_string(&values.to_val_entries()) {
            Ok(s) => write!(f, "{}", s),
            Err(_) => write!(f, "{:?}", values),
        },
        HeapValue::Task(task) => match &task.value {
            Some(v) => write!(f, "Task(id={}, value={:?})", task.id, v.value),
            None => write!(f, "Task(id={}, pending)", task.id),
        },
        HeapValue::Channel(channel) => {
            write!(
                f,
                "Channel(id={}, capacity={}, type={:?})",
                channel.id,
                channel.capacity.unwrap_or(0),
                channel.inner_type
            )
        }
        HeapValue::Stream(stream) => {
            write!(f, "Stream(id={}, type={:?})", stream.id, stream.inner_type)
        }
        HeapValue::StreamCursor(cur) => {
            write!(f, "StreamCursor(id={}, stream={})", cur.id, cur.stream_id)
        }
        HeapValue::Callable(_) => write!(f, "<function>"),
        HeapValue::Object(object) => write!(f, "Object(type={}, fields={:?})", object.type_name, object.fields),
        HeapValue::UpvalCell(value) => write!(f, "UpvalCell({:?})", value),
        HeapValue::ErrorVal(error) => write!(f, "Error(message={})", error.message),
    }
}

fn dispatch_type_for_heap_value(value: &HeapValue) -> Type {
    match value {
        HeapValue::Task(_) => Type::Task(Box::new(Type::Any)),
        HeapValue::Channel(channel) => Type::Channel(Box::new(channel.inner_type.clone())),
        HeapValue::Stream(stream) => Type::Generic {
            name: "Stream".to_string(),
            params: vec![stream.inner_type.clone()],
        },
        HeapValue::StreamCursor(_) => Type::Named("StreamCursor".to_string()),
        HeapValue::Object(object) => Type::Named(object.type_name.to_string()),
        HeapValue::String(_) => Type::String,
        HeapValue::List(_) => Type::List(Box::new(Type::Any)),
        HeapValue::Map(_) => Type::Map(Box::new(Type::Any), Box::new(Type::Any)),
        HeapValue::Callable(_) => Type::Function {
            params: Vec::new(),
            named_params: Vec::new(),
            return_type: Box::new(Type::Any),
        },
        HeapValue::UpvalCell(_) => Type::Any,
        HeapValue::ErrorVal(_) => Type::Named("Error".to_string()),
    }
}

impl Val {
    /// Derive a static type hint suitable for method/trait dispatch.
    #[inline]
    pub fn dispatch_type(&self) -> Type {
        match self {
            Val::Int(_) => Type::Int,
            Val::Float(_) => Type::Float,
            Val::Bool(_) => Type::Bool,
            Val::ShortStr(_) => Type::String,
            Val::Obj(value) => dispatch_type_for_heap_value(value.as_ref()),
            Val::Nil => Type::Nil,
        }
    }

    /// Format the value into a String using the built-in display implementation.
    pub fn display_string(&self, ctx: Option<&VmContext>) -> String {
        let _ = ctx;
        // Fast path for primitives that don't need trait lookup
        match self {
            Val::ShortStr(s) => return s.as_str().to_string(),
            Val::Obj(value) if matches!(value.as_ref(), HeapValue::String(_)) => {
                return self.as_str().expect("checked string").to_string();
            }
            Val::Int(i) => return i.to_string(),
            Val::Float(f) => return f.to_string(),
            Val::Bool(b) => return b.to_string(),
            Val::Nil => return "nil".to_string(),
            _ => {}
        }

        format!("{}", self)
    }
}
