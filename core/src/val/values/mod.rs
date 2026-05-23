use std::fmt::Debug;

// Using standard HashMap for maps and environments

use arcstr::ArcStr;

mod clone;
mod intern;
mod strings;
mod types;

pub use types::{FunctionNamedParamType, ShortStr, Type};

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
    /// Long string retained for the old scalar value shell.
    LongStr(ArcStr),
    #[default]
    Nil,
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
            (Val::LongStr(a), Val::LongStr(b)) => a == b,
            (Val::Nil, Val::Nil) => true,
            _ => false,
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
            Val::LongStr(value) => f.write_str(value.as_ref()),
            Val::Nil => write!(f, "nil"),
        }
    }
}
