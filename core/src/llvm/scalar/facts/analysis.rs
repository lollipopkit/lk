//! Pure helper functions for scalar facts analysis.
//!
//! None of these functions depend on `native_scalar_block_facts_with_initial`.
//! Split out from `facts.rs` to keep each file under the 1500-line limit.

use crate::llvm::scalar::kind::{NativeScalarFacts, NativeScalarKind};
use crate::llvm::straightline_value::{
    NativeBuiltin, NativeListElementKind, NativeMapKeyKind, NativeMapValueKind, NativeStraightlineValue,
    NativeTextPart, native_straightline_heap_const_value,
};
use crate::vm::Instr32;

/// Determine the return kind of a builtin function dynamically (unknown target).
pub(in crate::llvm) fn native_builtin_return_kind_dynamic(
    target: &NativeStraightlineValue,
    arg_count: u8,
) -> Option<NativeScalarKind> {
    match target {
        NativeStraightlineValue::Builtin(NativeBuiltin::Print | NativeBuiltin::Println) => Some(NativeScalarKind::Nil),
        NativeStraightlineValue::Builtin(NativeBuiltin::Panic) => Some(NativeScalarKind::Nil),
        NativeStraightlineValue::Builtin(NativeBuiltin::BitAnd | NativeBuiltin::BitNot | NativeBuiltin::BitOr) => {
            Some(NativeScalarKind::I64)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::CoreMakeStruct | NativeBuiltin::CoreMergeFields) => {
            Some(NativeScalarKind::I64)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::CoreTypeof) => Some(NativeScalarKind::StrPtr),
        NativeStraightlineValue::Builtin(NativeBuiltin::CoreCallMethod) => Some(NativeScalarKind::MaybeI64),
        NativeStraightlineValue::Builtin(NativeBuiltin::Chan) => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::Send) => Some(NativeScalarKind::Nil),
        NativeStraightlineValue::Builtin(NativeBuiltin::Recv) => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(
            NativeBuiltin::DatetimeAdd
            | NativeBuiltin::DatetimeDayOfWeek
            | NativeBuiltin::DatetimeDayOfYear
            | NativeBuiltin::DatetimeIsWeekend
            | NativeBuiltin::DatetimeNow
            | NativeBuiltin::DatetimeSub,
        ) => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::DatetimeFormat) => Some(NativeScalarKind::StrPtr),
        NativeStraightlineValue::Builtin(
            NativeBuiltin::IoStdoutWrite
            | NativeBuiltin::IoStdoutWriteln
            | NativeBuiltin::IoStderrWrite
            | NativeBuiltin::IoStdoutFlush,
        ) => Some(NativeScalarKind::Nil),
        NativeStraightlineValue::Builtin(NativeBuiltin::IoRead) => Some(NativeScalarKind::StrPtr),
        NativeStraightlineValue::Builtin(NativeBuiltin::IterRange)
            if arg_count == 1 || arg_count == 2 || arg_count == 3 =>
        {
            Some(NativeScalarKind::I64)
        }
        NativeStraightlineValue::Builtin(
            NativeBuiltin::MathSqrt
            | NativeBuiltin::MathPow
            | NativeBuiltin::MathExp
            | NativeBuiltin::MathSin
            | NativeBuiltin::MathCos,
        ) => Some(NativeScalarKind::F64),
        NativeStraightlineValue::Builtin(
            NativeBuiltin::MathAbs
            | NativeBuiltin::MathFloor
            | NativeBuiltin::MathCeil
            | NativeBuiltin::MathRound
            | NativeBuiltin::MathMin
            | NativeBuiltin::MathMax
            | NativeBuiltin::FibIterative
            | NativeBuiltin::MathlibDouble,
        ) => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::GreetingsMessage) => Some(NativeScalarKind::StrPtr),
        NativeStraightlineValue::Builtin(NativeBuiltin::TimeNow) if arg_count == 0 => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::TimeSleep) if arg_count == 1 => Some(NativeScalarKind::Nil),
        NativeStraightlineValue::Builtin(NativeBuiltin::TimeSince) if arg_count == 2 => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::TcpConnect | NativeBuiltin::TcpWrite) => {
            Some(NativeScalarKind::I64)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::TcpRead) => Some(NativeScalarKind::StrPtr),
        NativeStraightlineValue::Builtin(NativeBuiltin::TcpClose) => Some(NativeScalarKind::Bool),
        NativeStraightlineValue::Builtin(NativeBuiltin::StreamFromList | NativeBuiltin::StreamCollect) => {
            Some(NativeScalarKind::I64)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::StringLen) => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::MapSet | NativeBuiltin::MapMutate) => {
            Some(NativeScalarKind::I64)
        }
        NativeStraightlineValue::Builtin(
            NativeBuiltin::OsHostname
            | NativeBuiltin::OsArch
            | NativeBuiltin::OsName
            | NativeBuiltin::OsDirCurrent
            | NativeBuiltin::OsDirTemp,
        ) if arg_count == 0 => Some(NativeScalarKind::StrPtr),
        NativeStraightlineValue::Builtin(NativeBuiltin::OsDirList) if arg_count == 1 => Some(NativeScalarKind::StrPtr),
        NativeStraightlineValue::Builtin(_) => Some(NativeScalarKind::I64),
        _ => None,
    }
}

