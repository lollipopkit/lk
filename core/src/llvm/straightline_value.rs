mod equality;
mod modules;

use crate::{
    val::ShortStr,
    vm::{ConstHeapValue32Data, ConstRuntimeValue32Data, Opcode32, RuntimeMapKeyData},
};

use super::const_display::{
    native_const_list_display, native_const_map_display, native_const_object_display, native_string_const_value,
};
use equality::{
    native_const_runtime_eq, native_map_entries_are_string_keyed, native_map_entry_keys_match, native_map_key_str,
    native_map_keys_match, native_static_value_eq,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum NativeStringKeyKind {
    Short,
    Heap,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum NativeListElementKind {
    I64,
    Text,
    #[allow(dead_code)]
    StrPtr,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum NativeMapKeyKind {
    Str,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum NativeMapValueKind {
    I64,
}

#[derive(Clone)]
pub(super) enum NativeStraightlineValue {
    I64(String),
    F64(String),
    Bool(String),
    Nil,
    String {
        symbol: String,
        value: String,
        len: usize,
        key_kind: NativeStringKeyKind,
    },
    StringPtr(String),
    Text(Vec<NativeTextPart>),
    DynamicTextChar,
    DynamicSplitText {
        text: Vec<NativeTextPart>,
        delimiter: String,
    },
    List {
        symbol: String,
        value: String,
        elements: Vec<ConstRuntimeValue32Data>,
    },
    Map {
        symbol: String,
        value: String,
        entries: Vec<(RuntimeMapKeyData, ConstRuntimeValue32Data)>,
    },
    DynamicMap {
        id: usize,
        key: NativeMapKeyKind,
        value: NativeMapValueKind,
    },
    DynamicList {
        id: usize,
        element: NativeListElementKind,
    },
    DynamicConstListElement {
        elements: Vec<ConstRuntimeValue32Data>,
        index: String,
    },
    DynamicJoinedText {
        id: usize,
        delimiter_len: usize,
    },
    Channel {
        elements: Vec<ConstRuntimeValue32Data>,
    },
    ArgList {
        elements: Vec<NativeStraightlineValue>,
    },
    Object {
        symbol: String,
        value: String,
        type_name: String,
        fields: Vec<(String, ConstRuntimeValue32Data)>,
    },
    Cell {
        symbol: String,
        value: Box<NativeStraightlineValue>,
    },
    Error {
        symbol: String,
    },
    Builtin(NativeBuiltin),
    Module(NativeModule),
    Function(u16),
    Closure {
        function_index: u16,
        captures: Vec<NativeStraightlineValue>,
    },
}

#[derive(Clone)]
pub(super) enum NativeTextPart {
    I64(String),
    F64(String),
    Bool(String),
    Nil,
    StrPtr(String),
    String { symbol: String, value: String },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum NativeBuiltin {
    Print,
    Println,
    BitAnd,
    BitNot,
    BitOr,
    Chan,
    CoreCallMethod,
    CoreMakeStruct,
    CoreMergeFields,
    CoreRegisterTrait,
    CoreRegisterTraitImpl,
    CoreTypeof,
    Recv,
    DatetimeAdd,
    DatetimeDayOfWeek,
    DatetimeDayOfYear,
    DatetimeFormat,
    DatetimeIsWeekend,
    DatetimeNow,
    DatetimeSub,
    OsClock,
    OsEpoch,
    OsHostname,
    OsArch,
    OsName,
    OsDirCurrent,
    OsDirTemp,
    OsDirList,
    IterRange,
    IterMap,
    IterFilter,
    IterReduce,
    IterTake,
    IterSkip,
    IterChain,
    IterFlatten,
    IterUnique,
    IterChunk,
    IterEnumerate,
    IterZip,
    IoRead,
    IoStderrWrite,
    IoStdoutFlush,
    IoStdoutWrite,
    IoStdoutWriteln,
    JsonParse,
    TomlParse,
    TimeNow,
    TimeSleep,
    TimeSince,
    TcpClose,
    TcpConnect,
    TcpRead,
    TcpWrite,
    Send,
    StreamCollect,
    StreamFromList,
    StringLen,
    YamlParse,
    MathAbs,
    MathSqrt,
    MathFloor,
    MathCeil,
    MathRound,
    MathMin,
    MathMax,
    MathPow,
    MathExp,
    FibIterative,
    GreetingsMessage,
    MathlibDouble,
    MathSin,
    MathCos,
    MapDelete,
    MapSet,
    MapMutate,
    Panic,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum NativeModule {
    Datetime,
    Fib,
    Greetings,
    Os,
    OsEnv,
    Iter,
    Io,
    Json,
    Math,
    Mathlib,
    Map,
    Toml,
    Time,
    Tcp,
    Stream,
    String,
    Yaml,
}

pub(super) fn native_static_global(name: &str) -> Option<NativeStraightlineValue> {
    match name {
        "print" => Some(NativeStraightlineValue::Builtin(NativeBuiltin::Print)),
        "println" => Some(NativeStraightlineValue::Builtin(NativeBuiltin::Println)),
        "__lk_bit_and" => Some(NativeStraightlineValue::Builtin(NativeBuiltin::BitAnd)),
        "__lk_bit_not" => Some(NativeStraightlineValue::Builtin(NativeBuiltin::BitNot)),
        "__lk_bit_or" => Some(NativeStraightlineValue::Builtin(NativeBuiltin::BitOr)),
        "chan" => Some(NativeStraightlineValue::Builtin(NativeBuiltin::Chan)),
        "__lk_call_method" => Some(NativeStraightlineValue::Builtin(NativeBuiltin::CoreCallMethod)),
        "__lk_make_struct" => Some(NativeStraightlineValue::Builtin(NativeBuiltin::CoreMakeStruct)),
        "__lk_merge_fields" => Some(NativeStraightlineValue::Builtin(NativeBuiltin::CoreMergeFields)),
        "__lk_register_trait" => Some(NativeStraightlineValue::Builtin(NativeBuiltin::CoreRegisterTrait)),
        "__lk_register_trait_impl" => Some(NativeStraightlineValue::Builtin(NativeBuiltin::CoreRegisterTraitImpl)),
        "typeof" => Some(NativeStraightlineValue::Builtin(NativeBuiltin::CoreTypeof)),
        "panic" => Some(NativeStraightlineValue::Builtin(NativeBuiltin::Panic)),
        "recv" => Some(NativeStraightlineValue::Builtin(NativeBuiltin::Recv)),
        "send" => Some(NativeStraightlineValue::Builtin(NativeBuiltin::Send)),
        "datetime" => Some(NativeStraightlineValue::Module(NativeModule::Datetime)),
        "os" => Some(NativeStraightlineValue::Module(NativeModule::Os)),
        "iter" => Some(NativeStraightlineValue::Module(NativeModule::Iter)),
        "io" => Some(NativeStraightlineValue::Module(NativeModule::Io)),
        "json" => Some(NativeStraightlineValue::Module(NativeModule::Json)),
        "math" => Some(NativeStraightlineValue::Module(NativeModule::Math)),
        "m" => Some(NativeStraightlineValue::Module(NativeModule::Math)),
        "abs" => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathAbs)),
        "max" => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathMax)),
        "min" => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathMin)),
        "square_root" => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathSqrt)),
        "fib" => Some(NativeStraightlineValue::Module(NativeModule::Fib)),
        "iterative" => Some(NativeStraightlineValue::Builtin(NativeBuiltin::FibIterative)),
        "greetings" => Some(NativeStraightlineValue::Module(NativeModule::Greetings)),
        "mathlib" => Some(NativeStraightlineValue::Module(NativeModule::Mathlib)),
        "map" => Some(NativeStraightlineValue::Module(NativeModule::Map)),
        "toml" => Some(NativeStraightlineValue::Module(NativeModule::Toml)),
        "time" => Some(NativeStraightlineValue::Module(NativeModule::Time)),
        "tcp" => Some(NativeStraightlineValue::Module(NativeModule::Tcp)),
        "stream" => Some(NativeStraightlineValue::Module(NativeModule::Stream)),
        "string_mod" | "string" => Some(NativeStraightlineValue::Module(NativeModule::String)),
        "yaml" => Some(NativeStraightlineValue::Module(NativeModule::Yaml)),
        _ => None,
    }
}

