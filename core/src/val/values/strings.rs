use std::cell::RefCell;

use arcstr::ArcStr;

use crate::util::fast_map::{FastHashMap, fast_hash_map_new};

use super::{
    Val,
    intern::{intern, intern_owned},
    map_key_cache::cache_fresh_str_hash,
    types::ShortStr,
};

thread_local! {
    static STR_INT_KEY_CACHE: RefCell<FastHashMap<(usize, usize, i64), ArcStr>> = RefCell::new(fast_hash_map_new());
}

const STR_INT_KEY_CACHE_LIMIT: usize = 4096;

impl Val {
    /// 从 &str 构造 Val，≤7 字节走 ShortStr（零分配），更长走 Str(ArcStr)。
    #[inline]
    pub fn from_str(s: &str) -> Val {
        if let Some(short) = ShortStr::new(s) {
            Val::ShortStr(short)
        } else {
            Val::Str(intern(s))
        }
    }

    #[inline]
    pub(crate) fn from_string(s: String) -> Val {
        if let Some(short) = ShortStr::new(s.as_str()) {
            Val::ShortStr(short)
        } else {
            Val::Str(intern_owned(s))
        }
    }

    #[inline]
    fn from_concat_string(s: String) -> Val {
        if let Some(short) = ShortStr::new(s.as_str()) {
            Val::ShortStr(short)
        } else {
            let arc = intern_owned(s);
            cache_fresh_str_hash(arc.as_str());
            Val::Str(arc)
        }
    }

    #[inline]
    pub fn str_intern(s: &str) -> Val {
        Val::Str(intern(s))
    }

    #[inline]
    pub fn intern_str(s: &str) -> ArcStr {
        intern(s)
    }

    #[inline]
    pub(crate) fn cached_str_int_key(prefix: &str, suffix: i64) -> ArcStr {
        let cache_key = (prefix.as_ptr() as usize, prefix.len(), suffix);
        if let Some(key) = STR_INT_KEY_CACHE.with(|cache| cache.borrow().get(&cache_key).cloned()) {
            return key;
        }
        let mut buf = itoa::Buffer::new();
        let suffix = buf.format(suffix);
        let mut key = String::with_capacity(prefix.len() + suffix.len());
        key.push_str(prefix);
        key.push_str(suffix);
        let key = ArcStr::from(key);
        STR_INT_KEY_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            if cache.len() >= STR_INT_KEY_CACHE_LIMIT {
                cache.clear();
            }
            cache.insert(cache_key, key.clone());
        });
        key
    }

    #[inline]
    pub fn string_key_arcstr(&self) -> Option<ArcStr> {
        match self {
            Val::Str(s) => Some(s.clone()),
            Val::ShortStr(s) => Some(intern(s.as_str())),
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn dynamic_string_key_arcstr(&self) -> Option<ArcStr> {
        match self {
            Val::Str(s) => Some(s.clone()),
            Val::ShortStr(s) => Some(ArcStr::from(s.as_str())),
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn primitive_key_arcstr(&self) -> Option<ArcStr> {
        match self {
            Val::Str(s) => Some(s.clone()),
            Val::ShortStr(s) => Some(intern(s.as_str())),
            Val::Int(i) => {
                let mut buf = itoa::Buffer::new();
                Some(intern(buf.format(*i)))
            }
            Val::Float(f) => Some(intern_owned(f.to_string())),
            Val::Bool(b) => Some(intern(if *b { "true" } else { "false" })),
            _ => None,
        }
    }

    /// 若 Val 是字符串变体，返回 &str；否则返回 None。
    #[inline]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Val::ShortStr(s) => Some(s.as_str()),
            Val::Str(s) => Some(s.as_str()),
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
    pub(crate) fn to_str_value(value: &Val) -> Val {
        match value {
            Val::ShortStr(s) => Val::ShortStr(*s),
            Val::Str(s) => Val::Str(s.clone()),
            Val::Int(i) => {
                let mut buf = itoa::Buffer::new();
                Val::from_str(buf.format(*i))
            }
            Val::Float(f) => {
                let mut buf = ryu::Buffer::new();
                Val::from_str(buf.format(*f))
            }
            Val::Bool(true) => Val::ShortStr(ShortStr::new("true").unwrap()),
            Val::Bool(false) => Val::ShortStr(ShortStr::new("false").unwrap()),
            Val::Nil => Val::ShortStr(ShortStr::new("nil").unwrap()),
            other => Val::from_string(other.to_string()),
        }
    }

    #[inline]
    pub(crate) fn ascii_char_value(byte: u8) -> Val {
        debug_assert!(byte.is_ascii());
        Val::ShortStr(ShortStr::from_char(byte as char))
    }

    #[inline]
    pub(crate) fn concat_str_add_rhs(prefix: &str, rhs: &Val) -> Option<Val> {
        match rhs {
            Val::ShortStr(s) => Some(Self::concat_strings(prefix, s.as_str())),
            Val::Str(s) => Some(Self::concat_strings(prefix, s.as_str())),
            Val::Int(i) => {
                let mut buf = itoa::Buffer::new();
                Some(Self::concat_strings(prefix, buf.format(*i)))
            }
            Val::Float(f) => {
                let mut buf = ryu::Buffer::new();
                Some(Self::concat_strings(prefix, buf.format(*f)))
            }
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn concat_str_tostr_rhs(prefix: &str, rhs: &Val) -> Option<Val> {
        match rhs {
            Val::ShortStr(s) => Some(Self::concat_strings(prefix, s.as_str())),
            Val::Str(s) => Some(Self::concat_strings(prefix, s.as_str())),
            Val::Int(i) => {
                let mut buf = itoa::Buffer::new();
                Some(Self::concat_strings(prefix, buf.format(*i)))
            }
            Val::Float(f) => {
                let mut buf = ryu::Buffer::new();
                Some(Self::concat_strings(prefix, buf.format(*f)))
            }
            Val::Bool(true) => Some(Self::concat_strings(prefix, "true")),
            Val::Bool(false) => Some(Self::concat_strings(prefix, "false")),
            Val::Nil => Some(Self::concat_strings(prefix, "nil")),
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn concat_add_lhs_str(lhs: &Val, suffix: &str) -> Option<Val> {
        match lhs {
            Val::ShortStr(s) => Some(Self::concat_strings(s.as_str(), suffix)),
            Val::Str(s) => Some(Self::concat_strings(s.as_str(), suffix)),
            Val::Int(i) => {
                let mut buf = itoa::Buffer::new();
                Some(Self::concat_strings(buf.format(*i), suffix))
            }
            Val::Float(f) => {
                let mut buf = ryu::Buffer::new();
                Some(Self::concat_strings(buf.format(*f), suffix))
            }
            _ => None,
        }
    }
}