/// Determine the return kind of a known builtin with known args.
pub(in crate::llvm) fn native_builtin_return_kind(
    target: NativeStraightlineValue,
    args: &[NativeStraightlineValue],
) -> Option<NativeScalarKind> {
    match target {
        NativeStraightlineValue::Builtin(NativeBuiltin::OsClock) if args.is_empty() => Some(NativeScalarKind::F64),
        NativeStraightlineValue::Builtin(NativeBuiltin::OsEpoch) if args.is_empty() => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(
            NativeBuiltin::OsHostname
            | NativeBuiltin::OsArch
            | NativeBuiltin::OsName
            | NativeBuiltin::OsDirCurrent
            | NativeBuiltin::OsDirTemp,
        ) if args.is_empty() => Some(NativeScalarKind::StrPtr),
        NativeStraightlineValue::Builtin(NativeBuiltin::OsDirList) if args.len() == 1 => Some(NativeScalarKind::StrPtr),
        NativeStraightlineValue::Builtin(NativeBuiltin::IterRange)
            if args.len() == 1 || args.len() == 2 || args.len() == 3 =>
        {
            Some(NativeScalarKind::I64)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::MathExp | NativeBuiltin::MathSin | NativeBuiltin::MathCos)
            if native_static_math_unary_i64(args).is_some() =>
        {
            Some(NativeScalarKind::I64)
        }
        NativeStraightlineValue::Builtin(
            NativeBuiltin::MathSqrt
            | NativeBuiltin::MathPow
            | NativeBuiltin::MathExp
            | NativeBuiltin::MathSin
            | NativeBuiltin::MathCos,
        ) => Some(NativeScalarKind::F64),
        NativeStraightlineValue::Builtin(
            NativeBuiltin::MathAbs
            | NativeBuiltin::MathFloor
            | NativeBuiltin::MathCeil
            | NativeBuiltin::MathRound
            | NativeBuiltin::MathMin
            | NativeBuiltin::MathMax
            | NativeBuiltin::FibIterative
            | NativeBuiltin::MathlibDouble,
        ) => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::GreetingsMessage) if args.len() == 1 => {
            Some(NativeScalarKind::StrPtr)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::CoreCallMethod) => native_core_method_return_kind(args),
        NativeStraightlineValue::Builtin(NativeBuiltin::BitAnd) if args.len() == 2 => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::BitOr) if args.len() == 2 => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::BitNot) if args.len() == 1 => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::CoreMakeStruct) if args.len() == 2 => {
            Some(NativeScalarKind::I64)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::CoreMergeFields) if args.len() == 2 => {
            Some(NativeScalarKind::I64)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::CoreTypeof) if args.len() == 1 => {
            Some(NativeScalarKind::StrPtr)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::Chan) => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::Send) => Some(NativeScalarKind::Nil),
        NativeStraightlineValue::Builtin(NativeBuiltin::Recv) => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(
            NativeBuiltin::DatetimeAdd
            | NativeBuiltin::DatetimeDayOfWeek
            | NativeBuiltin::DatetimeDayOfYear
            | NativeBuiltin::DatetimeIsWeekend
            | NativeBuiltin::DatetimeNow
            | NativeBuiltin::DatetimeSub,
        ) => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::DatetimeFormat) => Some(NativeScalarKind::StrPtr),
        NativeStraightlineValue::Builtin(
            NativeBuiltin::IoStdoutWrite
            | NativeBuiltin::IoStdoutWriteln
            | NativeBuiltin::IoStderrWrite
            | NativeBuiltin::IoStdoutFlush,
        ) => Some(NativeScalarKind::Nil),
        NativeStraightlineValue::Builtin(NativeBuiltin::IoRead) => Some(NativeScalarKind::StrPtr),
        NativeStraightlineValue::Builtin(
            NativeBuiltin::JsonParse | NativeBuiltin::TomlParse | NativeBuiltin::YamlParse,
        ) if args.len() == 1 => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::TimeNow) if args.is_empty() => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::TimeSleep) if args.len() == 1 => Some(NativeScalarKind::Nil),
        NativeStraightlineValue::Builtin(NativeBuiltin::TimeSince) if args.len() == 2 => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::TcpConnect | NativeBuiltin::TcpWrite) => {
            Some(NativeScalarKind::I64)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::TcpRead) => Some(NativeScalarKind::StrPtr),
        NativeStraightlineValue::Builtin(NativeBuiltin::TcpClose) => Some(NativeScalarKind::Bool),
        NativeStraightlineValue::Builtin(NativeBuiltin::StreamFromList | NativeBuiltin::StreamCollect) => {
            Some(NativeScalarKind::I64)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::StringLen) if args.len() == 1 => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::MapSet | NativeBuiltin::MapMutate) => {
            Some(NativeScalarKind::I64)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::Print | NativeBuiltin::Println) => Some(NativeScalarKind::Nil),
        NativeStraightlineValue::Builtin(NativeBuiltin::Panic) if args.len() <= 1 => Some(NativeScalarKind::Nil),
        NativeStraightlineValue::Builtin(_) => Some(NativeScalarKind::I64),
        _ => None,
    }
}