pub(super) fn native_static_to_string_value(
    value: NativeStraightlineValue,
    symbol: String,
) -> Option<NativeStraightlineValue> {
    let value = match value {
        NativeStraightlineValue::Nil => "nil".to_string(),
        NativeStraightlineValue::Bool(value) if value == "0" => "false".to_string(),
        NativeStraightlineValue::Bool(value) if value == "1" => "true".to_string(),
        NativeStraightlineValue::I64(value) if !value.starts_with('%') => value,
        NativeStraightlineValue::F64(value) if !value.starts_with('%') && !value.starts_with("0x") => value,
        NativeStraightlineValue::String { value, .. } => value,
        NativeStraightlineValue::StringPtr(_)
        | NativeStraightlineValue::Text(_)
        | NativeStraightlineValue::DynamicSplitText { .. }
        | NativeStraightlineValue::DynamicTextChar => return None,
        NativeStraightlineValue::Cell { .. }
        | NativeStraightlineValue::DynamicMap {
            key: NativeMapKeyKind::Str,
            value: NativeMapValueKind::I64,
            ..
        }
        | NativeStraightlineValue::DynamicList { .. }
        | NativeStraightlineValue::DynamicConstListElement { .. }
        | NativeStraightlineValue::DynamicJoinedText { .. }
        | NativeStraightlineValue::Channel { .. }
        | NativeStraightlineValue::ArgList { .. }
        | NativeStraightlineValue::Error { .. }
        | NativeStraightlineValue::List { .. }
        | NativeStraightlineValue::Map { .. }
        | NativeStraightlineValue::Object { .. }
        | NativeStraightlineValue::Builtin(_)
        | NativeStraightlineValue::Module(_) => return None,
        NativeStraightlineValue::F64(_)
        | NativeStraightlineValue::Bool(_)
        | NativeStraightlineValue::I64(_)
        | NativeStraightlineValue::Function(_)
        | NativeStraightlineValue::Closure { .. } => return None,
    };
    Some(NativeStraightlineValue::String {
        len: value.chars().count(),
        symbol,
        key_kind: native_runtime_string_key_kind(&value),
        value,
    })
}

pub(super) fn native_static_string_starts_with(
    target: NativeStraightlineValue,
    prefix: NativeStraightlineValue,
) -> Option<NativeStraightlineValue> {
    let NativeStraightlineValue::String { value: target, .. } = target else {
        return None;
    };
    let NativeStraightlineValue::String { value: prefix, .. } = prefix else {
        return None;
    };
    Some(NativeStraightlineValue::Bool(
        i64::from(target.starts_with(&prefix)).to_string(),
    ))
}

pub(super) fn native_static_string_split(
    target: NativeStraightlineValue,
    delimiter: NativeStraightlineValue,
    symbol: String,
) -> Option<NativeStraightlineValue> {
    let NativeStraightlineValue::String { value: target, .. } = target else {
        return None;
    };
    let NativeStraightlineValue::String { value: delimiter, .. } = delimiter else {
        return None;
    };
    let elements = target
        .split(&delimiter)
        .map(native_const_string_value)
        .collect::<Vec<_>>();
    Some(NativeStraightlineValue::List {
        value: native_const_list_display(&elements)?,
        symbol,
        elements,
    })
}

pub(super) fn native_static_list_join(
    target: NativeStraightlineValue,
    separator: NativeStraightlineValue,
    symbol: String,
) -> Option<NativeStraightlineValue> {
    let NativeStraightlineValue::List { elements, .. } = target else {
        return None;
    };
    let NativeStraightlineValue::String { value: separator, .. } = separator else {
        return None;
    };
    let mut parts = Vec::with_capacity(elements.len());
    for value in elements {
        parts.push(native_const_runtime_string(value)?);
    }
    let value = parts.join(&separator);
    Some(NativeStraightlineValue::String {
        len: value.chars().count(),
        symbol,
        key_kind: native_runtime_string_key_kind(&value),
        value,
    })
}

pub(super) fn native_static_equality_bool(equal: bool, opcode: Opcode32) -> NativeStraightlineValue {
    let result = if opcode == Opcode32::CmpNeInt { !equal } else { equal };
    NativeStraightlineValue::Bool(i64::from(result).to_string())
}

pub(super) fn native_static_collection_equality_bool(
    lhs: &NativeStraightlineValue,
    rhs: &NativeStraightlineValue,
    opcode: Opcode32,
) -> Option<NativeStraightlineValue> {
    if !matches!(opcode, Opcode32::CmpInt | Opcode32::CmpNeInt) {
        return None;
    }
    let equal = match (lhs, rhs) {
        (NativeStraightlineValue::List { elements: lhs, .. }, NativeStraightlineValue::List { elements: rhs, .. }) => {
            lhs.len() == rhs.len()
                && lhs
                    .iter()
                    .zip(rhs.iter())
                    .all(|(lhs, rhs)| native_const_runtime_eq(lhs, rhs))
        }
        (NativeStraightlineValue::Map { entries: lhs, .. }, NativeStraightlineValue::Map { entries: rhs, .. }) => {
            lhs.len() == rhs.len()
                && lhs.iter().all(|(lhs_key, lhs_value)| {
                    rhs.iter()
                        .find(|(rhs_key, _)| rhs_key == lhs_key)
                        .is_some_and(|(_, rhs_value)| native_const_runtime_eq(lhs_value, rhs_value))
                })
        }
        (NativeStraightlineValue::Object { symbol: lhs, .. }, NativeStraightlineValue::Object { symbol: rhs, .. }) => {
            lhs == rhs
        }
        _ => return None,
    };
    Some(native_static_equality_bool(equal, opcode))
}

