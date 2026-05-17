use std::{collections::HashMap, sync::Arc};

use anyhow::Result;

use crate::util::fast_map::{FastHashMap, fast_hash_map_with_capacity};
use arcstr::ArcStr;

use super::{ChannelValue, TaskValue, Type, Val};

impl From<String> for Val {
    #[inline]
    fn from(s: String) -> Self {
        Val::from_str(&s)
    }
}

impl From<&str> for Val {
    #[inline]
    fn from(s: &str) -> Self {
        Val::from_str(s)
    }
}

impl From<i64> for Val {
    #[inline]
    fn from(i: i64) -> Self {
        Val::Int(i)
    }
}

impl From<f64> for Val {
    #[inline]
    fn from(f: f64) -> Self {
        Val::Float(f)
    }
}

impl From<bool> for Val {
    #[inline]
    fn from(b: bool) -> Self {
        Val::Bool(b)
    }
}

impl<V, S, H> From<HashMap<S, V, H>> for Val
where
    V: Into<Val>,
    S: AsRef<str>,
    H: core::hash::BuildHasher,
{
    fn from(m: HashMap<S, V, H>) -> Self {
        let mut inner: FastHashMap<ArcStr, Val> = fast_hash_map_with_capacity(m.len());
        for (k, v) in m.into_iter() {
            inner.insert(Val::intern_str(k.as_ref()), v.into());
        }
        Val::Map(Arc::new(inner))
    }
}

impl<T> From<Vec<T>> for Val
where
    T: Into<Val>,
{
    fn from(v: Vec<T>) -> Self {
        let v: Vec<Val> = v.into_iter().map(Into::into).collect();
        Val::List(Arc::new(v))
    }
}

impl<T> From<Box<T>> for Val
where
    T: Into<Val>,
{
    fn from(b: Box<T>) -> Self {
        (*b).into()
    }
}

impl<T> From<Option<T>> for Val
where
    T: Into<Val>,
{
    fn from(o: Option<T>) -> Self {
        match o {
            Some(v) => v.into(),
            None => Val::Nil,
        }
    }
}

impl From<()> for Val {
    fn from(_: ()) -> Self {
        Val::Nil
    }
}

impl From<(u64, Val)> for Val {
    fn from((id, value): (u64, Val)) -> Self {
        Val::Task(Arc::new(TaskValue { id, value: Some(value) }))
    }
}

impl From<(u64, i64, Type)> for Val {
    fn from((id, capacity, inner_type): (u64, i64, Type)) -> Self {
        Val::Channel(Arc::new(ChannelValue {
            id,
            capacity: Some(capacity),
            inner_type,
        }))
    }
}

impl From<serde_json::Value> for Val {
    fn from(val: serde_json::Value) -> Self {
        match val {
            serde_json::Value::String(s) => Val::from_str(&s),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Val::Int(i)
                } else if let Some(f) = n.as_f64() {
                    Val::Float(f)
                } else {
                    Val::Nil
                }
            }
            serde_json::Value::Bool(b) => Val::Bool(b),
            serde_json::Value::Array(a) => {
                let v: Vec<Val> = a.into_iter().map(Val::from).collect();
                Val::List(Arc::from(v))
            }
            serde_json::Value::Object(o) => {
                let m: FastHashMap<ArcStr, Val> = o
                    .into_iter()
                    .map(|(k, v)| (Val::intern_str(k.as_str()), Val::from(v)))
                    .collect();
                Val::Map(Arc::new(m))
            }
            serde_json::Value::Null => Val::Nil,
        }
    }
}

impl From<serde_yaml::Value> for Val {
    fn from(val: serde_yaml::Value) -> Self {
        match val {
            serde_yaml::Value::String(s) => Val::from_str(&s),
            serde_yaml::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Val::Int(i)
                } else if let Some(f) = n.as_f64() {
                    Val::Float(f)
                } else {
                    Val::Nil
                }
            }
            serde_yaml::Value::Bool(b) => Val::Bool(b),
            serde_yaml::Value::Sequence(a) => {
                let v: Vec<Val> = a.into_iter().map(Val::from).collect();
                Val::List(Arc::from(v))
            }
            serde_yaml::Value::Mapping(o) => {
                let m: FastHashMap<ArcStr, Val> = o
                    .into_iter()
                    .filter_map(|(k, v)| {
                        if let serde_yaml::Value::String(key) = k {
                            Some((Val::intern_str(key.as_str()), Val::from(v)))
                        } else {
                            None
                        }
                    })
                    .collect();
                Val::Map(Arc::new(m))
            }
            serde_yaml::Value::Null => Val::Nil,
            serde_yaml::Value::Tagged(tagged) => Val::from(tagged.value),
        }
    }
}

impl Val {
    pub fn try_from<T>(val: T) -> Result<Self>
    where
        T: serde::Serialize,
    {
        Ok(serde_json::to_value(val)?.into())
    }
}