fn native_static_math_unary_i64(args: &[NativeStraightlineValue]) -> Option<i64> {
    let [arg] = args else {
        return None;
    };
    let value = match arg {
        NativeStraightlineValue::I64(value) if !value.starts_with('%') => value.parse::<i64>().ok()? as f64,
        NativeStraightlineValue::F64(value) if !value.starts_with('%') && !value.starts_with("0x") => {
            value.parse().ok()?
        }
        _ => return None,
    };
    for result in [value.exp(), value.sin(), value.cos()] {
        if result.fract() == 0.0 {
            return Some(result as i64);
        }
    }
    None
}

/// Determine the return kind for a CoreCallMethod builtin.
fn native_core_method_return_kind(args: &[NativeStraightlineValue]) -> Option<NativeScalarKind> {
    if let [
        NativeStraightlineValue::Object { type_name, .. },
        NativeStraightlineValue::String { value: method, .. },
        NativeStraightlineValue::List { elements, .. },
    ] = args
        && elements.is_empty()
    {
        return match (type_name.as_str(), method.as_str()) {
            (_, "describe" | "show") => Some(NativeScalarKind::StrPtr),
            _ => Some(NativeScalarKind::I64),
        };
    }
    if let [
        NativeStraightlineValue::Object { type_name, .. },
        NativeStraightlineValue::String { value: method, .. },
        NativeStraightlineValue::DynamicList { .. },
    ] = args
    {
        return match (type_name.as_str(), method.as_str()) {
            (_, "describe" | "show") => Some(NativeScalarKind::StrPtr),
            _ => Some(NativeScalarKind::I64),
        };
    }
    if let [
        NativeStraightlineValue::DynamicMap {
            key: NativeMapKeyKind::Str,
            value: NativeMapValueKind::I64,
            ..
        },
        NativeStraightlineValue::String { value: method, .. },
        NativeStraightlineValue::List { elements, .. },
    ] = args
        && method == "get"
        && elements.len() == 1
    {
        return Some(NativeScalarKind::MaybeI64);
    }
    if let [
        NativeStraightlineValue::DynamicMap {
            key: NativeMapKeyKind::Str,
            value: NativeMapValueKind::I64,
            ..
        },
        NativeStraightlineValue::String { value: method, .. },
        NativeStraightlineValue::ArgList { elements },
    ] = args
        && method == "get"
        && elements.len() == 1
    {
        return Some(NativeScalarKind::MaybeI64);
    }
    if let [
        NativeStraightlineValue::DynamicList {
            element: NativeListElementKind::I64,
            ..
        }
        | NativeStraightlineValue::List { .. },
        NativeStraightlineValue::String { value: method, .. },
        NativeStraightlineValue::List { elements, .. },
    ] = args
        && method == "get"
        && elements.len() == 1
    {
        return Some(NativeScalarKind::MaybeI64);
    }
    if let [
        NativeStraightlineValue::DynamicList {
            element: NativeListElementKind::I64,
            ..
        }
        | NativeStraightlineValue::List { .. },
        NativeStraightlineValue::String { value: method, .. },
        NativeStraightlineValue::DynamicList {
            element: NativeListElementKind::I64,
            ..
        },
    ] = args
        && method == "get"
    {
        return Some(NativeScalarKind::MaybeI64);
    }
    if let [
        NativeStraightlineValue::DynamicList {
            element: NativeListElementKind::I64,
            ..
        }
        | NativeStraightlineValue::List { .. },
        NativeStraightlineValue::String { value: method, .. },
        NativeStraightlineValue::Function(_) | NativeStraightlineValue::Closure { .. },
    ] = args
        && (method == "filter" || method == "map")
    {
        return Some(NativeScalarKind::I64);
    }
    if let [
        NativeStraightlineValue::DynamicList {
            element: NativeListElementKind::I64,
            ..
        }
        | NativeStraightlineValue::List { .. },
        NativeStraightlineValue::String { value: method, .. },
        NativeStraightlineValue::ArgList { .. },
    ] = args
        && (method == "filter" || method == "map" || method == "reduce")
    {
        return Some(NativeScalarKind::I64);
    }
    if let [
        NativeStraightlineValue::String { value: first, .. },
        NativeStraightlineValue::String { value: second, .. },
        NativeStraightlineValue::List { .. }
        | NativeStraightlineValue::DynamicList { .. }
        | NativeStraightlineValue::DynamicConstListElement { .. },
    ] = args
    {
        let method = if native_static_string_method_known(first) {
            first
        } else {
            second
        };
        return match method.as_str() {
            "is_empty" | "contains" | "ends_with" => Some(NativeScalarKind::Bool),
            "find" => Some(NativeScalarKind::I64),
            "chars" => Some(NativeScalarKind::I64),
            "lower" | "upper" | "trim" | "reverse" | "replace" | "repeat" => Some(NativeScalarKind::StrPtr),
            _ => Some(NativeScalarKind::I64),
        };
    }
    let [
        NativeStraightlineValue::Module(crate::llvm::straightline_value::NativeModule::OsEnv),
        NativeStraightlineValue::String { value: method, .. },
        NativeStraightlineValue::List { elements, .. },
    ] = args
    else {
        return Some(NativeScalarKind::I64);
    };
    if method == "get" && (elements.len() == 1 || elements.len() == 2) {
        Some(NativeScalarKind::StrPtr)
    } else {
        Some(NativeScalarKind::I64)
    }
}