pub(super) fn native_static_truthy(value: &NativeStraightlineValue) -> Option<bool> {
    match value {
        NativeStraightlineValue::Nil => Some(false),
        NativeStraightlineValue::Bool(value) if !value.starts_with('%') => Some(value != "0"),
        NativeStraightlineValue::I64(value) | NativeStraightlineValue::F64(value) if !value.starts_with('%') => {
            Some(true)
        }
        NativeStraightlineValue::String { .. }
        | NativeStraightlineValue::StringPtr(_)
        | NativeStraightlineValue::Text(_)
        | NativeStraightlineValue::DynamicSplitText { .. }
        | NativeStraightlineValue::DynamicTextChar
        | NativeStraightlineValue::List { .. }
        | NativeStraightlineValue::Map { .. }
        | NativeStraightlineValue::DynamicMap {
            key: NativeMapKeyKind::Str,
            value: NativeMapValueKind::I64,
            ..
        }
        | NativeStraightlineValue::DynamicList { .. }
        | NativeStraightlineValue::DynamicConstListElement { .. }
        | NativeStraightlineValue::DynamicJoinedText { .. }
        | NativeStraightlineValue::Channel { .. }
        | NativeStraightlineValue::Object { .. }
        | NativeStraightlineValue::Cell { .. }
        | NativeStraightlineValue::Error { .. } => Some(true),
        NativeStraightlineValue::Bool(_) | NativeStraightlineValue::I64(_) | NativeStraightlineValue::F64(_) => None,
        NativeStraightlineValue::Builtin(_)
        | NativeStraightlineValue::Module(_)
        | NativeStraightlineValue::Function(_)
        | NativeStraightlineValue::ArgList { .. }
        | NativeStraightlineValue::Closure { .. } => None,
    }
}

pub(super) fn native_static_not(value: &NativeStraightlineValue) -> Option<NativeStraightlineValue> {
    match value {
        NativeStraightlineValue::Nil => Some(NativeStraightlineValue::Bool("1".to_string())),
        NativeStraightlineValue::Bool(value) if value == "0" => Some(NativeStraightlineValue::Bool("1".to_string())),
        NativeStraightlineValue::Bool(value) if value == "1" => Some(NativeStraightlineValue::Bool("0".to_string())),
        _ => None,
    }
}

pub(super) fn native_static_i64_binary(lhs: &str, rhs: &str, opcode: Opcode32) -> Option<String> {
    if lhs.starts_with('%') || rhs.starts_with('%') {
        return None;
    }
    let lhs = lhs.parse::<i64>().ok()?;
    let rhs = rhs.parse::<i64>().ok()?;
    let value = match opcode {
        Opcode32::AddInt => lhs.wrapping_add(rhs),
        Opcode32::SubInt => lhs.wrapping_sub(rhs),
        Opcode32::MulInt => lhs.wrapping_mul(rhs),
        Opcode32::DivInt if rhs != 0 => lhs.wrapping_div(rhs),
        Opcode32::ModInt if rhs != 0 => lhs.wrapping_rem(rhs),
        _ => return None,
    };
    Some(value.to_string())
}

pub(super) fn native_static_i64_divisor_nonzero(value: &str) -> Option<bool> {
    if value.starts_with('%') {
        return None;
    }
    Some(value.parse::<i64>().ok()? != 0)
}

pub(super) fn native_static_f64_binary(lhs: &str, rhs: &str, opcode: Opcode32) -> Option<String> {
    if lhs.starts_with('%') || rhs.starts_with('%') || lhs.starts_with("0x") || rhs.starts_with("0x") {
        return None;
    }
    let lhs = lhs.parse::<f64>().ok()?;
    let rhs = rhs.parse::<f64>().ok()?;
    let value = match opcode {
        Opcode32::AddFloat => lhs + rhs,
        Opcode32::SubFloat => lhs - rhs,
        Opcode32::MulFloat => lhs * rhs,
        Opcode32::DivFloat if rhs != 0.0 => lhs / rhs,
        Opcode32::ModFloat if rhs != 0.0 => lhs % rhs,
        _ => return None,
    };
    Some(super::ir_text::llvm_float_literal(value))
}

pub(super) fn native_static_f64_divisor_nonzero(value: &str) -> Option<bool> {
    if value.starts_with('%') || value.starts_with("0x") {
        return None;
    }
    Some(value.parse::<f64>().ok()? != 0.0)
}

