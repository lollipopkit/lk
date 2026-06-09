//! Pure helper functions for scalar facts analysis.
//!
//! None of these functions depend on `native_scalar_block_facts_with_initial`.
//! Split out from `facts.rs` to keep each file under the 1500-line limit.

use crate::llvm::scalar::kind::{NativeScalarFacts, NativeScalarKind};
use crate::llvm::stdlib_catalog::stdlib_builtin_return_kind;
use crate::llvm::straightline_value::{
    NativeBuiltin, NativeListElementKind, NativeMapKeyKind, NativeMapValueKind, NativeStraightlineValue,
    NativeTextPart, native_static_index, native_straightline_heap_const_value,
};
use crate::vm::Instr;

/// Determine the return kind of a builtin function dynamically (unknown target).
pub(in crate::llvm) fn native_builtin_return_kind_dynamic(
    target: &NativeStraightlineValue,
    arg_count: u8,
) -> Option<NativeScalarKind> {
    if let NativeStraightlineValue::Builtin(builtin) = target
        && let Some(kind) = stdlib_builtin_return_kind(*builtin, usize::from(arg_count))
    {
        return Some(kind);
    }
    match target {
        NativeStraightlineValue::Builtin(NativeBuiltin::BitAnd | NativeBuiltin::BitNot | NativeBuiltin::BitOr) => {
            Some(NativeScalarKind::I64)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::CoreMakeStruct | NativeBuiltin::CoreMergeFields) => {
            Some(NativeScalarKind::I64)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::CoreSet) if arg_count == 1 => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::CoreTypeof) => Some(NativeScalarKind::StrPtr),
        NativeStraightlineValue::Builtin(NativeBuiltin::CoreCallMethod) => Some(NativeScalarKind::MaybeI64),
        NativeStraightlineValue::Builtin(NativeBuiltin::FibIterative | NativeBuiltin::MathlibDouble)
            if arg_count == 1 =>
        {
            Some(NativeScalarKind::I64)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::GreetingsMessage) if arg_count == 1 => {
            Some(NativeScalarKind::StrPtr)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::StringLen | NativeBuiltin::ListLen) => {
            Some(NativeScalarKind::I64)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::MapModuleMethod(method)) => {
            native_map_module_return_kind(method)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::ListIndexOf) => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::ListContains | NativeBuiltin::ListIsEmpty) => {
            Some(NativeScalarKind::Bool)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::MapSet | NativeBuiltin::MapMutate) => {
            Some(NativeScalarKind::I64)
        }
        NativeStraightlineValue::Builtin(_) => Some(NativeScalarKind::I64),
        _ => None,
    }
}

