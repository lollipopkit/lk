//! `lk-values` — L0 front-end value/type model for LK.
//!
//! The compile-time literal/type model (`LiteralVal`, `Type`, `ShortStr`, the
//! numeric hierarchy). Extracted from `core::val` so it can become a clean
//! dependency-free L0 layer; the runtime value model (`RuntimeVal`, heap,
//! callables) stays in `core` (it embeds the execution model). Re-exported at
//! `lk_core::val`, so in-crate paths like `crate::val::Type` are unchanged.

use std::sync::Arc;

mod numeric;
mod strings;
mod types;

pub use numeric::{NumericClass, NumericHierarchy};
pub use types::{FunctionNamedParamType, ShortStr, ShortStrOrStr, Type};

// NOTE: runtime resource-handle values (TaskValue/ChannelValue/StreamValue/
// StreamCursorValue/SliceValue/ResourceValue/ResourceHandle) live in
// `super::runtime_model` — they embed `RuntimeVal`/`RuntimePayload` (the runtime
// model), whereas this module is the front-end literal/type model (a clean L0
// candidate: `LiteralVal`, `Type`, `ShortStr`, `numeric`). Both are re-exported
// at `crate::val`, so external `val::TaskValue` etc. paths are unchanged.

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