pub(super) fn native_static_compare_bool(
    lhs: &NativeStraightlineValue,
    rhs: &NativeStraightlineValue,
    opcode: Opcode32,
) -> Option<bool> {
    match (lhs, rhs) {
        (NativeStraightlineValue::I64(lhs), NativeStraightlineValue::I64(rhs)) => {
            let lhs = native_static_i64(lhs)?;
            let rhs = native_static_i64(rhs)?;
            Some(match opcode {
                Opcode32::CmpInt => lhs == rhs,
                Opcode32::CmpNeInt => lhs != rhs,
                Opcode32::CmpLtInt => lhs < rhs,
                Opcode32::CmpLeInt => lhs <= rhs,
                Opcode32::CmpGtInt => lhs > rhs,
                Opcode32::CmpGeInt => lhs >= rhs,
                _ => return None,
            })
        }
        (NativeStraightlineValue::F64(lhs), NativeStraightlineValue::F64(rhs)) => {
            let lhs = native_static_f64(lhs)?;
            let rhs = native_static_f64(rhs)?;
            Some(match opcode {
                Opcode32::CmpInt => lhs == rhs,
                Opcode32::CmpNeInt => lhs != rhs,
                Opcode32::CmpLtInt => lhs < rhs,
                Opcode32::CmpLeInt => lhs <= rhs,
                Opcode32::CmpGtInt => lhs > rhs,
                Opcode32::CmpGeInt => lhs >= rhs,
                _ => return None,
            })
        }
        (NativeStraightlineValue::Bool(lhs), NativeStraightlineValue::Bool(rhs))
            if matches!(opcode, Opcode32::CmpInt | Opcode32::CmpNeInt) =>
        {
            let equal = lhs == rhs;
            Some(if opcode == Opcode32::CmpNeInt { !equal } else { equal })
        }
        (NativeStraightlineValue::String { value: lhs, .. }, NativeStraightlineValue::String { value: rhs, .. }) => {
            Some(match opcode {
                Opcode32::CmpInt => lhs == rhs,
                Opcode32::CmpNeInt => lhs != rhs,
                Opcode32::CmpLtInt => lhs < rhs,
                Opcode32::CmpLeInt => lhs <= rhs,
                Opcode32::CmpGtInt => lhs > rhs,
                Opcode32::CmpGeInt => lhs >= rhs,
                _ => return None,
            })
        }
        (NativeStraightlineValue::Text(lhs), NativeStraightlineValue::String { value: rhs, .. })
        | (NativeStraightlineValue::String { value: rhs, .. }, NativeStraightlineValue::Text(lhs))
            if matches!(opcode, Opcode32::CmpInt | Opcode32::CmpNeInt) =>
        {
            let lhs = native_static_text_string(lhs)?;
            let equal = &lhs == rhs;
            Some(if opcode == Opcode32::CmpNeInt { !equal } else { equal })
        }
        (NativeStraightlineValue::Text(lhs), NativeStraightlineValue::Text(rhs))
            if matches!(opcode, Opcode32::CmpInt | Opcode32::CmpNeInt) =>
        {
            let equal = native_static_text_string(lhs)? == native_static_text_string(rhs)?;
            Some(if opcode == Opcode32::CmpNeInt { !equal } else { equal })
        }
        (NativeStraightlineValue::Nil, NativeStraightlineValue::Nil)
            if matches!(opcode, Opcode32::CmpInt | Opcode32::CmpNeInt) =>
        {
            Some(opcode == Opcode32::CmpInt)
        }
        (NativeStraightlineValue::Nil, _) | (_, NativeStraightlineValue::Nil)
            if matches!(opcode, Opcode32::CmpInt | Opcode32::CmpNeInt) =>
        {
            Some(opcode == Opcode32::CmpNeInt)
        }
        (NativeStraightlineValue::List { .. }, NativeStraightlineValue::List { .. })
        | (NativeStraightlineValue::Map { .. }, NativeStraightlineValue::Map { .. })
            if matches!(opcode, Opcode32::CmpInt | Opcode32::CmpNeInt) =>
        {
            let NativeStraightlineValue::Bool(value) = native_static_collection_equality_bool(lhs, rhs, opcode)? else {
                return None;
            };
            Some(value == "1")
        }
        _ => None,
    }
}

pub(super) fn native_static_text_string(parts: &[NativeTextPart]) -> Option<String> {
    let mut out = String::new();
    for part in parts {
        match part {
            NativeTextPart::I64(value) if !value.starts_with('%') => out.push_str(value),
            NativeTextPart::F64(value) if !value.starts_with('%') && !value.starts_with("0x") => out.push_str(value),
            NativeTextPart::Bool(value) if value == "0" => out.push_str("false"),
            NativeTextPart::Bool(value) if value == "1" => out.push_str("true"),
            NativeTextPart::Nil => out.push_str("nil"),
            NativeTextPart::String { value, .. } => out.push_str(value),
            NativeTextPart::StrPtr(_) | NativeTextPart::Bool(_) | NativeTextPart::I64(_) | NativeTextPart::F64(_) => {
                return None;
            }
        }
    }
    Some(out)
}

pub(super) fn native_static_alias_symbol(value: &NativeStraightlineValue) -> Option<&str> {
    match value {
        NativeStraightlineValue::List { symbol, .. }
        | NativeStraightlineValue::Map { symbol, .. }
        | NativeStraightlineValue::Object { symbol, .. }
        | NativeStraightlineValue::Cell { symbol, .. } => Some(symbol),
        NativeStraightlineValue::Error { symbol, .. } => Some(symbol),
        _ => None,
    }
}

pub(super) fn native_static_container_test(
    value: NativeStraightlineValue,
    opcode: Opcode32,
) -> Option<NativeStraightlineValue> {
    let matched = match opcode {
        Opcode32::IsList => matches!(
            value,
            NativeStraightlineValue::List { .. }
                | NativeStraightlineValue::String { .. }
                | NativeStraightlineValue::DynamicList { .. }
        ),
        Opcode32::IsMap => matches!(value, NativeStraightlineValue::Map { .. }),
        _ => return None,
    };
    Some(NativeStraightlineValue::Bool(i64::from(matched).to_string()))
}

pub(super) fn native_static_len(value: NativeStraightlineValue) -> Option<NativeStraightlineValue> {
    let len = match value {
        NativeStraightlineValue::String { len, .. } => len,
        NativeStraightlineValue::List { elements, .. } => elements.len(),
        NativeStraightlineValue::Map { entries, .. } => entries.len(),
        _ => return None,
    };
    Some(NativeStraightlineValue::I64(len.to_string()))
}

pub(super) fn native_static_list_from_values(
    values: &[NativeStraightlineValue],
    symbol: String,
) -> Option<NativeStraightlineValue> {
    let elements = values
        .iter()
        .map(native_runtime_const_value)
        .collect::<Option<Vec<_>>>()?;
    Some(NativeStraightlineValue::List {
        value: native_const_list_display(&elements)?,
        symbol,
        elements,
    })
}

pub(super) fn native_static_map_from_pairs(
    pairs: &[(NativeStraightlineValue, NativeStraightlineValue)],
    symbol: String,
) -> Option<NativeStraightlineValue> {
    let entries = pairs
        .iter()
        .map(|(key, value)| Some((native_map_key(key.clone())?, native_runtime_const_value(value)?)))
        .collect::<Option<Vec<_>>>()?;
    Some(NativeStraightlineValue::Map {
        value: native_const_map_display(&entries)?,
        symbol,
        entries,
    })
}

pub(super) fn native_static_map_rest(
    target: NativeStraightlineValue,
    removed_keys: &[NativeStraightlineValue],
    symbol: String,
) -> Option<NativeStraightlineValue> {
    let NativeStraightlineValue::Map { entries, .. } = target else {
        return None;
    };
    let removed_keys = removed_keys
        .iter()
        .cloned()
        .map(native_map_key)
        .collect::<Option<Vec<_>>>()?;
    let entries = entries
        .into_iter()
        .filter(|(key, _)| !removed_keys.iter().any(|removed| native_map_keys_match(key, removed)))
        .collect::<Vec<_>>();
    Some(NativeStraightlineValue::Map {
        value: native_const_map_display(&entries)?,
        symbol,
        entries,
    })
}