/// Determine the return kind of a known builtin with known args.
pub(in crate::llvm) fn native_builtin_return_kind(
    target: NativeStraightlineValue,
    args: &[NativeStraightlineValue],
) -> Option<NativeScalarKind> {
    if let NativeStraightlineValue::Builtin(
        builtin @ (NativeBuiltin::MathExp | NativeBuiltin::MathSin | NativeBuiltin::MathCos),
    ) = target
        && native_static_math_unary_i64(builtin, args).is_some()
    {
        return Some(NativeScalarKind::I64);
    }
    if let NativeStraightlineValue::Builtin(NativeBuiltin::CoreCallMethod) = target {
        return native_core_method_return_kind(args);
    }
    if let NativeStraightlineValue::Builtin(builtin) = target
        && let Some(kind) = stdlib_builtin_return_kind(builtin, args.len())
    {
        return Some(kind);
    }
    match target {
        NativeStraightlineValue::Builtin(NativeBuiltin::BitAnd) if args.len() == 2 => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::BitOr) if args.len() == 2 => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::BitNot) if args.len() == 1 => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::CoreMakeStruct) if args.len() == 2 => {
            Some(NativeScalarKind::I64)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::CoreSet) if args.len() == 1 => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::CoreMergeFields) if args.len() == 2 => {
            Some(NativeScalarKind::I64)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::CoreTypeof) if args.len() == 1 => {
            Some(NativeScalarKind::StrPtr)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::FibIterative | NativeBuiltin::MathlibDouble)
            if args.len() == 1 =>
        {
            Some(NativeScalarKind::I64)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::GreetingsMessage) if args.len() == 1 => {
            Some(NativeScalarKind::StrPtr)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::StringLen) if args.len() == 1 => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::MapModuleMethod(method)) => {
            native_map_module_return_kind(method)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::ListContains) if args.len() == 2 => {
            Some(NativeScalarKind::Bool)
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::ListIsEmpty) if args.len() == 1 => Some(NativeScalarKind::Bool),
        NativeStraightlineValue::Builtin(NativeBuiltin::ListLen) if args.len() == 1 => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::ListJoin) if args.len() == 2 => Some(NativeScalarKind::StrPtr),
        NativeStraightlineValue::Builtin(NativeBuiltin::ListFirst) if args.len() == 1 => native_static_index(
            args[0].clone(),
            NativeStraightlineValue::I64("0".to_string()),
            String::new(),
        )
        .and_then(|value| static_value_kind(&value))
        .or(Some(NativeScalarKind::I64)),
        NativeStraightlineValue::Builtin(NativeBuiltin::ListLast) if args.len() == 1 => {
            native_list_last_return_kind(&args[0]).or(Some(NativeScalarKind::I64))
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::ListGet) if args.len() == 2 => {
            native_static_index(args[0].clone(), args[1].clone(), String::new())
                .and_then(|value| static_value_kind(&value))
                .or(Some(NativeScalarKind::I64))
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::ListPop) if args.len() == 1 => {
            native_dynamic_list_element_kind(&args[0])
                .or_else(|| native_list_last_return_kind(&args[0]))
                .or(Some(NativeScalarKind::I64))
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::ListIndexOf) if args.len() == 2 => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(
            NativeBuiltin::ListConcat
            | NativeBuiltin::ListInsert
            | NativeBuiltin::ListPush
            | NativeBuiltin::ListRemoveAt
            | NativeBuiltin::ListReverse
            | NativeBuiltin::ListSet
            | NativeBuiltin::ListSlice
            | NativeBuiltin::ListSort,
        ) => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Builtin(NativeBuiltin::MapSet | NativeBuiltin::MapMutate) => {
            Some(NativeScalarKind::I64)
        }
        NativeStraightlineValue::Builtin(_) => Some(NativeScalarKind::I64),
        _ => None,
    }
}

fn native_static_math_unary_i64(builtin: NativeBuiltin, args: &[NativeStraightlineValue]) -> Option<i64> {
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
    let result = match builtin {
        NativeBuiltin::MathExp => value.exp(),
        NativeBuiltin::MathSin => value.sin(),
        NativeBuiltin::MathCos => value.cos(),
        _ => return None,
    };
    if result.fract() == 0.0 {
        return Some(result as i64);
    }
    None
}

fn native_map_module_return_kind(method: &str) -> Option<NativeScalarKind> {
    match method {
        "has" => Some(NativeScalarKind::Bool),
        "len" => Some(NativeScalarKind::I64),
        "get" => Some(NativeScalarKind::I64),
        "keys" | "values" => Some(NativeScalarKind::I64),
        _ => None,
    }
}