fn native_static_string_method_known(method: &str) -> bool {
    matches!(
        method,
        "is_empty"
            | "lower"
            | "upper"
            | "trim"
            | "reverse"
            | "contains"
            | "ends_with"
            | "find"
            | "replace"
            | "repeat"
            | "chars"
    )
}

/// Extract the return kind from scalar facts for a function's code.
pub(in crate::llvm) fn native_return_kind_from_facts(
    code: &[Instr32],
    facts: &NativeScalarFacts,
) -> Option<NativeScalarKind> {
    use crate::vm::Opcode32;
    let mut return_kind = None;
    for (pc, instr) in code.iter().copied().enumerate() {
        if instr.opcode() != Opcode32::Return {
            continue;
        }
        let kind = if instr.b() == 0 {
            NativeScalarKind::Nil
        } else if instr.b() == 1 {
            facts.register_kind_before(pc, instr.a())?
        } else {
            return None;
        };
        if return_kind.replace(kind).is_some_and(|previous| previous != kind) {
            return None;
        }
    }
    return_kind
}

/// Create a dynamic text part from a register's kind (for template strings).
pub(in crate::llvm) fn dynamic_text_part(kind: NativeScalarKind, _reg: u8) -> Option<NativeStraightlineValue> {
    let part = match kind {
        NativeScalarKind::I64 | NativeScalarKind::MaybeI64 => NativeTextPart::I64("0".to_string()),
        NativeScalarKind::F64 => NativeTextPart::F64("0.0".to_string()),
        NativeScalarKind::Bool => NativeTextPart::Bool("false".to_string()),
        NativeScalarKind::Nil => NativeTextPart::Nil,
        NativeScalarKind::StrPtr => NativeTextPart::StrPtr("".to_string()),
    };
    Some(NativeStraightlineValue::Text(vec![part]))
}