pub(super) fn native_static_object_from_fields(
    values: &[NativeStraightlineValue],
    symbol: String,
) -> Option<NativeStraightlineValue> {
    let [NativeStraightlineValue::String { value: type_name, .. }, fields @ ..] = values else {
        return None;
    };
    let mut out = Vec::with_capacity(fields.len() / 2);
    for pair in fields.chunks_exact(2) {
        let NativeStraightlineValue::String { value: key, .. } = &pair[0] else {
            return None;
        };
        out.push((key.clone(), native_runtime_const_value(&pair[1])?));
    }
    Some(NativeStraightlineValue::Object {
        value: native_const_object_display(type_name, &out)?,
        symbol,
        type_name: type_name.clone(),
        fields: out,
    })
}

pub(super) fn native_static_make_struct(
    args: &[NativeStraightlineValue],
    symbol: String,
) -> Option<NativeStraightlineValue> {
    if let [
        NativeStraightlineValue::String { value: type_name, .. },
        NativeStraightlineValue::Object { fields, .. },
    ] = args
    {
        return Some(NativeStraightlineValue::Object {
            value: native_const_object_display(type_name, fields)?,
            symbol,
            type_name: type_name.clone(),
            fields: fields.clone(),
        });
    }
    let [
        NativeStraightlineValue::String { value: type_name, .. },
        NativeStraightlineValue::Map { entries, .. },
    ] = args
    else {
        return None;
    };
    let mut fields = Vec::with_capacity(entries.len());
    for (key, value) in entries {
        fields.push((native_map_key_str(key)?.to_string(), value.clone()));
    }
    Some(NativeStraightlineValue::Object {
        value: native_const_object_display(type_name, &fields)?,
        symbol,
        type_name: type_name.clone(),
        fields,
    })
}

pub(super) fn native_static_merge_fields(
    args: &[NativeStraightlineValue],
    symbol: String,
) -> Option<NativeStraightlineValue> {
    let [base, NativeStraightlineValue::Map { entries: overlay, .. }] = args else {
        return None;
    };
    match base {
        NativeStraightlineValue::Nil => Some(NativeStraightlineValue::Map {
            value: native_const_map_display(overlay)?,
            symbol,
            entries: overlay.clone(),
        }),
        NativeStraightlineValue::Map { entries: base, .. } => {
            let mut entries = base.clone();
            for (key, value) in overlay {
                if let Some((_, slot)) = entries
                    .iter_mut()
                    .find(|(entry_key, _)| native_map_keys_match(entry_key, key))
                {
                    *slot = value.clone();
                } else {
                    entries.push((key.clone(), value.clone()));
                }
            }
            Some(NativeStraightlineValue::Map {
                value: native_const_map_display(&entries)?,
                symbol,
                entries,
            })
        }
        NativeStraightlineValue::Object { type_name, fields, .. } => {
            let mut fields = fields.clone();
            for (key, value) in overlay {
                let key = native_map_key_str(key)?.to_string();
                if let Some((_, slot)) = fields.iter_mut().find(|(field_key, _)| *field_key == key) {
                    *slot = value.clone();
                } else {
                    fields.push((key, value.clone()));
                }
            }
            Some(NativeStraightlineValue::Object {
                value: native_const_object_display(type_name, &fields)?,
                symbol,
                type_name: type_name.clone(),
                fields,
            })
        }
        _ => None,
    }
}

pub(super) fn native_static_int_range(
    start: NativeStraightlineValue,
    end: NativeStraightlineValue,
    step: NativeStraightlineValue,
    inclusive: bool,
    symbol: String,
) -> Option<NativeStraightlineValue> {
    let start = native_i64_const_value(start)?;
    let end = native_i64_const_value(end)?;
    let step = native_i64_const_value(step)?;
    if step == 0 {
        return None;
    }

    let mut elements = Vec::new();
    let mut current = start;
    if step > 0 {
        while if inclusive { current <= end } else { current < end } {
            elements.push(ConstRuntimeValue32Data::Int(current));
            current = current.checked_add(step)?;
        }
    } else {
        while if inclusive { current >= end } else { current > end } {
            elements.push(ConstRuntimeValue32Data::Int(current));
            current = current.checked_add(step)?;
        }
    }

    Some(NativeStraightlineValue::List {
        value: native_const_list_display(&elements)?,
        symbol,
        elements,
    })
}

pub(super) fn native_static_index(
    target: NativeStraightlineValue,
    key: NativeStraightlineValue,
    symbol: String,
) -> Option<NativeStraightlineValue> {
    match target {
        NativeStraightlineValue::Nil => Some(NativeStraightlineValue::Nil),
        NativeStraightlineValue::String { value, .. } => {
            if let NativeStraightlineValue::List { elements, .. } = key {
                let out = native_static_string_slice(&value, &elements)?;
                return Some(NativeStraightlineValue::String {
                    symbol,
                    len: out.chars().count(),
                    key_kind: native_runtime_string_key_kind(&out),
                    value: out,
                });
            }
            let index = native_i64_const_index_for_len(key, value.len())?;
            let Some(ch) = value.chars().nth(index) else {
                return Some(NativeStraightlineValue::Nil);
            };
            Some(NativeStraightlineValue::String {
                symbol,
                value: ch.to_string(),
                len: 1,
                key_kind: NativeStringKeyKind::Short,
            })
        }
        NativeStraightlineValue::List { elements, .. } => {
            if let NativeStraightlineValue::List {
                elements: key_elements, ..
            } = key
            {
                let elements = native_static_list_slice(&elements, &key_elements)?;
                return Some(NativeStraightlineValue::List {
                    value: native_const_list_display(&elements)?,
                    symbol,
                    elements,
                });
            }
            let index = native_i64_const_index_for_len(key, elements.len())?;
            let Some(value) = elements.get(index) else {
                return Some(NativeStraightlineValue::Nil);
            };
            native_const_runtime_value(value, symbol)
        }
        NativeStraightlineValue::ArgList { elements } => {
            let index = native_i64_const_index_for_len(key, elements.len())?;
            elements.get(index).cloned()
        }
        NativeStraightlineValue::Map { entries, .. } => {
            if native_map_entries_are_string_keyed(&entries) {
                let Some(key) = native_string_key_value(key) else {
                    return Some(NativeStraightlineValue::Nil);
                };
                let Some((_, value)) = entries
                    .iter()
                    .find(|(entry_key, _)| native_map_key_str(entry_key).is_some_and(|entry_key| entry_key == key))
                else {
                    return Some(NativeStraightlineValue::Nil);
                };
                return native_const_runtime_value(value, symbol);
            }
            let key = native_map_key(key)?;
            let Some((_, value)) = entries.iter().find(|(entry_key, _)| *entry_key == key) else {
                return Some(NativeStraightlineValue::Nil);
            };
            native_const_runtime_value(value, symbol)
        }
        NativeStraightlineValue::Object { fields, .. } => {
            let NativeStraightlineValue::String { value: key, .. } = key else {
                return None;
            };
            let Some((_, value)) = fields.iter().find(|(field_key, _)| *field_key == key) else {
                return Some(NativeStraightlineValue::Nil);
            };
            native_const_runtime_value(value, symbol)
        }
        NativeStraightlineValue::Module(module) => native_static_module_index(module, key),
        _ => None,
    }
}

