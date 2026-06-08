use std::fmt;

use anyhow::Result;
use arcstr::ArcStr;
use serde::{Deserialize, Serialize, Serializer};

use crate::typ::{NumericClass, NumericHierarchy};

/// 内联短字符串：0–7 字节 UTF-8，完全存储在 LiteralVal 内（零堆分配）。
/// 实现了 Copy，克隆无需原子操作。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShortStr {
    len: u8,
    data: [u8; 7],
}

impl ShortStr {
    /// 从 str 创建。若 s.len() > 7 返回 None。
    #[inline]
    pub fn new(s: &str) -> Option<Self> {
        let bytes = s.as_bytes();
        if bytes.len() > 7 {
            return None;
        }
        let mut data = [0u8; 7];
        data[..bytes.len()].copy_from_slice(bytes);
        Some(Self {
            len: bytes.len() as u8,
            data,
        })
    }

    #[inline]
    pub fn from_char(ch: char) -> Self {
        let mut data = [0u8; 7];
        let encoded = ch.encode_utf8(&mut data);
        Self {
            len: encoded.len() as u8,
            data,
        }
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        // SAFETY: data 在构造时已验证为合法 UTF-8。
        std::str::from_utf8(&self.data[..self.len as usize]).expect("ShortStr contains valid UTF-8")
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len as usize
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Concatenate a ShortStr prefix with an i64 suffix, returning ShortStr
    /// if the result fits in 7 bytes, otherwise a heap-allocated String.
    /// Avoids intermediate String allocation for small numbers.
    #[inline]
    pub fn concat_int(self, n: i64) -> ShortStrOrStr {
        let prefix = self.as_str().as_bytes();
        let prefix_len = prefix.len();
        // Fast path for common small non-negative integers: write digits directly.
        if n >= 0 && n < 10000 {
            let num_len = decimal_len_under_10000(n as u64);
            let total_len = prefix_len + num_len;
            if total_len <= 7 {
                let mut data = [0u8; 7];
                data[..prefix_len].copy_from_slice(prefix);
                write_u64_to_buf(n as u64, &mut data[prefix_len..]);
                return ShortStrOrStr::Short(ShortStr {
                    len: total_len as u8,
                    data,
                });
            }
        }
        // Fallback: format to String and try ShortStr
        let combined = format!("{}{}", self.as_str(), n);
        if let Some(short) = ShortStr::new(&combined) {
            ShortStrOrStr::Short(short)
        } else {
            ShortStrOrStr::Str(combined)
        }
    }

    /// Concatenate an i64 prefix with a ShortStr suffix, returning ShortStr
    /// if the result fits in 7 bytes, otherwise a heap-allocated String.
    #[inline]
    pub fn concat_int_prefix(n: i64, suffix: ShortStr) -> ShortStrOrStr {
        let suffix_bytes = suffix.as_str().as_bytes();
        let suffix_len = suffix_bytes.len();
        if n >= 0 && n < 10000 {
            let mut data = [0u8; 7];
            let num_len = write_u64_to_buf(n as u64, &mut data[..]);
            let total_len = num_len + suffix_len;
            if total_len <= 7 {
                data[num_len..total_len].copy_from_slice(suffix_bytes);
                return ShortStrOrStr::Short(ShortStr {
                    len: total_len as u8,
                    data,
                });
            }
        }
        let combined = format!("{}{}", n, suffix.as_str());
        if let Some(short) = ShortStr::new(&combined) {
            ShortStrOrStr::Short(short)
        } else {
            ShortStrOrStr::Str(combined)
        }
    }

    /// Concatenate two ShortStr values, returning ShortStr if the result
    /// fits in 7 bytes, otherwise a heap-allocated String.
    #[inline]
    pub fn concat(self, other: ShortStr) -> ShortStrOrStr {
        let a = self.as_str().as_bytes();
        let b = other.as_str().as_bytes();
        let total_len = a.len() + b.len();
        if total_len <= 7 {
            let mut data = [0u8; 7];
            data[..a.len()].copy_from_slice(a);
            data[a.len()..total_len].copy_from_slice(b);
            ShortStrOrStr::Short(ShortStr {
                len: total_len as u8,
                data,
            })
        } else {
            ShortStrOrStr::Str(format!("{}{}", self.as_str(), other.as_str()))
        }
    }
}

/// Result of concatenating two ShortStr values or a ShortStr with an Int.
/// Avoids String allocation when the result fits in ShortStr.
pub enum ShortStrOrStr {
    Short(ShortStr),
    Str(String),
}

/// Write a u64 as decimal ASCII to buf, returning the number of bytes written.
/// Assumes buf has at least 4 bytes of space (for numbers up to 9999).
#[inline]
fn write_u64_to_buf(n: u64, buf: &mut [u8]) -> usize {
    if n < 10 {
        buf[0] = b'0' + n as u8;
        1
    } else if n < 100 {
        buf[0] = b'0' + (n / 10) as u8;
        buf[1] = b'0' + (n % 10) as u8;
        2
    } else if n < 1000 {
        buf[0] = b'0' + (n / 100) as u8;
        buf[1] = b'0' + ((n / 10) % 10) as u8;
        buf[2] = b'0' + (n % 10) as u8;
        3
    } else if n < 10000 {
        buf[0] = b'0' + (n / 1000) as u8;
        buf[1] = b'0' + ((n / 100) % 10) as u8;
        buf[2] = b'0' + ((n / 10) % 10) as u8;
        buf[3] = b'0' + (n % 10) as u8;
        4
    } else {
        // Fallback for larger numbers
        let s = n.to_string();
        let len = s.len().min(buf.len());
        buf[..len].copy_from_slice(&s.as_bytes()[..len]);
        len
    }
}

#[inline]
fn decimal_len_under_10000(n: u64) -> usize {
    if n < 10 {
        1
    } else if n < 100 {
        2
    } else if n < 1000 {
        3
    } else {
        4
    }
}

impl fmt::Debug for ShortStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

impl fmt::Display for ShortStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<ShortStr> for ArcStr {
    fn from(s: ShortStr) -> ArcStr {
        ArcStr::from(s.as_str())
    }
}

impl serde::Serialize for ShortStr {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for ShortStr {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = <String as serde::Deserialize>::deserialize(deserializer)?;
        ShortStr::new(&s).ok_or_else(|| serde::de::Error::custom("string too long for ShortStr"))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionNamedParamType {
    pub name: String,
    pub ty: Type,
    pub has_default: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Type {
    /// Primitive types
    Int,
    Float,
    String,
    Bool,
    Nil,

    /// Generic container types
    List(Box<Type>), // List<T>
    Map(Box<Type>, Box<Type>), // Map<K, V>
    Set(Box<Type>),            // Set<T>
    /// Fixed-length heterogeneous tuple: (T0, T1, ...)
    Tuple(Vec<Type>),

    /// Function type with parameters and return type
    Function {
        params: Vec<Type>,
        named_params: Vec<FunctionNamedParamType>,
        return_type: Box<Type>,
    },

    /// Concurrency types
    Task(Box<Type>),
    Channel(Box<Type>),

    /// Union types: Int | String
    Union(Vec<Type>),

    /// Optional types: Int? (sugar for Int | Nil)
    Optional(Box<Type>),

    /// Type variables for inference (prefixed with ')
    Variable(String),

    /// Custom named types
    Named(String),

    /// Generic type with parameters: List<T>, Map<K, V>
    Generic {
        name: String,
        params: Vec<Type>,
    },

    /// Boxed runtime value that preserves inner type metadata
    Boxed(Box<Type>),

    /// Any type (top type)
    Any,
}

impl Type {
    pub fn parse(s: &str) -> Option<Type> {
        let s = s.trim();

        // Handle primitive types
        match s {
            "Int" => return Some(Type::Int),
            "Float" => return Some(Type::Float),
            "String" => return Some(Type::String),
            "Bool" => return Some(Type::Bool),
            "Nil" => return Some(Type::Nil),
            "Any" => return Some(Type::Any),
            _ => {}
        }

        // Handle optional types: Int? (allow trailing whitespace before '?').
        let s_no_ws = s.trim_end();
        if let Some(inner) = s_no_ws.strip_suffix('?') {
            let inner = inner.trim_end();
            if !inner.is_empty() {
                return Type::parse(inner).map(|t| Type::Optional(Box::new(t)));
            }
        }

        // Handle type variables: 'T, 'K, 'V
        if s.starts_with('\'') && s.len() > 1 {
            return Some(Type::Variable(s[1..].to_string()));
        }

        // Handle union types: Int | String | Nil
        if s.contains(" | ") {
            let mut types = Vec::new();
            for part in s.split(" | ") {
                if let Some(ty) = Type::parse(part) {
                    types.push(ty);
                }
            }
            if !types.is_empty() {
                return Some(Type::Union(types));
            }
        }

        // Handle generic types with angle brackets
        if let Some(open) = s.find('<')
            && let Some(close) = s.rfind('>')
        {
            let base = &s[..open];
            if !is_type_name(base) {
                return None;
            }
            let params_str = &s[open + 1..close];

            // Parse type parameters
            let params: Vec<Type> = if params_str.is_empty() {
                vec![]
            } else {
                let mut params = Vec::new();
                for param in params_str.split(',').map(str::trim) {
                    params.push(Type::parse(param)?);
                }
                params
            };

            // Handle specific generic types
            match base {
                "List" => {
                    if params.len() == 1 {
                        return Some(Type::List(Box::new(params[0].clone())));
                    }
                }
                "Map" => {
                    if params.len() == 2 {
                        return Some(Type::Map(Box::new(params[0].clone()), Box::new(params[1].clone())));
                    }
                }
                "Set" => {
                    if params.len() == 1 {
                        return Some(Type::Set(Box::new(params[0].clone())));
                    }
                }
                "Task" => {
                    if params.len() == 1 {
                        return Some(Type::Task(Box::new(params[0].clone())));
                    }
                }
                "Channel" => {
                    if params.len() == 1 {
                        return Some(Type::Channel(Box::new(params[0].clone())));
                    }
                }
                "Box" | "Boxed" => {
                    if params.len() == 1 {
                        return Some(Type::Boxed(Box::new(params[0].clone())));
                    }
                }
                _ => {
                    // Generic custom type
                    return Some(Type::Generic {
                        name: base.to_string(),
                        params,
                    });
                }
            }
        }

        // Handle function types: (Int, String) -> Bool
        if s.contains("->") {
            if let Some((params_str, return_str)) = s.split_once("->") {
                let params_str = params_str.trim();
                let return_str = return_str.trim();

                // Parse parameters
                let params = if params_str.starts_with('(') && params_str.ends_with(')') {
                    let inner = &params_str[1..params_str.len() - 1];
                    if inner.is_empty() {
                        vec![]
                    } else {
                        let mut params = Vec::new();
                        for param in inner.split(',').map(str::trim) {
                            if let Some(ty) = Type::parse(param) {
                                params.push(ty);
                            }
                        }
                        params
                    }
                } else {
                    vec![]
                };

                // Parse return type
                if let Some(return_type) = Type::parse(return_str) {
                    return Some(Type::Function {
                        params,
                        named_params: Vec::new(),
                        return_type: Box::new(return_type),
                    });
                }
            }
        }

        // Handle bare List and Map as generic types
        match s {
            "List" => Some(Type::List(Box::new(Type::Any))),
            "Map" => Some(Type::Map(Box::new(Type::Any), Box::new(Type::Any))),
            "Set" => Some(Type::Set(Box::new(Type::Any))),
            _ => {
                // Assume it's a named custom type
                if is_type_name(s) {
                    Some(Type::Named(s.to_string()))
                } else {
                    None
                }
            }
        }
    }

    /// Get a display representation of the type
    pub fn display(&self) -> String {
        match self {
            Type::Int => "Int".to_string(),
            Type::Float => "Float".to_string(),
            Type::String => "String".to_string(),
            Type::Bool => "Bool".to_string(),
            Type::Nil => "Nil".to_string(),
            Type::Any => "Any".to_string(),
            Type::List(elem) => format!("List<{}>", elem.display()),
            Type::Map(k, v) => format!("Map<{}, {}>", k.display(), v.display()),
            Type::Set(elem) => format!("Set<{}>", elem.display()),
            Type::Tuple(elems) => {
                if elems.is_empty() {
                    "Tuple<>".to_string()
                } else {
                    let mut parts = Vec::with_capacity(elems.len());
                    for elem in elems {
                        parts.push(elem.display());
                    }
                    format!("Tuple<{}>", parts.join(", "))
                }
            }
            Type::Function {
                params,
                named_params,
                return_type,
            } => {
                let mut segments: Vec<String> = Vec::new();
                if !params.is_empty() {
                    for param in params {
                        segments.push(param.display());
                    }
                }
                if !named_params.is_empty() {
                    let mut named_parts = Vec::with_capacity(named_params.len());
                    for np in named_params {
                        let mut s = format!("{}: {}", np.name, np.ty.display());
                        if np.has_default {
                            s.push_str(" = _");
                        }
                        named_parts.push(s);
                    }
                    segments.push(format!("{{{}}}", named_parts.join(", ")));
                }
                format!("({}) -> {}", segments.join(", "), return_type.display())
            }
            Type::Task(inner) => format!("Task<{}>", inner.display()),
            Type::Channel(inner) => format!("Channel<{}>", inner.display()),
            Type::Union(types) => {
                let mut type_strs = Vec::with_capacity(types.len());
                for ty in types {
                    type_strs.push(ty.display());
                }
                type_strs.join(" | ")
            }
            Type::Optional(inner) => format!("{}?", inner.display()),
            Type::Variable(name) => format!("'{}", name),
            Type::Named(name) => name.clone(),
            Type::Generic { name, params } => {
                if params.is_empty() {
                    name.clone()
                } else {
                    let mut param_strs = Vec::with_capacity(params.len());
                    for param in params {
                        param_strs.push(param.display());
                    }
                    format!("{}<{}>", name, param_strs.join(", "))
                }
            }
            Type::Boxed(inner) => format!("Box<{}>", inner.display()),
        }
    }

    /// Check if this type can be assigned to another type (subtyping)
    pub fn is_assignable_to(&self, other: &Type) -> bool {
        match (self, other) {
            // Any type is assignable to Any
            (_, Type::Any) => true,
            // Any can flow into any type (dynamic fallback)
            (Type::Any, _) => true,
            // Same types are assignable
            (a, b) if a == b => true,
            // Boxed types act as transparent wrappers — must come before numeric hierarchy
            // so that Box<Any> unwraps to Any before numeric ordering is applied.
            (Type::Boxed(inner), Type::Boxed(expected)) => inner.is_assignable_to(expected),
            (Type::Boxed(inner), expected) => inner.is_assignable_to(expected),
            (actual, Type::Boxed(expected)) => actual.is_assignable_to(expected),
            // Numeric hierarchy: allow Int -> Float, Float -> Boxed, etc.
            (lhs, rhs) if lhs.numeric_class().is_some() && rhs.numeric_class().is_some() => {
                let lhs_class = lhs.numeric_class().unwrap();
                let rhs_class = rhs.numeric_class().unwrap();
                lhs_class <= rhs_class
            }
            (Type::Nil, Type::Optional(_)) => true,
            // Optional types: T is assignable to ?T
            (inner, Type::Optional(expected_inner)) => inner.is_assignable_to(expected_inner),
            // Union types: T is assignable to Union if T is assignable to any member
            (t, Type::Union(union_types)) => union_types.iter().any(|ut| t.is_assignable_to(ut)),
            // Union member is assignable to union
            (Type::Union(union_types), target) => union_types.iter().all(|ut| ut.is_assignable_to(target)),
            // Generic containers with covariant element types
            (Type::List(a), Type::List(b)) => a.is_assignable_to(b),
            (Type::Map(ak, av), Type::Map(bk, bv)) => ak.is_assignable_to(bk) && av.is_assignable_to(bv),
            (Type::Set(a), Type::Set(b)) => a.is_assignable_to(b),
            (Type::Tuple(as_), Type::Tuple(bs)) => {
                as_.len() == bs.len() && as_.iter().zip(bs.iter()).all(|(a, b)| a.is_assignable_to(b))
            }
            // Function types (contravariant parameters, covariant return)
            (
                Type::Function {
                    params: a_params,
                    named_params: a_named,
                    return_type: a_ret,
                },
                Type::Function {
                    params: b_params,
                    named_params: b_named,
                    return_type: b_ret,
                },
            ) => {
                if a_params.len() != b_params.len() {
                    false
                } else {
                    // Parameters are contravariant
                    let params_compatible = b_params
                        .iter()
                        .zip(a_params.iter())
                        .all(|(b_param, a_param)| b_param.is_assignable_to(a_param));
                    if !params_compatible {
                        return false;
                    }

                    if a_named.len() != b_named.len() {
                        return false;
                    }
                    let mut a_map =
                        std::collections::HashMap::<&str, &FunctionNamedParamType>::with_capacity(a_named.len());
                    for np in a_named {
                        a_map.insert(np.name.as_str(), np);
                    }
                    let named_compatible = b_named.iter().all(|b_np| {
                        if let Some(a_np) = a_map.get(b_np.name.as_str()) {
                            b_np.has_default == a_np.has_default && b_np.ty.is_assignable_to(&a_np.ty)
                        } else {
                            false
                        }
                    });
                    if !named_compatible {
                        return false;
                    }
                    // Return type is covariant
                    let return_compatible = a_ret.is_assignable_to(b_ret);
                    params_compatible && named_compatible && return_compatible
                }
            }
            // Concurrency types
            (Type::Task(a), Type::Task(b)) => a.is_assignable_to(b),
            (Type::Channel(a), Type::Channel(b)) => a.is_assignable_to(b),
            // No other assignability rules
            _ => false,
        }
    }

    /// Map type into numeric hierarchy class when applicable.
    pub fn numeric_class(&self) -> Option<NumericClass> {
        NumericHierarchy::classify(self)
    }

    /// Check if this type contains any type variables
    pub fn contains_variables(&self) -> bool {
        match self {
            Type::Variable(_) => true,
            Type::List(inner) | Type::Set(inner) | Type::Optional(inner) | Type::Task(inner) | Type::Channel(inner) => {
                inner.contains_variables()
            }
            Type::Map(k, v) => k.contains_variables() || v.contains_variables(),
            Type::Function {
                params,
                named_params,
                return_type,
            } => {
                params.iter().any(|p| p.contains_variables())
                    || named_params.iter().any(|np| np.ty.contains_variables())
                    || return_type.contains_variables()
            }
            Type::Union(types) => types.iter().any(|t| t.contains_variables()),
            Type::Tuple(elems) => elems.iter().any(|t| t.contains_variables()),
            Type::Generic { params, .. } => params.iter().any(|p| p.contains_variables()),
            Type::Boxed(inner) => inner.contains_variables(),
            _ => false,
        }
    }

    /// Substitute type variables with concrete types
    pub fn substitute(&self, substitutions: &std::collections::HashMap<String, Type>) -> Type {
        match self {
            Type::Variable(name) => substitutions.get(name).cloned().unwrap_or_else(|| self.clone()),
            Type::List(inner) => Type::List(Box::new(inner.substitute(substitutions))),
            Type::Set(inner) => Type::Set(Box::new(inner.substitute(substitutions))),
            Type::Map(k, v) => Type::Map(
                Box::new(k.substitute(substitutions)),
                Box::new(v.substitute(substitutions)),
            ),
            Type::Function {
                params,
                named_params,
                return_type,
            } => Type::Function {
                params: {
                    let mut out = Vec::with_capacity(params.len());
                    for param in params {
                        out.push(param.substitute(substitutions));
                    }
                    out
                },
                named_params: {
                    let mut out = Vec::with_capacity(named_params.len());
                    for np in named_params {
                        out.push(FunctionNamedParamType {
                            name: np.name.clone(),
                            ty: np.ty.substitute(substitutions),
                            has_default: np.has_default,
                        });
                    }
                    out
                },
                return_type: Box::new(return_type.substitute(substitutions)),
            },
            Type::Tuple(elems) => {
                let mut out = Vec::with_capacity(elems.len());
                for elem in elems {
                    out.push(elem.substitute(substitutions));
                }
                Type::Tuple(out)
            }
            Type::Optional(inner) => Type::Optional(Box::new(inner.substitute(substitutions))),
            Type::Task(inner) => Type::Task(Box::new(inner.substitute(substitutions))),
            Type::Channel(inner) => Type::Channel(Box::new(inner.substitute(substitutions))),
            Type::Union(types) => {
                let mut out = Vec::with_capacity(types.len());
                for ty in types {
                    out.push(ty.substitute(substitutions));
                }
                Type::Union(out)
            }
            Type::Generic { name, params } => Type::Generic {
                name: name.clone(),
                params: {
                    let mut out = Vec::with_capacity(params.len());
                    for param in params {
                        out.push(param.substitute(substitutions));
                    }
                    out
                },
            },
            Type::Boxed(inner) => Type::Boxed(Box::new(inner.substitute(substitutions))),
            _ => self.clone(),
        }
    }
}

fn is_type_name(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::{ShortStr, ShortStrOrStr};

    #[test]
    fn short_str_concat_int_falls_back_when_prefix_fills_inline_buffer() {
        let prefix = ShortStr::new("answer=").expect("short");

        let value = prefix.concat_int(42);

        match value {
            ShortStrOrStr::Str(value) => assert_eq!(value, "answer=42"),
            ShortStrOrStr::Short(value) => panic!("expected heap string fallback, got {}", value.as_str()),
        }
    }
}
