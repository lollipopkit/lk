use std::{fmt::Debug, sync::Arc};

use crate::val::RuntimeVal;

mod strings;
mod types;

pub use types::{FunctionNamedParamType, ShortStr, ShortStrOrStr, Type};

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
    pub roots: Vec<RuntimeVal>,
}

#[derive(Debug, Clone)]
pub struct StreamCursorValue {
    pub id: u64,
    pub stream_id: u64,
    pub roots: Vec<RuntimeVal>,
}

#[derive(Debug, Clone, Default)]
pub enum LiteralVal {
    /// AST inline short string literal.
    ShortStr(ShortStr),
    Int(i64),
    Float(f64),
    Bool(bool),
    String(Arc<str>),
    #[default]
    Nil,
}

impl PartialEq for LiteralVal {
    fn eq(&self, other: &Self) -> bool {
        if let (Some(a), Some(b)) = (self.as_str(), other.as_str()) {
            return a == b;
        }
        match (self, other) {
            (LiteralVal::Int(a), LiteralVal::Int(b)) => a == b,
            (LiteralVal::Float(a), LiteralVal::Float(b)) => a == b,
            (LiteralVal::Bool(a), LiteralVal::Bool(b)) => a == b,
            (LiteralVal::Nil, LiteralVal::Nil) => true,
            _ => false,
        }
    }
}

impl core::fmt::Display for LiteralVal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LiteralVal::Int(i) => write!(f, "{i}"),
            LiteralVal::Float(fl) => write!(f, "{fl}"),
            LiteralVal::Bool(b) => write!(f, "{b}"),
            LiteralVal::ShortStr(s) => f.write_str(s.as_str()),
            LiteralVal::String(value) => f.write_str(value.as_ref()),
            LiteralVal::Nil => write!(f, "nil"),
        }
    }
}