/// Determine the return kind for a CoreCallMethod builtin.
fn native_core_method_return_kind(args: &[NativeStraightlineValue]) -> Option<NativeScalarKind> {
    if let [
        receiver @ NativeStraightlineValue::Module(_),
        method @ NativeStraightlineValue::String { .. },
        method_args,
    ] = args
    {
        let builtin = native_static_index(receiver.clone(), method.clone(), String::new())?;
        let arg_count = core_method_arg_count(method_args)?;
        if arg_count == 0
            && let Some(kind) = native_builtin_return_kind(builtin.clone(), &[])
        {
            return Some(kind);
        }
        return native_builtin_return_kind_dynamic(&builtin, arg_count);
    }
    if let [
        NativeStraightlineValue::Set { .. },
        NativeStraightlineValue::String { value: method, .. },
        method_args,
    ] = args
    {
        return match (method.as_str(), core_method_arg_count(method_args)?) {
            ("has" | "contains", 1) => Some(NativeScalarKind::Bool),
            ("len", 0) => Some(NativeScalarKind::I64),
            ("add" | "delete", 1) => Some(NativeScalarKind::I64),
            _ => None,
        };
    }
    if let [
        NativeStraightlineValue::ArgList { .. },
        NativeStraightlineValue::String { value: method, .. },
        method_args,
    ] = args
    {
        return match (method.as_str(), method_args) {
            ("contains", NativeStraightlineValue::ArgList { elements }) if elements.len() == 1 => {
                Some(NativeScalarKind::Bool)
            }
            ("contains", NativeStraightlineValue::List { elements, .. }) if elements.len() == 1 => {
                Some(NativeScalarKind::Bool)
            }
            ("is_empty", NativeStraightlineValue::ArgList { elements }) if elements.is_empty() => {
                Some(NativeScalarKind::Bool)
            }
            ("is_empty", NativeStraightlineValue::List { elements, .. }) if elements.is_empty() => {
                Some(NativeScalarKind::Bool)
            }
            ("index_of", NativeStraightlineValue::ArgList { elements }) if elements.len() == 1 => {
                Some(NativeScalarKind::I64)
            }
            ("index_of", NativeStraightlineValue::List { elements, .. }) if elements.len() == 1 => {
                Some(NativeScalarKind::I64)
            }
            _ => Some(NativeScalarKind::I64),
        };
    }
    if let [
        NativeStraightlineValue::DynamicList {
            element: NativeListElementKind::F64,
            ..
        },
        NativeStraightlineValue::String { value: method, .. },
        _,
    ] = args
    {
        return match method.as_str() {
            "contains" => Some(NativeScalarKind::Bool),
            "index_of" => Some(NativeScalarKind::I64),
            "pop" => Some(NativeScalarKind::F64),
            _ => Some(NativeScalarKind::I64),
        };
    }
    if let [
        NativeStraightlineValue::DynamicList { .. } | NativeStraightlineValue::List { .. },
        NativeStraightlineValue::String { value: method, .. },
        method_args,
    ] = args
    {
        return match (method.as_str(), core_method_arg_count(method_args)?) {
            ("contains", 1) => Some(NativeScalarKind::Bool),
            ("is_empty", 0) => Some(NativeScalarKind::Bool),
            ("index_of", 1) => Some(NativeScalarKind::I64),
            _ => Some(NativeScalarKind::I64),
        };
    }
    if let [
        NativeStraightlineValue::Object { type_name, .. },
        NativeStraightlineValue::String { value: method, .. },
        NativeStraightlineValue::ArgList { elements },
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
            value,
            ..
        },
        NativeStraightlineValue::String { value: method, .. },
        NativeStraightlineValue::List { elements, .. },
    ] = args
        && method == "get"
        && elements.len() == 1
    {
        return match value {
            NativeMapValueKind::I64 => Some(NativeScalarKind::MaybeI64),
            NativeMapValueKind::F64 => Some(NativeScalarKind::F64),
            NativeMapValueKind::Bool => Some(NativeScalarKind::Bool),
            NativeMapValueKind::StrPtr => Some(NativeScalarKind::MaybeStrPtr),
        };
    }
    if let [
        NativeStraightlineValue::DynamicMap {
            key: NativeMapKeyKind::Str,
            value,
            ..
        },
        NativeStraightlineValue::String { value: method, .. },
        NativeStraightlineValue::ArgList { elements },
    ] = args
        && method == "get"
        && elements.len() == 1
    {
        return match value {
            NativeMapValueKind::I64 => Some(NativeScalarKind::MaybeI64),
            NativeMapValueKind::F64 => Some(NativeScalarKind::F64),
            NativeMapValueKind::Bool => Some(NativeScalarKind::Bool),
            NativeMapValueKind::StrPtr => Some(NativeScalarKind::MaybeStrPtr),
        };
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
        NativeStraightlineValue::DynamicList { .. } | NativeStraightlineValue::List { .. },
        NativeStraightlineValue::String { value: method, .. },
        NativeStraightlineValue::Function(_) | NativeStraightlineValue::Closure { .. },
    ] = args
        && (method == "filter" || method == "map")
    {
        return Some(NativeScalarKind::I64);
    }
    if let [
        NativeStraightlineValue::DynamicList { .. } | NativeStraightlineValue::List { .. },
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
        | NativeStraightlineValue::DynamicConstListElement { .. }
        | NativeStraightlineValue::DynamicArgListElement { .. },
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
            "lower" | "upper" | "trim" | "reverse" | "substring" | "replace" | "repeat" => {
                Some(NativeScalarKind::StrPtr)
            }
            _ => Some(NativeScalarKind::I64),
        };
    }
    Some(NativeScalarKind::I64)
}

