use arcstr::ArcStr;

use std::sync::Arc;

use super::{
    HeapValue, Val,
    intern::{intern, intern_owned},
    map_key_cache::cache_fresh_str_hash,
    types::ShortStr,
};

impl Val {
    /// 从 &str 构造 Val，≤7 字节走 ShortStr（零分配），更长走 heap string。
    #[inline]
    pub fn from_str(s: &str) -> Val {
        if let Some(short) = ShortStr::new(s) {
            Val::ShortStr(short)
        } else {
            Val::Obj(Arc::new(HeapValue::String(intern(s).as_str().into())))
        }
    }

    #[inline]
    fn from_concat_string(s: String) -> Val {
        if let Some(short) = ShortStr::new(s.as_str()) {
            Val::ShortStr(short)
        } else {
            let arc = intern_owned(s);
            cache_fresh_str_hash(arc.as_str());
            Val::Obj(Arc::new(HeapValue::String(arc.as_str().into())))
        }
    }

    #[inline]
    pub fn str_intern(s: &str) -> Val {
        Val::Obj(Arc::new(HeapValue::String(intern(s).as_str().into())))
    }

    #[inline]
    pub fn intern_str(s: &str) -> ArcStr {
        intern(s)
    }

    #[inline]
    pub fn string_key_arcstr(&self) -> Option<ArcStr> {
        match self {
            Val::ShortStr(s) => Some(intern(s.as_str())),
            Val::Obj(value) => match value.as_ref() {
                HeapValue::String(value) => Some(intern(value.as_ref())),
                _ => None,
            },
            _ => None,
        }
    }

    /// 若 Val 是字符串变体，返回 &str；否则返回 None。
    #[inline]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Val::ShortStr(s) => Some(s.as_str()),
            Val::Obj(value) => match value.as_ref() {
                HeapValue::String(value) => Some(value.as_ref()),
                _ => None,
            },
            _ => None,
        }
    }

    /// Fast string concatenation — hot path for `s = s + "x"` loops.
    #[inline]
    pub(crate) fn concat_strings(a: &str, b: &str) -> Val {
        if a.is_empty() {
            return Val::from_str(b);
        }
        if b.is_empty() {
            return Val::from_str(a);
        }
        let total = a.len() + b.len();
        let mut s = String::with_capacity(total);
        s.push_str(a);
        s.push_str(b);
        Val::from_concat_string(s)
    }

    #[inline]
    pub(crate) fn ascii_char_value(byte: u8) -> Val {
        debug_assert!(byte.is_ascii());
        Val::ShortStr(ShortStr::from_char(byte as char))
    }
}
