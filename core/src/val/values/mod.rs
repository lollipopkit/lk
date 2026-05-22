use std::{collections::HashMap, fmt::Debug, sync::Arc};

use arcstr::ArcStr;

use crate::util::fast_map::FastHashMap;

// Using standard HashMap for maps and environments

use crate::stmt::NamedParamDecl;

use super::runtime_model::{CallableValue, HeapValue, RuntimeObject, RuntimeVal, TypedList, TypedMap};

use crate::vm::{
    NativeFunction32, RuntimeCallable32, VmContext, analysis::vm_runtime_metrics_enabled,
    registers::copy_container_value_for_register_with_metrics,
};

mod cache;
mod call;
mod clone;
mod convert;
mod intern;
mod map_key_cache;
mod ops;
mod serde_impl;
mod strings;
mod types;

use cache::cached_list_contains;

pub use types::{FunctionNamedParamType, ShortStr, Type};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AotFunction {
    pub ptr: usize,
    pub arity: u8,
}

/// Legacy display/type stub for closures parsed outside the Instr32 compiler.
/// Executable closures are represented by `RuntimeCallable32`.
pub struct ClosureValue {
    pub params: Arc<Vec<String>>,
    pub named_params: Arc<Vec<NamedParamDecl>>,
    debug_name: Option<String>,
    debug_location: Option<String>,
}

pub struct ClosureInit {
    pub params: Arc<Vec<String>>,
    pub named_params: Arc<Vec<NamedParamDecl>>,
    pub debug_name: Option<String>,
    pub debug_location: Option<String>,
}

// Implement a non-recursive Debug for closures to avoid printing their captured
// environment, which can contain self-referential cycles via globals and lead
// to stack overflows when formatting with `{:?}`.
impl core::fmt::Debug for ClosureValue {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let name = self.debug_name.as_deref().unwrap_or("<closure>");
        let params = self.params.join(", ");
        let named: Vec<String> = self.named_params.iter().map(|p| p.name.clone()).collect();
        f.debug_struct("ClosureValue")
            .field("name", &name)
            .field("params", &params)
            .field("named_params", &named)
            .field("body", &"<body>")
            // Intentionally omit env/upvalues/captures to avoid recursive prints
            .finish()
    }
}

impl ClosureValue {
    pub fn new(init: ClosureInit) -> Self {
        let ClosureInit {
            params,
            named_params,
            debug_name,
            debug_location,
        } = init;
        Self {
            params,
            named_params,
            debug_name,
            debug_location,
        }
    }
}

impl ClosureValue {
    #[inline]
    pub fn debug_name(&self) -> Option<&str> {
        self.debug_name.as_deref()
    }

    #[inline]
    pub fn debug_location(&self) -> Option<&str> {
        self.debug_location.as_deref()
    }

    #[inline]
    pub fn frame_display_name(&self) -> String {
        self.debug_name.clone().unwrap_or_else(|| "<closure>".to_string())
    }
}