/// Check whether a key value is a supported string/int map key for static analysis.
pub(in crate::llvm) fn native_string_int_map_key_supported(value: &NativeStraightlineValue) -> bool {
    if matches!(value, NativeStraightlineValue::StringPtr(_)) {
        return true;
    }
    let NativeStraightlineValue::Text(parts) = value else {
        return matches!(value, NativeStraightlineValue::String { value, .. } if value.is_ascii());
    };
    let Some((last, prefix)) = parts.split_last() else {
        return false;
    };
    if !prefix.is_empty() {
        return false;
    }
    matches!(
        last,
        NativeTextPart::I64(_) | NativeTextPart::String { .. } | NativeTextPart::StrPtr(_)
    )
}

/// Check whether a value supports `Len` operation in static analysis.
pub(in crate::llvm) fn native_dynamic_text_len_supported(value: &NativeStraightlineValue) -> bool {
    match value {
        NativeStraightlineValue::Text(parts) => parts.iter().all(|p| {
            matches!(
                p,
                NativeTextPart::I64(_) | NativeTextPart::String { .. } | NativeTextPart::StrPtr(_)
            )
        }),
        NativeStraightlineValue::String { .. } | NativeStraightlineValue::StringPtr(_) => true,
        _ => false,
    }
}

/// Check if a function has a CallDirect instruction targeting itself.
pub(in crate::llvm) fn function_has_self_recursive_call_direct(
    function: &crate::vm::Function32Data,
    _all_functions: &[crate::vm::Function32Data],
    func_idx: u16,
) -> bool {
    use crate::vm::Opcode32;
    function.code.iter().any(|&raw| {
        let Ok(instr) = crate::vm::Instr32::try_from_raw(raw) else {
            return false;
        };
        instr.opcode() == Opcode32::CallDirect && instr.b() as u16 == func_idx
    })
}

/// Create initial global kinds from function count (all I64).
pub(in crate::llvm) fn global_kinds_from_fns(global_count: usize) -> Vec<Option<NativeScalarKind>> {
    (0..global_count).map(|_| Some(NativeScalarKind::I64)).collect()
}

/// Resolve a heap constant to its static straightline value.
pub(in crate::llvm) fn native_static_heap_const_value(
    value: &crate::vm::ConstHeapValue32Data,
) -> Option<(Option<NativeScalarKind>, NativeStraightlineValue)> {
    let value = native_straightline_heap_const_value(0, 0, value)?;
    let kind = static_value_kind(&value);
    Some((kind, value))
}

/// Infer a scalar kind from a straightline value.
pub(in crate::llvm) fn static_value_kind(value: &NativeStraightlineValue) -> Option<NativeScalarKind> {
    match value {
        NativeStraightlineValue::I64(_) => Some(NativeScalarKind::I64),
        NativeStraightlineValue::F64(_) => Some(NativeScalarKind::F64),
        NativeStraightlineValue::Bool(_) => Some(NativeScalarKind::Bool),
        NativeStraightlineValue::Nil => Some(NativeScalarKind::Nil),
        NativeStraightlineValue::StringPtr(_) | NativeStraightlineValue::String { .. } => {
            Some(NativeScalarKind::StrPtr)
        }
        NativeStraightlineValue::Builtin(_) => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Text(_) => Some(NativeScalarKind::StrPtr),
        NativeStraightlineValue::Function(_) => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Closure { .. } => Some(NativeScalarKind::I64),
        NativeStraightlineValue::ArgList { .. } => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Module(_) => Some(NativeScalarKind::I64),
        NativeStraightlineValue::List { .. } => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Map { .. } => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Object { .. } => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Cell { .. } => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Error { .. } => Some(NativeScalarKind::Nil),
        NativeStraightlineValue::DynamicMap {
            key: NativeMapKeyKind::Str,
            value: NativeMapValueKind::I64,
            ..
        } => Some(NativeScalarKind::I64),
        NativeStraightlineValue::DynamicList { .. } | NativeStraightlineValue::DynamicConstListElement { .. } => {
            Some(NativeScalarKind::I64)
        }
        NativeStraightlineValue::Channel { .. } => Some(NativeScalarKind::I64),
        NativeStraightlineValue::DynamicJoinedText { .. } => Some(NativeScalarKind::StrPtr),
        NativeStraightlineValue::DynamicTextChar => Some(NativeScalarKind::StrPtr),
        NativeStraightlineValue::DynamicSplitText { .. } => Some(NativeScalarKind::I64),
    }
}