fn native_static_module_index(module: NativeModule, key: NativeStraightlineValue) -> Option<NativeStraightlineValue> {
    modules::native_static_module_index(module, key)
}

pub(super) fn native_static_map_delete(
    target: NativeStraightlineValue,
    key: NativeStraightlineValue,
    symbol: String,
) -> Option<NativeStraightlineValue> {
    let NativeStraightlineValue::Map { mut entries, .. } = target else {
        return None;
    };
    let key = native_map_key(key)?;
    let compare_string_keys = native_map_entries_are_string_keyed(&entries);
    let mut removed = ConstRuntimeValue32Data::Nil;
    entries.retain(|(entry_key, value)| {
        if native_map_entry_keys_match(entry_key, &key, compare_string_keys) {
            removed = value.clone();
            false
        } else {
            true
        }
    });
    let updated = ConstRuntimeValue32Data::Heap(Box::new(ConstHeapValue32Data::Map(entries)));
    let elements = vec![updated, removed];
    Some(NativeStraightlineValue::List {
        value: native_const_list_display(&elements)?,
        symbol,
        elements,
    })
}

pub(super) fn native_static_set_index(
    target: NativeStraightlineValue,
    key: NativeStraightlineValue,
    value: NativeStraightlineValue,
) -> Option<NativeStraightlineValue> {
    match target {
        NativeStraightlineValue::List {
            symbol, mut elements, ..
        } => {
            let index = native_i64_const_index(key)?;
            let slot = elements.get_mut(index)?;
            *slot = native_runtime_const_value(&value)?;
            Some(NativeStraightlineValue::List {
                value: native_const_list_display(&elements)?,
                symbol,
                elements,
            })
        }
        NativeStraightlineValue::Map {
            symbol, mut entries, ..
        } => {
            let key = native_map_key(key)?;
            let value = native_runtime_const_value(&value)?;
            let compare_string_keys = native_map_entries_are_string_keyed(&entries);
            if let Some((_, slot)) = entries
                .iter_mut()
                .find(|(entry_key, _)| native_map_entry_keys_match(entry_key, &key, compare_string_keys))
            {
                *slot = value;
            } else {
                entries.push((key, value));
            }
            Some(NativeStraightlineValue::Map {
                value: native_const_map_display(&entries)?,
                symbol,
                entries,
            })
        }
        NativeStraightlineValue::Object {
            symbol,
            type_name,
            mut fields,
            ..
        } => {
            let NativeStraightlineValue::String { value: key, .. } = key else {
                return None;
            };
            let value = native_runtime_const_value(&value)?;
            if let Some((_, slot)) = fields.iter_mut().find(|(field_key, _)| *field_key == key) {
                *slot = value;
            } else {
                fields.push((key, value));
            }
            Some(NativeStraightlineValue::Object {
                value: native_const_object_display(&type_name, &fields)?,
                symbol,
                type_name,
                fields,
            })
        }
        _ => None,
    }
}

pub(super) fn native_static_list_push(
    target: NativeStraightlineValue,
    value: NativeStraightlineValue,
) -> Option<NativeStraightlineValue> {
    let NativeStraightlineValue::List {
        symbol, mut elements, ..
    } = target
    else {
        return None;
    };
    elements.push(native_runtime_const_value(&value)?);
    Some(NativeStraightlineValue::List {
        value: native_const_list_display(&elements)?,
        symbol,
        elements,
    })
}

pub(super) fn native_static_load_cell(value: NativeStraightlineValue) -> Option<NativeStraightlineValue> {
    let NativeStraightlineValue::Cell { value, .. } = value else {
        return None;
    };
    Some(*value)
}

pub(super) fn native_static_store_cell(
    cell: NativeStraightlineValue,
    value: NativeStraightlineValue,
) -> Option<NativeStraightlineValue> {
    let NativeStraightlineValue::Cell { symbol, .. } = cell else {
        return None;
    };
    Some(NativeStraightlineValue::Cell {
        symbol,
        value: Box::new(value),
    })
}

pub(super) fn native_static_to_iter(value: NativeStraightlineValue, symbol: String) -> Option<NativeStraightlineValue> {
    match value {
        value @ NativeStraightlineValue::List { .. } => Some(value),
        NativeStraightlineValue::String { value, .. } => {
            let elements = value
                .chars()
                .map(|ch| ConstRuntimeValue32Data::ShortStr(ch.to_string()))
                .collect::<Vec<_>>();
            Some(NativeStraightlineValue::List {
                value: native_const_list_display(&elements)?,
                symbol,
                elements,
            })
        }
        NativeStraightlineValue::Map { mut entries, .. } => {
            entries.sort_by(|(lhs, _), (rhs, _)| lhs.cmp(rhs));
            let elements = entries
                .into_iter()
                .map(|(key, value)| {
                    Some(ConstRuntimeValue32Data::Heap(Box::new(ConstHeapValue32Data::List(
                        vec![native_map_key_const_value(key)?, value],
                    ))))
                })
                .collect::<Option<Vec<_>>>()?;
            Some(NativeStraightlineValue::List {
                value: native_const_list_display(&elements)?,
                symbol,
                elements,
            })
        }
        _ => None,
    }
}

pub(super) fn native_static_slice_from(
    target: NativeStraightlineValue,
    start: NativeStraightlineValue,
    symbol: String,
) -> Option<NativeStraightlineValue> {
    let start = native_i64_const_index(start)?;
    match target {
        NativeStraightlineValue::String { value, .. } => {
            let value = value.chars().skip(start).collect::<String>();
            Some(NativeStraightlineValue::String {
                len: value.chars().count(),
                symbol,
                key_kind: native_runtime_string_key_kind(&value),
                value,
            })
        }
        NativeStraightlineValue::List { elements, .. } => {
            let elements = elements.into_iter().skip(start).collect::<Vec<_>>();
            Some(NativeStraightlineValue::List {
                symbol,
                value: native_const_list_display(&elements)?,
                elements,
            })
        }
        _ => None,
    }
}