#[derive(Debug, Clone)]
pub struct TaskValue {
    pub id: u64,
    pub value: Option<Val>,
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
        Self::Obj(Arc::new(HeapValue::List(TypedList::from_legacy_values(&values))))
    }

    #[inline]
    pub fn as_list(&self) -> Option<Arc<Vec<Val>>> {
        match self {
            Self::Obj(value) => match value.as_ref() {
                HeapValue::List(value) => Some(Arc::new(value.to_legacy_values())),
                _ => None,
            },
            _ => None,
        }
    }

    #[inline]
    pub fn map(values: Arc<FastHashMap<ArcStr, Val>>) -> Self {
        Self::Obj(Arc::new(HeapValue::Map(TypedMap::from_legacy_entries(&values))))
    }

    #[inline]
    pub fn as_map(&self) -> Option<Arc<FastHashMap<ArcStr, Val>>> {
        match self {
            Self::Obj(value) => match value.as_ref() {
                HeapValue::Map(value) => Some(Arc::new(value.to_legacy_entries())),
                _ => None,
            },
            _ => None,
        }
    }

    #[inline]
    pub fn closure(function: Arc<ClosureValue>) -> Self {
        Self::Obj(Arc::new(HeapValue::Callable(CallableValue::ParsedClosure(function))))
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
    pub fn runtime_native32(function: NativeFunction32, arity: u16) -> Self {
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

    #[inline]
    pub fn as_closure(&self) -> Option<&Arc<ClosureValue>> {
        match self {
            Self::Obj(value) => match value.as_ref() {
                HeapValue::Callable(CallableValue::ParsedClosure(function)) => Some(function),
                _ => None,
            },
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn list_contains(list: &Arc<Vec<Val>>, needle: &Val) -> bool {
        if let Some(result) = cached_list_contains(list, needle) {
            result
        } else {
            (**list).contains(needle)
        }
    }

    #[inline]
    pub(crate) fn list_contains_all(list: &Arc<Vec<Val>>, subset: &Arc<Vec<Val>>) -> bool {
        subset.iter().all(|item| Val::list_contains(list, item))
    }

    /// Construct a runtime object of a named custom type.
    ///
    /// This is now backed by `RuntimeObject`; heap-backed legacy field values
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
    fn access_copy_value(value: &Val, collect_metrics: Option<bool>) -> Val {
        match collect_metrics {
            Some(collect_metrics) => copy_container_value_for_register_with_metrics(value, collect_metrics),
            None => value.clone(),
        }
    }

    #[inline]
    fn access_copy_slice(slice: &[Val], collect_metrics: Option<bool>) -> Arc<Vec<Val>> {
        if slice.is_empty() {
            return Arc::new(Vec::new());
        }
        match collect_metrics {
            Some(collect_metrics) => {
                let mut out = Vec::with_capacity(slice.len());
                for value in slice {
                    out.push(copy_container_value_for_register_with_metrics(value, collect_metrics));
                }
                Arc::new(out)
            }
            None => Arc::new(slice.to_vec()),
        }
    }

    #[inline]
    fn access_impl(&self, field: &Val, collect_metrics: Option<bool>) -> Option<Val> {
        match (self, field) {
            // Map: field lookup by key only (do not shadow keys with synthetic fields)
            (value, key) if value.as_map().is_some() && key.as_str().is_some() => {
                let m = value.as_map().expect("checked map");
                Self::map_get_str(&m, key.as_str().unwrap())
                    .map(|value| Self::access_copy_value(value, collect_metrics))
            }
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
            (value, Val::Int(i)) if value.as_list().is_some() => {
                let l = value.as_list().expect("checked list");
                let idx = if *i < 0 {
                    l.len().checked_sub(i.unsigned_abs() as usize)?
                } else {
                    *i as usize
                };
                l.get(idx).map(|value| Self::access_copy_value(value, collect_metrics))
            }
            (value, key_value) if value.as_list().is_some() && key_value.as_list().is_some() => {
                let l = value.as_list().expect("checked list");
                let key = key_value.as_list().expect("checked list key");
                let (start, end) = range_key_bounds(&key, l.len())?;
                Some(Val::list(Self::access_copy_slice(&l[start..end], collect_metrics)))
            }
            (lhs, key_value) if lhs.as_str().is_some() && key_value.as_list().is_some() => {
                let key = key_value.as_list().expect("checked list key");
                let text = lhs.as_str().unwrap();
                let len = if text.is_ascii() {
                    text.len()
                } else {
                    text.chars().count()
                };
                let (start, end) = range_key_bounds(&key, len)?;
                if text.is_ascii() {
                    Some(Val::from_str(&text[start..end]))
                } else {
                    Some(Val::from_str(
                        &text
                            .chars()
                            .skip(start)
                            .take(end.saturating_sub(start))
                            .collect::<String>(),
                    ))
                }
            }
            (value, key) if value.as_list().is_some() && key.as_str() == Some("len") => {
                Some(Val::Int(value.as_list().expect("checked list").len() as i64))
            }
            (lhs, key) if lhs.as_str().is_some() && key.as_str() == Some("len") => {
                Some(Val::Int(lhs.as_str().unwrap().len() as i64))
            }
            // Map index -> [key, value]
            (value, Val::Int(i)) if value.as_map().is_some() => {
                let m = value.as_map().expect("checked map");
                if *i < 0 {
                    return None;
                }
                let mut entries: Vec<_> = m.iter().collect();
                entries.sort_by(|(left, _), (right, _)| left.as_str().cmp(right.as_str()));
                let idx = *i as usize;
                if idx >= entries.len() {
                    return None;
                }
                let (key, value) = entries[idx];
                Some(Val::list(
                    vec![
                        Val::from_str(key.as_str()),
                        Self::access_copy_value(value, collect_metrics),
                    ]
                    .into(),
                ))
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
            (value, key) if value.as_task().is_some() && key.as_str() == Some("value") => {
                match &value.as_task().expect("checked task").value {
                    Some(v) => Some(Self::access_copy_value(v, collect_metrics)),
                    None => Some(Val::Nil),
                }
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

    #[inline]
    pub(crate) fn clone_list_slice_with_metrics(slice: &[Val], collect_metrics: bool) -> Arc<Vec<Val>> {
        if slice.is_empty() {
            return Arc::new(Vec::new());
        }
        if !collect_metrics {
            return Arc::new(slice.to_vec());
        }
        let mut vec = Vec::with_capacity(slice.len());
        for value in slice {
            vec.push(copy_container_value_for_register_with_metrics(value, collect_metrics));
        }
        Arc::new(vec)
    }

    #[inline]
    pub(crate) fn concat_lists_with_metrics(left: &[Val], right: &[Val], collect_metrics: bool) -> Arc<Vec<Val>> {
        if left.is_empty() {
            return Self::clone_list_slice_with_metrics(right, collect_metrics);
        }
        if right.is_empty() {
            return Self::clone_list_slice_with_metrics(left, collect_metrics);
        }
        if !collect_metrics {
            let mut vec = Vec::with_capacity(left.len() + right.len());
            vec.extend_from_slice(left);
            vec.extend_from_slice(right);
            return Arc::new(vec);
        }
        let mut vec = Vec::with_capacity(left.len() + right.len());
        for value in left.iter().chain(right.iter()) {
            vec.push(copy_container_value_for_register_with_metrics(value, collect_metrics));
        }
        Arc::new(vec)
    }

    #[inline(always)]
    pub fn append_to_list(list: &[Val], value: &Val) -> Arc<Vec<Val>> {
        Self::append_to_list_with_metrics(list, value, vm_runtime_metrics_enabled())
    }

    #[inline(always)]
    pub fn append_to_list_with_metrics(list: &[Val], value: &Val, collect_metrics: bool) -> Arc<Vec<Val>> {
        if !collect_metrics {
            let mut vec = Vec::with_capacity(list.len() + 1);
            vec.extend_from_slice(list);
            vec.push(value.clone());
            return Arc::new(vec);
        }
        let mut vec = Vec::with_capacity(list.len() + 1);
        for item in list {
            vec.push(copy_container_value_for_register_with_metrics(item, collect_metrics));
        }
        vec.push(copy_container_value_for_register_with_metrics(value, collect_metrics));
        Arc::new(vec)
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
    fn runtime_native32_is_stored_as_callable_heap_value() {
        let value = Val::runtime_native32(crate::vm::NativeFunction32::Plain(dummy_native32), 0);

        assert!(value.is_callable());
        assert!(matches!(
            value,
            Val::Obj(ref object)
                if matches!(object.as_ref(), HeapValue::Callable(CallableValue::RuntimeNative32 { arity: 0, .. }))
        ));
        assert!(value.as_runtime_callable32().is_none());
    }

    #[test]
    fn legacy_val_containers_are_materialized_as_typed_heap_values() {
        let list = Val::list(Arc::new(vec![Val::Int(1), Val::Int(2)]));
        assert!(matches!(
            list,
            Val::Obj(ref object) if matches!(object.as_ref(), HeapValue::List(TypedList::Int(values)) if values == &vec![1, 2])
        ));

        let mut map_items = FastHashMap::default();
        map_items.insert(ArcStr::from("answer"), Val::Int(42));
        let map = Val::map(Arc::new(map_items));
        assert!(matches!(
            map,
            Val::Obj(ref object)
                if matches!(object.as_ref(), HeapValue::Map(TypedMap::StringInt(values)) if values.get("answer") == Some(&42))
        ));
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
            (a, b) if a.as_map().is_some() && b.as_map().is_some() => {
                a.as_map().expect("checked map") == b.as_map().expect("checked map")
            }
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

impl core::fmt::Display for Val {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Val::Int(i) => write!(f, "{i}"),
            Val::Float(fl) => write!(f, "{fl}"),
            Val::Bool(b) => write!(f, "{b}"),
            Val::ShortStr(s) => f.write_str(s.as_str()),
            value if value.as_map().is_some() => {
                let m = value.as_map().expect("checked map");
                // Avoid serialization errors by using debug fallback
                match serde_json::to_string(m.as_ref()) {
                    Ok(s) => write!(f, "{}", s),
                    Err(_) => write!(f, "{:?}", m),
                }
            }
            Val::Obj(value) => display_heap_value(value.as_ref(), f),
            Val::Nil => write!(f, "nil"),
        }
    }
}

#[inline]
fn range_key_bounds(key: &[Val], len: usize) -> Option<(usize, usize)> {
    let Val::Int(first) = key.first()? else {
        return None;
    };
    let mut previous = *first;
    for item in key.iter().skip(1) {
        let Val::Int(current) = item else {
            return None;
        };
        if *current != previous + 1 {
            return None;
        }
        previous = *current;
    }

    let start = normalize_slice_bound(*first, len);
    let end = normalize_slice_bound(previous + 1, len);
    Some((start.min(end), end))
}

#[inline]
fn normalize_slice_bound(index: i64, len: usize) -> usize {
    if index < 0 {
        len.saturating_sub(index.unsigned_abs() as usize)
    } else {
        (index as usize).min(len)
    }
}

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
    }
}

fn heap_values_eq(left: &HeapValue, right: &HeapValue) -> bool {
    match (left, right) {
        (HeapValue::String(left), HeapValue::String(right)) => left == right,
        (HeapValue::List(left), HeapValue::List(right)) => left == right,
        (HeapValue::Map(left), HeapValue::Map(right)) => left == right,
        (HeapValue::Task(left), HeapValue::Task(right)) => left.id == right.id && left.value == right.value,
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
        _ => std::ptr::eq(left, right),
    }
}

fn display_heap_value(value: &HeapValue, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
    match value {
        HeapValue::String(value) => f.write_str(value.as_ref()),
        HeapValue::List(values) => match serde_json::to_string(&values.to_legacy_values()) {
            Ok(s) => write!(f, "{}", s),
            Err(_) => write!(f, "{:?}", values),
        },
        HeapValue::Map(values) => match serde_json::to_string(&values.to_legacy_entries()) {
            Ok(s) => write!(f, "{}", s),
            Err(_) => write!(f, "{:?}", values),
        },
        HeapValue::Task(task) => match &task.value {
            Some(v) => write!(f, "Task(id={}, value={})", task.id, v),
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
        HeapValue::Callable(CallableValue::ParsedClosure(closure)) => write!(f, "fn({})", closure.params.join(", ")),
        HeapValue::Callable(_) => write!(f, "<function>"),
        HeapValue::Object(object) => write!(f, "Object(type={}, fields={:?})", object.type_name, object.fields),
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
            value if value.as_map().is_some() => Type::Map(Box::new(Type::Any), Box::new(Type::Any)),
            Val::Obj(value) => dispatch_type_for_heap_value(value.as_ref()),
            Val::Nil => Type::Nil,
        }
    }

    /// Format the value into a String, preferring a user-defined Display-like
    /// trait method when available in the provided environment. This enables
    /// automatically using `impl Display for Type { fn display(self) -> String }`
    /// or a legacy `show(self) -> String` method if present via the trait/impl
    /// registry. Falls back to the built-in Display for Val.
    pub fn display_string(&self, ctx: Option<&VmContext>) -> String {
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

        if let Some(ctx_ref) = ctx
            && let Some(tc) = ctx_ref.type_checker()
        {
            let method_val = tc
                .registry()
                .get_method(&self.dispatch_type(), "to_string")
                .or_else(|| tc.registry().get_method(&self.dispatch_type(), "show"));

            if let Some(fun_val) = method_val {
                // Create a temporary mutable context for method calls
                let mut temp_ctx = ctx_ref.clone();
                let call_res = fun_val.call(std::slice::from_ref(self), &mut temp_ctx);
                if let Ok(v) = call_res {
                    // If the method returned a string, use it directly; otherwise use default formatting of returned value
                    return match v.as_str() {
                        Some(s) => s.to_string(),
                        None => format!("{}", v),
                    };
                }
            }
        }

        // Fallback to default Display implementation for Val
        format!("{}", self)
    }
}
