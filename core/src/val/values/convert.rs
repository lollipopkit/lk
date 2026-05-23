use std::sync::Arc;

use anyhow::Result;

use super::{ChannelValue, Type, Val};

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

impl From<(u64, i64, Type)> for Val {
    fn from((id, capacity, inner_type): (u64, i64, Type)) -> Self {
        Val::channel(Arc::new(ChannelValue {
            id,
            capacity: Some(capacity),
            inner_type,
        }))
    }
}

impl Val {
    pub fn try_from<T>(val: T) -> Result<Self>
    where
        T: serde::Serialize,
    {
        match serde_json::to_value(val)? {
            serde_json::Value::Null => Ok(Val::Nil),
            serde_json::Value::Bool(value) => Ok(Val::Bool(value)),
            serde_json::Value::Number(value) => {
                if let Some(value) = value.as_i64() {
                    Ok(Val::Int(value))
                } else if let Some(value) = value.as_f64() {
                    Ok(Val::Float(value))
                } else {
                    Err(anyhow::anyhow!("numeric value is outside LK scalar range"))
                }
            }
            serde_json::Value::String(value) => Ok(Val::from_str(&value)),
            serde_json::Value::Array(_) | serde_json::Value::Object(_) => Err(anyhow::anyhow!(
                "structured serialization must use runtime decoder APIs"
            )),
        }
    }
}