pub(super) fn native_static_contains(
    needle: NativeStraightlineValue,
    haystack: NativeStraightlineValue,
) -> Option<NativeStraightlineValue> {
    let contains = match haystack {
        NativeStraightlineValue::String { value, .. } => {
            let Some(needle) = native_string_key_value(needle) else {
                return Some(NativeStraightlineValue::Bool("0".to_string()));
            };
            value.contains(&needle)
        }
        NativeStraightlineValue::List { elements, .. } => elements
            .iter()
            .filter_map(|value| native_const_runtime_value(value, String::new()))
            .any(|value| native_static_value_eq(&value, &needle)),
        NativeStraightlineValue::Map { entries, .. } => {
            if native_map_entries_are_string_keyed(&entries) {
                let Some(needle) = native_string_map_key_value(needle) else {
                    return Some(NativeStraightlineValue::Bool("0".to_string()));
                };
                entries
                    .iter()
                    .filter_map(|(key, _)| native_map_key_str(key))
                    .any(|key| key == needle)
            } else {
                let needle = native_map_key(needle)?;
                entries.iter().any(|(key, _)| *key == needle)
            }
        }
        _ => return None,
    };
    Some(NativeStraightlineValue::Bool(i64::from(contains).to_string()))
}

pub(super) fn native_straightline_heap_const_value(
    function_index: usize,
    const_index: u16,
    value: &ConstHeapValue32Data,
) -> Option<NativeStraightlineValue> {
    match value {
        ConstHeapValue32Data::LongString(value) => Some(NativeStraightlineValue::String {
            symbol: format!("@lk_func{function_index}_heap_str_{const_index}"),
            len: value.chars().count(),
            key_kind: NativeStringKeyKind::Heap,
            value: native_string_const_value(value)?,
        }),
        ConstHeapValue32Data::List(values) => Some(NativeStraightlineValue::List {
            symbol: format!("@lk_func{function_index}_heap_list_{const_index}"),
            value: native_const_list_display(values)?,
            elements: values.clone(),
        }),
        ConstHeapValue32Data::Map(values) => Some(NativeStraightlineValue::Map {
            symbol: format!("@lk_func{function_index}_heap_map_{const_index}"),
            value: native_const_map_display(values)?,
            entries: values.clone(),
        }),
        ConstHeapValue32Data::UpvalCell(value) => Some(NativeStraightlineValue::Cell {
            symbol: format!("@lk_func{function_index}_heap_cell_{const_index}"),
            value: Box::new(native_const_runtime_value(
                value,
                format!("@lk_func{function_index}_heap_cell_value_{const_index}"),
            )?),
        }),
    }
}

pub(super) fn native_const_runtime_value(
    value: &ConstRuntimeValue32Data,
    symbol: String,
) -> Option<NativeStraightlineValue> {
    match value {
        ConstRuntimeValue32Data::Nil => Some(NativeStraightlineValue::Nil),
        ConstRuntimeValue32Data::Bool(value) => Some(NativeStraightlineValue::Bool(i64::from(*value).to_string())),
        ConstRuntimeValue32Data::Int(value) => Some(NativeStraightlineValue::I64(value.to_string())),
        ConstRuntimeValue32Data::Float(value) => {
            Some(NativeStraightlineValue::F64(super::ir_text::llvm_float_literal(*value)))
        }
        ConstRuntimeValue32Data::ShortStr(value) => Some(NativeStraightlineValue::String {
            symbol,
            len: value.chars().count(),
            key_kind: NativeStringKeyKind::Short,
            value: native_string_const_value(value)?,
        }),
        ConstRuntimeValue32Data::Heap(value) => match value.as_ref() {
            ConstHeapValue32Data::LongString(value) => Some(NativeStraightlineValue::String {
                symbol,
                len: value.chars().count(),
                key_kind: NativeStringKeyKind::Heap,
                value: native_string_const_value(value)?,
            }),
            ConstHeapValue32Data::List(values) => Some(NativeStraightlineValue::List {
                symbol,
                value: native_const_list_display(values)?,
                elements: values.clone(),
            }),
            ConstHeapValue32Data::Map(values) => Some(NativeStraightlineValue::Map {
                symbol,
                value: native_const_map_display(values)?,
                entries: values.clone(),
            }),
            ConstHeapValue32Data::UpvalCell(value) => Some(NativeStraightlineValue::Cell {
                symbol,
                value: Box::new(native_const_runtime_value(value, String::new())?),
            }),
        },
    }
}

pub(super) fn native_runtime_const_value(value: &NativeStraightlineValue) -> Option<ConstRuntimeValue32Data> {
    match value {
        NativeStraightlineValue::Nil => Some(ConstRuntimeValue32Data::Nil),
        NativeStraightlineValue::Bool(value) if !value.starts_with('%') => {
            Some(ConstRuntimeValue32Data::Bool(value != "0"))
        }
        NativeStraightlineValue::I64(value) if !value.starts_with('%') => {
            Some(ConstRuntimeValue32Data::Int(value.parse().ok()?))
        }
        NativeStraightlineValue::F64(value) if !value.starts_with('%') && !value.starts_with("0x") => {
            Some(ConstRuntimeValue32Data::Float(value.parse().ok()?))
        }
        NativeStraightlineValue::String { value, key_kind, .. } => {
            if *key_kind == NativeStringKeyKind::Short {
                Some(ConstRuntimeValue32Data::ShortStr(value.clone()))
            } else {
                Some(ConstRuntimeValue32Data::Heap(Box::new(
                    ConstHeapValue32Data::LongString(value.clone()),
                )))
            }
        }
        NativeStraightlineValue::List { elements, .. } => {
            let mut out = Vec::with_capacity(elements.len());
            out.extend(elements.iter().cloned());
            Some(ConstRuntimeValue32Data::Heap(Box::new(ConstHeapValue32Data::List(out))))
        }
        NativeStraightlineValue::Map { entries, .. } => {
            let mut out = Vec::with_capacity(entries.len());
            out.extend(entries.iter().cloned());
            Some(ConstRuntimeValue32Data::Heap(Box::new(ConstHeapValue32Data::Map(out))))
        }
        NativeStraightlineValue::Object { .. }
        | NativeStraightlineValue::Channel { .. }
        | NativeStraightlineValue::DynamicMap {
            key: NativeMapKeyKind::Str,
            value: NativeMapValueKind::I64,
            ..
        }
        | NativeStraightlineValue::DynamicList { .. }
        | NativeStraightlineValue::DynamicConstListElement { .. }
        | NativeStraightlineValue::DynamicJoinedText { .. }
        | NativeStraightlineValue::StringPtr(_)
        | NativeStraightlineValue::Text(_)
        | NativeStraightlineValue::DynamicSplitText { .. }
        | NativeStraightlineValue::DynamicTextChar
        | NativeStraightlineValue::Cell { .. }
        | NativeStraightlineValue::Error { .. }
        | NativeStraightlineValue::Bool(_)
        | NativeStraightlineValue::I64(_)
        | NativeStraightlineValue::F64(_)
        | NativeStraightlineValue::Builtin(_)
        | NativeStraightlineValue::Module(_)
        | NativeStraightlineValue::Function(_)
        | NativeStraightlineValue::ArgList { .. }
        | NativeStraightlineValue::Closure { .. } => None,
    }
}

