use super::{
    Val,
    intern::{intern, intern_owned},
    types::ShortStr,
};

impl Val {
    /// 从 &str 构造 Val，≤7 字节走 ShortStr（零分配），更长走 heap string。
    #[inline]
    pub fn from_str(s: &str) -> Val {
        if let Some(short) = ShortStr::new(s) {
            Val::ShortStr(short)
        } else {
            Val::LongStr(intern(s))
        }
    }

    #[inline]
    fn from_concat_string(s: String) -> Val {
        if let Some(short) = ShortStr::new(s.as_str()) {
            Val::ShortStr(short)
        } else {
            let arc = intern_owned(s);
            Val::LongStr(arc)
        }
    }

    /// 若 Val 是字符串变体，返回 &str；否则返回 None。
    #[inline]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Val::ShortStr(s) => Some(s.as_str()),
            Val::LongStr(value) => Some(value.as_ref()),
            _ => None,
        }
    }

    /// String literal concatenation used by AST constant folding.
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
}
