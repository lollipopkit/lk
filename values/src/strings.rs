use std::sync::Arc;

use super::{LiteralVal, types::ShortStr};

impl LiteralVal {
    /// Build an AST literal string. Runtime long strings are materialized as heap strings during lowering.
    #[inline]
    #[allow(clippy::should_implement_trait)] // infallible, unlike `FromStr::from_str`
    pub fn from_str(s: &str) -> Self {
        if let Some(short) = ShortStr::new(s) {
            Self::ShortStr(short)
        } else {
            Self::String(Arc::<str>::from(s))
        }
    }

    #[inline]
    fn from_concat_string(s: String) -> Self {
        if let Some(short) = ShortStr::new(s.as_str()) {
            Self::ShortStr(short)
        } else {
            Self::String(Arc::<str>::from(s))
        }
    }

    #[inline]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::ShortStr(s) => Some(s.as_str()),
            Self::String(value) => Some(value.as_ref()),
            _ => None,
        }
    }

    /// String literal concatenation used by AST constant folding.
    #[inline]
    pub fn concat_strings(a: &str, b: &str) -> Self {
        if a.is_empty() {
            return Self::from_str(b);
        }
        if b.is_empty() {
            return Self::from_str(a);
        }
        let total = a.len() + b.len();
        let mut s = String::with_capacity(total);
        s.push_str(a);
        s.push_str(b);
        Self::from_concat_string(s)
    }
}