fn native_i64_const_index(value: NativeStraightlineValue) -> Option<usize> {
    usize::try_from(native_i64_const_value(value)?).ok()
}

fn native_i64_const_index_for_len(value: NativeStraightlineValue, len: usize) -> Option<usize> {
    let index = native_i64_const_value(value)?;
    if index < 0 {
        usize::try_from((len as i64).checked_add(index)?).ok()
    } else {
        usize::try_from(index).ok()
    }
}

fn native_i64_const_value(value: NativeStraightlineValue) -> Option<i64> {
    let NativeStraightlineValue::I64(value) = value else {
        return None;
    };
    native_static_i64(&value)
}

fn native_static_range_bounds(elements: &[ConstRuntimeValue32Data], len: usize) -> Option<(usize, usize)> {
    if elements.is_empty() || elements.len() > 3 {
        return None;
    }
    let ConstRuntimeValue32Data::Int(start) = elements.first()? else {
        return None;
    };
    let last = elements.last().and_then(|value| match value {
        ConstRuntimeValue32Data::Int(value) => value.checked_add(1),
        _ => None,
    });
    let normalize = |index: i64| {
        if index < 0 {
            ((len as i64).checked_add(index)?).max(0)
        } else {
            index.min(len as i64)
        }
        .try_into()
        .ok()
    };
    let start: usize = normalize(*start)?;
    let end: usize = normalize(last.unwrap_or(len as i64))?.min(len);
    Some((start.min(end), end))
}

fn native_static_string_slice(value: &str, elements: &[ConstRuntimeValue32Data]) -> Option<String> {
    let (start, end) = native_static_range_bounds(elements, value.len())?;
    Some(value.get(start..end)?.to_string())
}

fn native_static_list_slice(
    values: &[ConstRuntimeValue32Data],
    elements: &[ConstRuntimeValue32Data],
) -> Option<Vec<ConstRuntimeValue32Data>> {
    let (start, end) = native_static_range_bounds(elements, values.len())?;
    Some(values.get(start..end)?.to_vec())
}

fn native_static_i64(value: &str) -> Option<i64> {
    if value.starts_with('%') {
        return None;
    }
    value.parse().ok()
}

fn native_static_f64(value: &str) -> Option<f64> {
    if value.starts_with('%') || value.starts_with("0x") {
        return None;
    }
    value.parse().ok()
}

fn native_map_key(value: NativeStraightlineValue) -> Option<RuntimeMapKeyData> {
    match value {
        NativeStraightlineValue::Nil => Some(RuntimeMapKeyData::Nil),
        NativeStraightlineValue::Bool(value) if value == "0" => Some(RuntimeMapKeyData::Bool(false)),
        NativeStraightlineValue::Bool(value) if value == "1" => Some(RuntimeMapKeyData::Bool(true)),
        NativeStraightlineValue::I64(value) if !value.starts_with('%') => {
            Some(RuntimeMapKeyData::Int(value.parse().ok()?))
        }
        NativeStraightlineValue::String {
            value,
            key_kind: NativeStringKeyKind::Short,
            ..
        } => Some(RuntimeMapKeyData::ShortStr(value)),
        NativeStraightlineValue::String {
            value,
            key_kind: NativeStringKeyKind::Heap,
            ..
        } => Some(RuntimeMapKeyData::String(value)),
        _ => None,
    }
}

fn native_const_string_value(value: &str) -> ConstRuntimeValue32Data {
    if ShortStr::new(value).is_some() {
        ConstRuntimeValue32Data::ShortStr(value.to_string())
    } else {
        ConstRuntimeValue32Data::Heap(Box::new(ConstHeapValue32Data::LongString(value.to_string())))
    }
}

pub(super) fn native_const_runtime_string(value: ConstRuntimeValue32Data) -> Option<String> {
    match value {
        ConstRuntimeValue32Data::ShortStr(value) => Some(value),
        ConstRuntimeValue32Data::Heap(value) => match *value {
            ConstHeapValue32Data::LongString(value) => Some(value),
            _ => None,
        },
        _ => None,
    }
}

fn native_string_key_value(value: NativeStraightlineValue) -> Option<String> {
    match value {
        NativeStraightlineValue::String { value, .. } => Some(value),
        NativeStraightlineValue::StringPtr(_) => None,
        _ => None,
    }
}

pub(super) fn native_runtime_string_key_kind(value: &str) -> NativeStringKeyKind {
    if ShortStr::new(value).is_some() {
        NativeStringKeyKind::Short
    } else {
        NativeStringKeyKind::Heap
    }
}

fn native_string_map_key_value(value: NativeStraightlineValue) -> Option<String> {
    match value {
        NativeStraightlineValue::Bool(value) if value == "0" => Some(false.to_string()),
        NativeStraightlineValue::Bool(value) if value == "1" => Some(true.to_string()),
        NativeStraightlineValue::I64(value) if !value.starts_with('%') => Some(value),
        NativeStraightlineValue::F64(value) if !value.starts_with('%') && !value.starts_with("0x") => {
            Some(value.parse::<f64>().ok()?.to_string())
        }
        NativeStraightlineValue::String { value, .. } => Some(value),
        _ => None,
    }
}

fn native_map_key_const_value(key: RuntimeMapKeyData) -> Option<ConstRuntimeValue32Data> {
    Some(match key {
        RuntimeMapKeyData::Nil => ConstRuntimeValue32Data::Nil,
        RuntimeMapKeyData::Bool(value) => ConstRuntimeValue32Data::Bool(value),
        RuntimeMapKeyData::Int(value) => ConstRuntimeValue32Data::Int(value),
        RuntimeMapKeyData::ShortStr(value) => ConstRuntimeValue32Data::ShortStr(value),
        RuntimeMapKeyData::String(value) => {
            ConstRuntimeValue32Data::Heap(Box::new(ConstHeapValue32Data::LongString(value)))
        }
        RuntimeMapKeyData::Obj(_) => return None,
    })
}