fn core_method_arg_count(args: &NativeStraightlineValue) -> Option<u8> {
    let len = match args {
        NativeStraightlineValue::ArgList { elements } => elements.len(),
        NativeStraightlineValue::List { elements, .. } => elements.len(),
        NativeStraightlineValue::DynamicList { .. } => 1,
        _ => return None,
    };
    u8::try_from(len).ok()
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
            | "substring"
            | "replace"
            | "repeat"
            | "chars"
    )
}

/// Extract the return kind from scalar facts for a function's code.
pub(in crate::llvm) fn native_return_kind_from_facts(
    code: &[Instr],
    facts: &NativeScalarFacts,
) -> Option<NativeScalarKind> {
    let mut return_kind = None;
    for (pc, instr) in code.iter().copied().enumerate() {
        if !instr.opcode().is_return() {
            continue;
        }
        let kind = if instr.return_count() == 0 {
            NativeScalarKind::Nil
        } else if instr.return_count() == 1 {
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
        NativeScalarKind::StrPtr | NativeScalarKind::MaybeStrPtr => NativeTextPart::StrPtr("".to_string()),
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
    let Some((last, prefix_parts)) = parts.split_last() else {
        return false;
    };
    if prefix_parts.is_empty() {
        return false;
    }
    matches!(last, NativeTextPart::I64(_))
        && prefix_parts
            .iter()
            .all(|part| matches!(part, NativeTextPart::String { .. }))
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

pub(super) fn dynamic_scalar_placeholder(
    kinds: &[Option<NativeScalarKind>],
    reg: u8,
    allow_i64: bool,
) -> Option<NativeStraightlineValue> {
    match kinds.get(reg as usize).copied().flatten()? {
        NativeScalarKind::I64 if allow_i64 => Some(NativeStraightlineValue::I64("0".to_string())),
        NativeScalarKind::F64 => Some(NativeStraightlineValue::F64("0.0".to_string())),
        NativeScalarKind::Bool => Some(NativeStraightlineValue::Bool("0".to_string())),
        NativeScalarKind::StrPtr | NativeScalarKind::MaybeStrPtr => {
            Some(NativeStraightlineValue::StringPtr(String::new()))
        }
        _ => None,
    }
}

/// Check if a function has a CallDirect instruction targeting itself.
pub(in crate::llvm) fn function_has_self_recursive_call_direct(
    function: &crate::vm::FunctionData,
    _all_functions: &[crate::vm::FunctionData],
    func_idx: u16,
) -> bool {
    use crate::vm::Opcode;
    function.code.iter().any(|&raw| {
        let Ok(instr) = crate::vm::Instr::try_from_raw(raw) else {
            return false;
        };
        instr.opcode() == Opcode::CallDirect && instr.b() as u16 == func_idx
    })
}

/// Create initial global kinds from function count (all I64).
pub(in crate::llvm) fn global_kinds_from_fns(global_count: usize) -> Vec<Option<NativeScalarKind>> {
    (0..global_count).map(|_| Some(NativeScalarKind::I64)).collect()
}

/// Resolve a heap constant to its static straightline value.
pub(in crate::llvm) fn native_static_heap_const_value(
    value: &crate::vm::ConstHeapValueData,
) -> Option<(Option<NativeScalarKind>, NativeStraightlineValue)> {
    let value = native_straightline_heap_const_value(0, 0, value)?;
    let kind = static_value_kind(&value);
    Some((kind, value))
}

fn native_list_last_return_kind(value: &NativeStraightlineValue) -> Option<NativeScalarKind> {
    let NativeStraightlineValue::List { elements, .. } = value else {
        return None;
    };
    let index = elements.len().checked_sub(1)?;
    native_static_index(
        value.clone(),
        NativeStraightlineValue::I64(index.to_string()),
        String::new(),
    )
    .and_then(|value| static_value_kind(&value))
}

fn native_dynamic_list_element_kind(value: &NativeStraightlineValue) -> Option<NativeScalarKind> {
    let NativeStraightlineValue::DynamicList { element, .. } = value else {
        return None;
    };
    match element {
        NativeListElementKind::I64 => Some(NativeScalarKind::I64),
        NativeListElementKind::F64 => Some(NativeScalarKind::F64),
        NativeListElementKind::Bool => Some(NativeScalarKind::Bool),
        NativeListElementKind::StrPtr | NativeListElementKind::Text => Some(NativeScalarKind::StrPtr),
    }
}

/// Infer a scalar kind from a straightline value.
pub(in crate::llvm) fn static_value_kind(value: &NativeStraightlineValue) -> Option<NativeScalarKind> {
    match value {
        NativeStraightlineValue::I64(_) => Some(NativeScalarKind::I64),
        NativeStraightlineValue::MaybeI64 { .. } => Some(NativeScalarKind::MaybeI64),
        NativeStraightlineValue::MaybeF64 { .. } => Some(NativeScalarKind::F64),
        NativeStraightlineValue::MaybeBool { .. } => Some(NativeScalarKind::Bool),
        NativeStraightlineValue::MaybeStrPtr { .. } => Some(NativeScalarKind::MaybeStrPtr),
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
        NativeStraightlineValue::Set { .. } => Some(NativeScalarKind::I64),
        NativeStraightlineValue::DisplayMap { .. } => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Object { .. } => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Cell { .. } => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Error { .. } => Some(NativeScalarKind::Nil),
        NativeStraightlineValue::DynamicMap { .. }
        | NativeStraightlineValue::DynamicMapIter { .. }
        | NativeStraightlineValue::DynamicMapEntry { .. } => Some(NativeScalarKind::I64),
        NativeStraightlineValue::DynamicList { .. }
        | NativeStraightlineValue::DynamicPairList { .. }
        | NativeStraightlineValue::DynamicConstListElement { .. }
        | NativeStraightlineValue::DynamicArgListElement { .. } => Some(NativeScalarKind::I64),
        NativeStraightlineValue::Channel { .. } => Some(NativeScalarKind::I64),
        NativeStraightlineValue::DynamicJoinedText { .. } => Some(NativeScalarKind::StrPtr),
        NativeStraightlineValue::DynamicTextChar => Some(NativeScalarKind::StrPtr),
        NativeStraightlineValue::DynamicSplitText { .. } => Some(NativeScalarKind::I64),
    }
}
