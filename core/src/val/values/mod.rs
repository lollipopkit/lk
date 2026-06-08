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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliceKind {
    List,
    String,
}

#[derive(Debug, Clone)]
pub struct SliceValue {
    pub source: RuntimeVal,
    pub kind: SliceKind,
    pub start: usize,
    pub len: usize,
}

#[derive(Clone)]
pub struct ResourceValue {
    pub kind: &'static str,
    pub handle: Arc<std::sync::Mutex<ResourceHandle>>,
}

impl Debug for ResourceValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceValue")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

pub enum ResourceHandle {
    File(std::fs::File),
    Stdin,
    Stdout,
    Stderr,
    TcpStream(std::net::TcpStream),
    TcpListener(std::net::TcpListener),
    UdpSocket(std::net::UdpSocket),
    Closed,
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
