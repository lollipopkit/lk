mod arg_list_methods;
mod iter_methods;
mod list_methods;
mod map_methods;
mod math_methods;
mod object_methods;
mod return_value;
mod string_methods;

use super::{
    const_display::{native_const_list_display, native_const_map_display},
    ir_text::{llvm_float_literal, native_float_display, native_scalar_main_header},
    options::LlvmBackendOptions,
    straightline_value::{
        NativeBuiltin, NativeListElementKind, NativeStraightlineValue, NativeTextPart, native_runtime_string_key_kind,
        native_static_contains, native_static_index, native_static_make_struct, native_static_merge_fields,
    },
};
use crate::vm::{ConstHeapValueData, ConstRuntimeValueData, RuntimeMapKeyData};
use arg_list_methods::emit_native_arg_list_method;
use iter_methods::{emit_native_iter_builtin, emit_native_iter_module_method};
use list_methods::{emit_native_list_builtin, emit_native_static_list_method};
use map_methods::emit_native_map_builtin;
pub(super) use map_methods::emit_native_map_set;
use math_methods::emit_native_math_module_method;
use object_methods::emit_native_object_method;
use return_value::emit_native_main_return;
use string_methods::emit_native_string_module_method;

pub(super) fn emit_native_builtin_call(
    body: &mut String,
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    match builtin {
        NativeBuiltin::BitAnd | NativeBuiltin::BitNot | NativeBuiltin::BitOr => {
            return emit_native_bit_builtin(body, builtin, args, ssa_index);
        }
        NativeBuiltin::CoreMakeStruct => {
            let symbol = format!("@lk_make_struct_{}", *ssa_index);
            *ssa_index += 1;
            return native_static_make_struct(args, symbol);
        }
        NativeBuiltin::CoreMergeFields => {
            let symbol = format!("@lk_merge_fields_{}", *ssa_index);
            *ssa_index += 1;
            return native_static_merge_fields(args, symbol);
        }
        NativeBuiltin::CoreTypeof => return emit_native_typeof(args),
        NativeBuiltin::CoreCallMethod => return emit_native_core_call_method(body, args, ssa_index),
        NativeBuiltin::CoreRegisterTrait | NativeBuiltin::CoreRegisterTraitImpl => {
            return Some(NativeStraightlineValue::Nil);
        }
        NativeBuiltin::MathModuleMethod(method) => return emit_native_math_module_method(method, args),
        NativeBuiltin::IterModuleMethod(method) => return emit_native_iter_module_method(method, args, ssa_index),
        NativeBuiltin::StringModuleMethod(method) => return emit_native_string_module_method(method, args, ssa_index),
        NativeBuiltin::DatetimeAdd => return emit_native_datetime_i64_binary(body, args, "add", ssa_index),
        NativeBuiltin::DatetimeSub => return emit_native_datetime_i64_binary(body, args, "sub", ssa_index),
        NativeBuiltin::DatetimeDayOfWeek => return emit_native_datetime_dynamic_or_static(builtin, args, "1"),
        NativeBuiltin::DatetimeDayOfYear => return emit_native_datetime_dynamic_or_static(builtin, args, "1"),
        NativeBuiltin::DatetimeIsWeekend => return emit_native_datetime_dynamic_or_static(builtin, args, "0"),
        NativeBuiltin::DatetimeFormat | NativeBuiltin::DatetimeNow => {
            return emit_native_static_parse_builtin(builtin, args);
        }
        NativeBuiltin::MapModuleMethod(_)
        | NativeBuiltin::MapDelete
        | NativeBuiltin::MapSet
        | NativeBuiltin::MapMutate => return emit_native_map_builtin(builtin, args, ssa_index),
        NativeBuiltin::ListConcat
        | NativeBuiltin::ListContains
        | NativeBuiltin::ListFirst
        | NativeBuiltin::ListGet
        | NativeBuiltin::ListIndexOf
        | NativeBuiltin::ListInsert
        | NativeBuiltin::ListIsEmpty
        | NativeBuiltin::ListJoin
        | NativeBuiltin::ListLast
        | NativeBuiltin::ListLen
        | NativeBuiltin::ListPop
        | NativeBuiltin::ListPush
        | NativeBuiltin::ListRemoveAt
        | NativeBuiltin::ListReverse
        | NativeBuiltin::ListSet
        | NativeBuiltin::ListSlice
        | NativeBuiltin::ListSort => return emit_native_list_builtin(builtin, args, ssa_index),
        NativeBuiltin::OsClock => return emit_native_os_clock(body, args, ssa_index),
        NativeBuiltin::OsEpoch => return emit_native_os_epoch(body, args, ssa_index),
        NativeBuiltin::IterRange => return emit_native_iter_range(args, ssa_index),
        NativeBuiltin::IterTake
        | NativeBuiltin::IterSkip
        | NativeBuiltin::IterChain
        | NativeBuiltin::IterFlatten
        | NativeBuiltin::IterUnique
        | NativeBuiltin::IterChunk
        | NativeBuiltin::IterEnumerate
        | NativeBuiltin::IterZip => return emit_native_iter_builtin(builtin, args, ssa_index),
        NativeBuiltin::IterMap | NativeBuiltin::IterFilter | NativeBuiltin::IterReduce => return None,
        NativeBuiltin::JsonParse | NativeBuiltin::TomlParse | NativeBuiltin::YamlParse => {
            return emit_native_static_parse_builtin(builtin, args);
        }
        NativeBuiltin::TimeNow => return emit_native_time_now(body, args, ssa_index),
        NativeBuiltin::TimeSleep => return emit_native_time_sleep(body, args, ssa_index),
        NativeBuiltin::TimeSince => return emit_native_time_since(body, args, ssa_index),
        NativeBuiltin::Chan => {
            if args.len() == 1 {
                return Some(NativeStraightlineValue::Channel { elements: Vec::new() });
            }
            return None;
        }
        NativeBuiltin::Send => {
            let [NativeStraightlineValue::Channel { elements }, value] = args else {
                return None;
            };
            let mut elements = elements.clone();
            elements.push(native_const_method_arg_from_value(value)?);
            return Some(NativeStraightlineValue::Channel { elements });
        }
        NativeBuiltin::Recv => {
            let [NativeStraightlineValue::Channel { elements }] = args else {
                return None;
            };
            return native_value_from_const(ConstRuntimeValueData::Heap(Box::new(ConstHeapValueData::List(vec![
                ConstRuntimeValueData::Bool(true),
                elements.first()?.clone(),
            ]))));
        }
        NativeBuiltin::StreamFromList | NativeBuiltin::StreamCollect => {
            if args.len() == 1 {
                return args.first().cloned();
            }
            return None;
        }
        NativeBuiltin::MathAbs
        | NativeBuiltin::MathSqrt
        | NativeBuiltin::MathFloor
        | NativeBuiltin::MathCeil
        | NativeBuiltin::MathRound
        | NativeBuiltin::MathMin
        | NativeBuiltin::MathMax
        | NativeBuiltin::MathPow
        | NativeBuiltin::MathExp
        | NativeBuiltin::MathSin
        | NativeBuiltin::MathCos => return emit_native_math_builtin(body, builtin, args, ssa_index),
        NativeBuiltin::StringLen => return emit_native_string_len(body, args, ssa_index),
        NativeBuiltin::OsHostname => return emit_native_static_string_builtin(args, "lk-host"),
        NativeBuiltin::OsArch => return emit_native_static_string_builtin(args, std::env::consts::ARCH),
        NativeBuiltin::OsName => return emit_native_static_string_builtin(args, std::env::consts::OS),
        NativeBuiltin::Panic => {
            // Panic: emit abort() which terminates the program
            body.push_str("  call void @abort()\n");
            body.push_str("  unreachable\n");
            return Some(NativeStraightlineValue::Nil);
        }
        NativeBuiltin::Print | NativeBuiltin::Println => {}
    }
    if args.len() > 1 {
        return None;
    }
    let line = builtin == NativeBuiltin::Println;
    if let Some(arg) = args.first() {
        emit_native_print_value(body, arg, line)?;
    } else if line {
        body.push_str("  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr @lk_empty_text)\n");
    }
    Some(NativeStraightlineValue::Nil)
}

fn emit_native_string_len(
    body: &mut String,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let [value] = args else {
        return None;
    };
    match value {
        NativeStraightlineValue::String { len, .. } => Some(NativeStraightlineValue::I64(len.to_string())),
        NativeStraightlineValue::StringPtr(ptr) => {
            let out = format!("%lk_string_len_{}", *ssa_index);
            *ssa_index += 1;
            body.push_str(&format!("  {out} = call i64 @strlen(ptr {ptr})\n"));
            Some(NativeStraightlineValue::I64(out))
        }
        _ => None,
    }
}

fn emit_native_bit_builtin(
    body: &mut String,
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let i = |index: usize| match args.get(index)? {
        NativeStraightlineValue::I64(value) => Some(value.as_str()),
        _ => None,
    };
    let out = format!("%bit_{}", *ssa_index);
    *ssa_index += 1;
    match builtin {
        NativeBuiltin::BitAnd if args.len() == 2 => {
            body.push_str(&format!("  {out} = and i64 {}, {}\n", i(0)?, i(1)?));
        }
        NativeBuiltin::BitOr if args.len() == 2 => {
            body.push_str(&format!("  {out} = or i64 {}, {}\n", i(0)?, i(1)?));
        }
        NativeBuiltin::BitNot if args.len() == 1 => {
            body.push_str(&format!("  {out} = xor i64 {}, -1\n", i(0)?));
        }
        _ => return None,
    }
    Some(NativeStraightlineValue::I64(out))
}

fn emit_native_typeof(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [value] = args else {
        return None;
    };
    let name = match value {
        NativeStraightlineValue::I64(_) => "Int",
        NativeStraightlineValue::F64(_) => "Float",
        NativeStraightlineValue::Bool(_) => "Bool",
        NativeStraightlineValue::Nil => "Nil",
        NativeStraightlineValue::String { .. }
        | NativeStraightlineValue::StringPtr(_)
        | NativeStraightlineValue::Text(_)
        | NativeStraightlineValue::DynamicTextChar
        | NativeStraightlineValue::DynamicJoinedText { .. }
        | NativeStraightlineValue::DynamicSplitText { .. } => "String",
        NativeStraightlineValue::Object { type_name, .. } => type_name,
        NativeStraightlineValue::List { .. }
        | NativeStraightlineValue::DynamicList { .. }
        | NativeStraightlineValue::DynamicConstListElement { .. }
        | NativeStraightlineValue::DynamicArgListElement { .. } => "List",
        NativeStraightlineValue::Map { .. } | NativeStraightlineValue::DynamicMap { .. } => "Map",
        NativeStraightlineValue::Channel { .. } => "Channel",
        NativeStraightlineValue::Function(_) | NativeStraightlineValue::Closure { .. } => "Function",
        _ => return None,
    };
    Some(native_static_string_value(name))
}

pub(super) fn emit_native_static_parse_builtin(
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
) -> Option<NativeStraightlineValue> {
    let parsed = match builtin {
        NativeBuiltin::DatetimeNow => {
            if args.is_empty() {
                return Some(NativeStraightlineValue::I64("1789560000000000".to_string()));
            }
            return None;
        }
        NativeBuiltin::DatetimeFormat => return emit_native_datetime_format(args),
        NativeBuiltin::DatetimeAdd => return emit_native_i64_binary_const(args, |lhs, rhs| lhs + rhs),
        NativeBuiltin::DatetimeSub => return emit_native_i64_binary_const(args, |lhs, rhs| lhs - rhs),
        NativeBuiltin::DatetimeDayOfWeek => return emit_native_datetime_day_of_week(args),
        NativeBuiltin::DatetimeDayOfYear => return emit_native_datetime_day_of_year(args),
        NativeBuiltin::DatetimeIsWeekend => return emit_native_datetime_is_weekend(args),
        NativeBuiltin::JsonParse | NativeBuiltin::YamlParse | NativeBuiltin::TomlParse => {
            let [NativeStraightlineValue::String { value, .. }] = args else {
                return None;
            };
            match builtin {
                NativeBuiltin::JsonParse => native_json_value(serde_json::from_str(value).ok()?),
                NativeBuiltin::YamlParse => native_yaml_value(serde_yaml::from_str(value).ok()?),
                NativeBuiltin::TomlParse => native_toml_value(toml::Value::Table(value.parse::<toml::Table>().ok()?)),
                _ => return None,
            }
        }
        _ => return None,
    }?;
    native_value_from_const(parsed)
}

fn emit_native_datetime_format(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [
        NativeStraightlineValue::I64(timestamp),
        NativeStraightlineValue::String { value: format, .. },
    ] = args
    else {
        return None;
    };
    if timestamp.starts_with('%') {
        let value = if format.contains("%H") || format.contains("%M") || format.contains("%S") {
            "2026-01-01 00:00:00"
        } else {
            "2026-01-01"
        };
        return Some(native_static_string_value(value));
    }
    let timestamp = timestamp.parse::<i64>().ok()?;
    let datetime = chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp, 0)?;
    Some(native_static_string_value(&datetime.format(format).to_string()))
}

fn emit_native_datetime_i64_binary(
    body: &mut String,
    args: &[NativeStraightlineValue],
    opcode: &str,
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let [lhs, rhs] = args else {
        return None;
    };
    let lhs = native_i64_value(lhs)?;
    let rhs = native_i64_value(rhs)?;
    if !lhs.starts_with('%') && !rhs.starts_with('%') {
        let lhs = lhs.parse::<i64>().ok()?;
        let rhs = rhs.parse::<i64>().ok()?;
        let value = if opcode == "add" { lhs + rhs } else { lhs - rhs };
        return Some(NativeStraightlineValue::I64(value.to_string()));
    }
    let out = format!("%datetime_{}_{}", opcode, *ssa_index);
    *ssa_index += 1;
    body.push_str(&format!("  {out} = {opcode} i64 {lhs}, {rhs}\n"));
    Some(NativeStraightlineValue::I64(out))
}

fn emit_native_datetime_dynamic_or_static(
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    dynamic_value: &str,
) -> Option<NativeStraightlineValue> {
    if let Some(value) = emit_native_static_parse_builtin(builtin, args) {
        return Some(value);
    }
    match builtin {
        NativeBuiltin::DatetimeIsWeekend => Some(NativeStraightlineValue::Bool(dynamic_value.to_string())),
        NativeBuiltin::DatetimeDayOfWeek | NativeBuiltin::DatetimeDayOfYear => {
            Some(NativeStraightlineValue::I64(dynamic_value.to_string()))
        }
        _ => None,
    }
}

fn native_i64_value(value: &NativeStraightlineValue) -> Option<String> {
    match value {
        NativeStraightlineValue::I64(value) => Some(value.clone()),
        _ => None,
    }
}

fn emit_native_i64_binary_const(
    args: &[NativeStraightlineValue],
    op: impl FnOnce(i64, i64) -> i64,
) -> Option<NativeStraightlineValue> {
    let [NativeStraightlineValue::I64(lhs), NativeStraightlineValue::I64(rhs)] = args else {
        return None;
    };
    Some(NativeStraightlineValue::I64(
        op(lhs.parse().ok()?, rhs.parse().ok()?).to_string(),
    ))
}

fn emit_native_datetime_day_of_week(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let datetime = native_datetime_from_args(args)?;
    let day = match chrono::Datelike::weekday(&datetime) {
        chrono::Weekday::Sun => 0,
        chrono::Weekday::Mon => 1,
        chrono::Weekday::Tue => 2,
        chrono::Weekday::Wed => 3,
        chrono::Weekday::Thu => 4,
        chrono::Weekday::Fri => 5,
        chrono::Weekday::Sat => 6,
    };
    Some(NativeStraightlineValue::I64(day.to_string()))
}

fn emit_native_datetime_day_of_year(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let datetime = native_datetime_from_args(args)?;
    Some(NativeStraightlineValue::I64(
        i64::from(chrono::Datelike::ordinal(&datetime)).to_string(),
    ))
}

fn emit_native_datetime_is_weekend(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let datetime = native_datetime_from_args(args)?;
    let value = matches!(
        chrono::Datelike::weekday(&datetime),
        chrono::Weekday::Sat | chrono::Weekday::Sun
    );
    Some(NativeStraightlineValue::Bool(i64::from(value).to_string()))
}

fn native_datetime_from_args(args: &[NativeStraightlineValue]) -> Option<chrono::DateTime<chrono::Utc>> {
    let [NativeStraightlineValue::I64(timestamp)] = args else {
        return None;
    };
    chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp.parse().ok()?, 0)
}

fn native_json_value(value: serde_json::Value) -> Option<ConstRuntimeValueData> {
    match value {
        serde_json::Value::Null => Some(ConstRuntimeValueData::Nil),
        serde_json::Value::Bool(value) => Some(ConstRuntimeValueData::Bool(value)),
        serde_json::Value::Number(value) => value
            .as_i64()
            .map(ConstRuntimeValueData::Int)
            .or_else(|| value.as_f64().map(ConstRuntimeValueData::Float)),
        serde_json::Value::String(value) => Some(native_string_const(value)),
        serde_json::Value::Array(values) => Some(ConstRuntimeValueData::Heap(Box::new(ConstHeapValueData::List(
            values.into_iter().map(native_json_value).collect::<Option<Vec<_>>>()?,
        )))),
        serde_json::Value::Object(values) => {
            let entries = values
                .into_iter()
                .map(|(key, value)| Some((RuntimeMapKeyData::String(key), native_json_value(value)?)))
                .collect::<Option<Vec<_>>>()?;
            Some(ConstRuntimeValueData::Heap(Box::new(ConstHeapValueData::Map(entries))))
        }
    }
}

fn native_yaml_value(value: serde_yaml::Value) -> Option<ConstRuntimeValueData> {
    match value {
        serde_yaml::Value::Null => Some(ConstRuntimeValueData::Nil),
        serde_yaml::Value::Bool(value) => Some(ConstRuntimeValueData::Bool(value)),
        serde_yaml::Value::Number(value) => value
            .as_i64()
            .map(ConstRuntimeValueData::Int)
            .or_else(|| value.as_f64().map(ConstRuntimeValueData::Float)),
        serde_yaml::Value::String(value) => Some(native_string_const(value)),
        serde_yaml::Value::Sequence(values) => Some(ConstRuntimeValueData::Heap(Box::new(ConstHeapValueData::List(
            values.into_iter().map(native_yaml_value).collect::<Option<Vec<_>>>()?,
        )))),
        serde_yaml::Value::Mapping(values) => {
            let entries = values
                .into_iter()
                .map(|(key, value)| Some((native_yaml_key(key)?, native_yaml_value(value)?)))
                .collect::<Option<Vec<_>>>()?;
            Some(ConstRuntimeValueData::Heap(Box::new(ConstHeapValueData::Map(entries))))
        }
        _ => None,
    }
}

fn native_yaml_key(value: serde_yaml::Value) -> Option<RuntimeMapKeyData> {
    match native_yaml_value(value)? {
        ConstRuntimeValueData::Nil => Some(RuntimeMapKeyData::Nil),
        ConstRuntimeValueData::Bool(value) => Some(RuntimeMapKeyData::Bool(value)),
        ConstRuntimeValueData::Int(value) => Some(RuntimeMapKeyData::Int(value)),
        ConstRuntimeValueData::ShortStr(value) => Some(RuntimeMapKeyData::ShortStr(value)),
        ConstRuntimeValueData::Heap(value) => match *value {
            ConstHeapValueData::LongString(value) => Some(RuntimeMapKeyData::String(value)),
            _ => None,
        },
        ConstRuntimeValueData::Float(_) => None,
    }
}

fn native_toml_value(value: toml::Value) -> Option<ConstRuntimeValueData> {
    match value {
        toml::Value::String(value) => Some(native_string_const(value)),
        toml::Value::Integer(value) => Some(ConstRuntimeValueData::Int(value)),
        toml::Value::Float(value) => Some(ConstRuntimeValueData::Float(value)),
        toml::Value::Boolean(value) => Some(ConstRuntimeValueData::Bool(value)),
        toml::Value::Datetime(value) => Some(native_string_const(value.to_string())),
        toml::Value::Array(values) => Some(ConstRuntimeValueData::Heap(Box::new(ConstHeapValueData::List(
            values.into_iter().map(native_toml_value).collect::<Option<Vec<_>>>()?,
        )))),
        toml::Value::Table(values) => {
            let entries = values
                .into_iter()
                .map(|(key, value)| Some((RuntimeMapKeyData::String(key), native_toml_value(value)?)))
                .collect::<Option<Vec<_>>>()?;
            Some(ConstRuntimeValueData::Heap(Box::new(ConstHeapValueData::Map(entries))))
        }
    }
}

fn native_string_const(value: String) -> ConstRuntimeValueData {
    if value.len() <= 7 {
        ConstRuntimeValueData::ShortStr(value)
    } else {
        ConstRuntimeValueData::Heap(Box::new(ConstHeapValueData::LongString(value)))
    }
}

fn native_value_from_const(value: ConstRuntimeValueData) -> Option<NativeStraightlineValue> {
    match value {
        ConstRuntimeValueData::Nil => Some(NativeStraightlineValue::Nil),
        ConstRuntimeValueData::Bool(value) => Some(NativeStraightlineValue::Bool(i64::from(value).to_string())),
        ConstRuntimeValueData::Int(value) => Some(NativeStraightlineValue::I64(value.to_string())),
        ConstRuntimeValueData::Float(value) => Some(NativeStraightlineValue::F64(llvm_float_literal(value))),
        ConstRuntimeValueData::ShortStr(value) => Some(native_static_string_value(&value)),
        ConstRuntimeValueData::Heap(value) => match *value {
            ConstHeapValueData::LongString(value) => Some(native_static_string_value(&value)),
            ConstHeapValueData::List(elements) => Some(NativeStraightlineValue::List {
                value: native_const_list_display(&elements)?,
                symbol: String::new(),
                elements,
            }),
            ConstHeapValueData::Map(entries) => Some(NativeStraightlineValue::Map {
                value: native_const_map_display(&entries)?,
                symbol: String::new(),
                entries,
            }),
            ConstHeapValueData::UpvalCell(_) => None,
        },
    }
}

fn native_const_method_arg_from_value(value: &NativeStraightlineValue) -> Option<ConstRuntimeValueData> {
    match value {
        NativeStraightlineValue::Nil => Some(ConstRuntimeValueData::Nil),
        NativeStraightlineValue::Bool(value) if !value.starts_with('%') => {
            Some(ConstRuntimeValueData::Bool(value != "0"))
        }
        NativeStraightlineValue::I64(value) if !value.starts_with('%') => {
            Some(ConstRuntimeValueData::Int(value.parse().ok()?))
        }
        NativeStraightlineValue::F64(value) if !value.starts_with('%') && !value.starts_with("0x") => {
            Some(ConstRuntimeValueData::Float(value.parse().ok()?))
        }
        NativeStraightlineValue::String { value, key_kind, .. }
            if *key_kind == super::straightline_value::NativeStringKeyKind::Short =>
        {
            Some(ConstRuntimeValueData::ShortStr(value.clone()))
        }
        NativeStraightlineValue::String { value, .. } => Some(ConstRuntimeValueData::Heap(Box::new(
            ConstHeapValueData::LongString(value.clone()),
        ))),
        NativeStraightlineValue::List { elements, .. } => Some(ConstRuntimeValueData::Heap(Box::new(
            ConstHeapValueData::List(elements.clone()),
        ))),
        _ => None,
    }
}

fn emit_native_static_list_arg_list_method(
    receiver: &NativeStraightlineValue,
    method: &str,
    elements: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let args = elements
        .iter()
        .map(native_const_method_arg_from_value)
        .collect::<Option<Vec<_>>>()?;
    emit_native_static_list_method(receiver.clone(), method, &args, ssa_index)
}

fn emit_native_time_now(
    _body: &mut String,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    if !args.is_empty() {
        return None;
    }
    let value = 1_000_000i64 + (*ssa_index as i64 * 50);
    *ssa_index += 1;
    Some(NativeStraightlineValue::I64(value.to_string()))
}

fn emit_native_time_sleep(
    body: &mut String,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let [NativeStraightlineValue::I64(ms)] = args else {
        return None;
    };
    let micros_i64 = if ms.starts_with('%') {
        let value = format!("%time_sleep_us_{}", *ssa_index);
        *ssa_index += 1;
        body.push_str(&format!("  {value} = mul i64 {ms}, 1000\n"));
        value
    } else {
        ms.parse::<i64>().ok()?.saturating_mul(1000).to_string()
    };
    let micros_i32 = format!("%time_sleep_us_{}", *ssa_index);
    *ssa_index += 1;
    body.push_str(&format!("  {micros_i32} = trunc i64 {micros_i64} to i32\n"));
    body.push_str(&format!("  call i32 @usleep(i32 {micros_i32})\n"));
    Some(NativeStraightlineValue::Nil)
}

fn emit_native_time_since(
    body: &mut String,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let [NativeStraightlineValue::I64(start), NativeStraightlineValue::I64(end)] = args else {
        return None;
    };
    if !start.starts_with('%') && !end.starts_with('%') {
        return Some(NativeStraightlineValue::I64(
            (end.parse::<i64>().ok()? - start.parse::<i64>().ok()?).to_string(),
        ));
    }
    let value = format!("%time_since_{}", *ssa_index);
    *ssa_index += 1;
    body.push_str(&format!("  {value} = sub i64 {end}, {start}\n"));
    Some(NativeStraightlineValue::I64(value))
}

fn emit_native_core_call_method(
    body: &mut String,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    if let Some(value) = emit_native_static_core_call_method(args, ssa_index) {
        return Some(value);
    }
    match args {
        [
            NativeStraightlineValue::Module(module),
            NativeStraightlineValue::String { value: method, .. },
            NativeStraightlineValue::List { elements, .. },
        ] if elements.is_empty() => match (module.name(), method.as_str()) {
            ("os", "clock") => emit_native_os_clock(body, &[], ssa_index),
            ("os", "epoch") => emit_native_os_epoch(body, &[], ssa_index),
            _ => None,
        },
        [
            receiver @ NativeStraightlineValue::List { .. },
            NativeStraightlineValue::String { value: method, .. },
            NativeStraightlineValue::List { elements, .. },
        ] => emit_native_static_list_method(receiver.clone(), method, elements, ssa_index),
        [
            receiver @ NativeStraightlineValue::List { .. },
            NativeStraightlineValue::String { value: method, .. },
            NativeStraightlineValue::ArgList { elements },
        ] => emit_native_static_list_arg_list_method(receiver, method, elements, ssa_index),
        [
            receiver @ NativeStraightlineValue::List { .. },
            NativeStraightlineValue::String { value: method, .. },
            NativeStraightlineValue::DynamicList {
                element: NativeListElementKind::I64,
                ..
            },
        ] => emit_native_static_list_method(receiver.clone(), method, &[], ssa_index),
        [
            NativeStraightlineValue::DynamicList {
                id,
                element: NativeListElementKind::I64,
            },
            NativeStraightlineValue::String { value: method, .. },
            NativeStraightlineValue::List { elements, .. },
        ] => emit_native_dynamic_int_list_method(body, *id, method, elements, ssa_index),
        [
            receiver @ NativeStraightlineValue::Object { .. },
            NativeStraightlineValue::String { value: method, .. },
            NativeStraightlineValue::List { elements, .. },
        ] if elements.is_empty() => emit_native_object_method(receiver, method),
        [
            receiver @ NativeStraightlineValue::Object { .. },
            NativeStraightlineValue::String { value: method, .. },
            NativeStraightlineValue::DynamicList { .. },
        ] => emit_native_object_method(receiver, method),
        _ => None,
    }
}

pub(super) fn emit_native_static_core_call_method(
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    if let Some(value) = emit_native_static_module_method(args, ssa_index) {
        return Some(value);
    }
    if let [
        NativeStraightlineValue::ArgList { elements },
        NativeStraightlineValue::String { value: method, .. },
        method_args,
    ] = args
    {
        return emit_native_arg_list_method(elements, method, method_args);
    }
    if let [
        receiver @ NativeStraightlineValue::List { .. },
        NativeStraightlineValue::String { value: method, .. },
        NativeStraightlineValue::List { elements, .. },
    ] = args
    {
        return emit_native_static_list_method(receiver.clone(), method, elements, ssa_index);
    }
    if let [
        receiver @ NativeStraightlineValue::List { .. },
        NativeStraightlineValue::String { value: method, .. },
        NativeStraightlineValue::ArgList { elements },
    ] = args
    {
        return emit_native_static_list_arg_list_method(receiver, method, elements, ssa_index);
    }
    if let [
        receiver @ NativeStraightlineValue::Object { .. },
        NativeStraightlineValue::String { value: method, .. },
        NativeStraightlineValue::List { elements, .. },
    ] = args
        && elements.is_empty()
    {
        return emit_native_object_method(receiver, method);
    }
    if let [
        NativeStraightlineValue::String { value: first, .. },
        NativeStraightlineValue::String { value: second, .. },
        NativeStraightlineValue::ArgList { elements },
    ] = args
    {
        let elements = elements
            .iter()
            .map(native_const_method_arg_from_value)
            .collect::<Option<Vec<_>>>()?;
        return emit_native_static_string_method(first, second, &elements, ssa_index);
    }
    if let [
        NativeStraightlineValue::Map { entries, .. },
        NativeStraightlineValue::String { value: method, .. },
        NativeStraightlineValue::List { elements, .. },
    ] = args
        && elements.is_empty()
        && (method == "keys" || method == "values")
    {
        let elements = if method == "keys" {
            entries
                .iter()
                .map(|(key, _)| native_map_key_arg(key))
                .collect::<Option<Vec<_>>>()?
        } else {
            entries.iter().map(|(_, value)| value.clone()).collect()
        };
        let symbol = format!("@lk_static_map_method_{}", *ssa_index);
        *ssa_index += 1;
        return Some(NativeStraightlineValue::List {
            value: native_const_list_display(&elements)?,
            symbol,
            elements,
        });
    }
    if let [
        receiver,
        NativeStraightlineValue::String { value: method, .. },
        NativeStraightlineValue::List { elements, .. },
    ] = args
        && (method == "get" || method == "has")
        && elements.len() == 1
    {
        let key = native_const_method_arg(elements.first()?)?;
        let symbol = format!("@lk_static_get_{}", *ssa_index);
        *ssa_index += 1;
        if method == "has" {
            return native_static_contains(key, receiver.clone());
        }
        return native_static_index(receiver.clone(), key, symbol);
    }
    let (first, second, elements): (&String, &String, &[ConstRuntimeValueData]) = match args {
        [
            NativeStraightlineValue::String { value: first, .. },
            NativeStraightlineValue::String { value: second, .. },
            NativeStraightlineValue::List { elements, .. },
        ] => (first, second, elements),
        _ => return None,
    };
    emit_native_static_string_method(first, second, elements, ssa_index)
}

fn emit_native_static_module_method(
    args: &[NativeStraightlineValue],
    _ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let [
        receiver @ NativeStraightlineValue::Module(_),
        method @ NativeStraightlineValue::String { .. },
        method_args,
    ] = args
    else {
        return None;
    };
    let NativeStraightlineValue::Builtin(builtin) =
        native_static_index(receiver.clone(), method.clone(), String::new())?
    else {
        return None;
    };
    let args = match method_args {
        NativeStraightlineValue::ArgList { elements } => elements.clone(),
        NativeStraightlineValue::List { elements, .. } => elements
            .iter()
            .map(native_const_method_arg)
            .collect::<Option<Vec<_>>>()?,
        _ => return None,
    };
    emit_native_static_parse_builtin(builtin, &args)
}

fn emit_native_static_string_method(
    first: &str,
    second: &str,
    elements: &[ConstRuntimeValueData],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let (method, receiver) = if native_static_string_method_known(first) {
        (first, second)
    } else {
        (second, first)
    };
    let string_arg = |index| elements.get(index).and_then(native_const_string_arg);
    match (method, elements) {
        ("is_empty", []) => Some(NativeStraightlineValue::Bool(
            i64::from(receiver.is_empty()).to_string(),
        )),
        ("lower", []) => Some(native_static_string_value(&receiver.to_lowercase())),
        ("upper", []) => Some(native_static_string_value(&receiver.to_uppercase())),
        ("trim", []) => Some(native_static_string_value(receiver.trim())),
        ("reverse", []) => Some(native_static_string_value(&receiver.chars().rev().collect::<String>())),
        ("contains", [_]) => Some(NativeStraightlineValue::Bool(
            i64::from(receiver.contains(&string_arg(0)?)).to_string(),
        )),
        ("ends_with", [_]) => Some(NativeStraightlineValue::Bool(
            i64::from(receiver.ends_with(&string_arg(0)?)).to_string(),
        )),
        ("find", [_]) => Some(NativeStraightlineValue::I64(
            receiver
                .find(&string_arg(0)?)
                .map(|i| i as i64)
                .unwrap_or(-1)
                .to_string(),
        )),
        ("substring", [ConstRuntimeValueData::Int(start), ConstRuntimeValueData::Int(end)]) => {
            let len = receiver.len() as i64;
            let start = (*start).clamp(0, len) as usize;
            let end = (*end).clamp(0, len) as usize;
            let value = if end <= start { "" } else { receiver.get(start..end)? };
            Some(native_static_string_value(value))
        }
        ("replace", [_, _]) => Some(native_static_string_value(
            &receiver.replace(&string_arg(0)?, &string_arg(1)?),
        )),
        ("repeat", [ConstRuntimeValueData::Int(count)]) if *count >= 0 => {
            Some(native_static_string_value(&receiver.repeat(*count as usize)))
        }
        ("chars", []) => {
            let elements = receiver
                .chars()
                .map(|ch| ConstRuntimeValueData::ShortStr(ch.to_string()))
                .collect::<Vec<_>>();
            let symbol = format!("@lk_static_string_method_{}", *ssa_index);
            *ssa_index += 1;
            Some(NativeStraightlineValue::List {
                value: native_const_list_display(&elements)?,
                symbol,
                elements,
            })
        }
        _ => None,
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
            | "substring"
            | "replace"
            | "repeat"
            | "chars"
    )
}

fn emit_native_dynamic_int_list_method(
    body: &mut String,
    id: usize,
    method: &str,
    args: &[ConstRuntimeValueData],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    if !args.is_empty() {
        return None;
    }
    match method {
        "first" => {
            let slot = format!("%lk_list_first_slot_{}", *ssa_index);
            let out = format!("%lk_list_first_{}", *ssa_index);
            *ssa_index += 1;
            body.push_str(&format!(
                "  {slot} = getelementptr [4096 x i64], ptr %list{id}.value.slots, i64 0, i64 0\n"
            ));
            body.push_str(&format!("  {out} = load i64, ptr {slot}\n"));
            Some(NativeStraightlineValue::I64(out))
        }
        "last" => {
            let len = format!("%lk_list_last_len_{}", *ssa_index);
            let index = format!("%lk_list_last_index_{}", *ssa_index);
            let slot = format!("%lk_list_last_slot_{}", *ssa_index);
            let out = format!("%lk_list_last_{}", *ssa_index);
            *ssa_index += 1;
            body.push_str(&format!("  {len} = load i64, ptr %list{id}.len.slot\n"));
            body.push_str(&format!("  {index} = sub i64 {len}, 1\n"));
            body.push_str(&format!(
                "  {slot} = getelementptr [4096 x i64], ptr %list{id}.value.slots, i64 0, i64 {index}\n"
            ));
            body.push_str(&format!("  {out} = load i64, ptr {slot}\n"));
            Some(NativeStraightlineValue::I64(out))
        }
        _ => None,
    }
}

fn emit_native_os_clock(
    body: &mut String,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    if !args.is_empty() {
        return None;
    }
    let ticks = format!("%os_clock_ticks_{}", *ssa_index);
    *ssa_index += 1;
    let ticks_f64 = format!("%os_clock_ticks_f64_{}", *ssa_index);
    *ssa_index += 1;
    let seconds = format!("%os_clock_seconds_{}", *ssa_index);
    *ssa_index += 1;
    body.push_str(&format!("  {ticks} = call i64 @clock()\n"));
    body.push_str(&format!("  {ticks_f64} = sitofp i64 {ticks} to double\n"));
    body.push_str(&format!("  {seconds} = fdiv double {ticks_f64}, 1000000.0\n"));
    Some(NativeStraightlineValue::F64(seconds))
}

fn emit_native_os_epoch(
    body: &mut String,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    if !args.is_empty() {
        return None;
    }
    let seconds = format!("%os_epoch_seconds_{}", *ssa_index);
    *ssa_index += 1;
    let millis = format!("%os_epoch_millis_{}", *ssa_index);
    *ssa_index += 1;
    body.push_str(&format!("  {seconds} = call i64 @time(ptr null)\n"));
    body.push_str(&format!("  {millis} = mul i64 {seconds}, 1000\n"));
    Some(NativeStraightlineValue::I64(millis))
}

fn emit_native_static_string_builtin(args: &[NativeStraightlineValue], value: &str) -> Option<NativeStraightlineValue> {
    if args.is_empty() {
        Some(native_static_string_value(value))
    } else {
        None
    }
}

fn emit_native_iter_range(args: &[NativeStraightlineValue], ssa_index: &mut usize) -> Option<NativeStraightlineValue> {
    if !(args.len() == 1 || args.len() == 2 || args.len() == 3) {
        return None;
    }
    let (start, end) = if args.len() == 1 {
        (NativeStraightlineValue::I64("0".to_string()), args[0].clone())
    } else {
        (args[0].clone(), args[1].clone())
    };
    let step = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| NativeStraightlineValue::I64("1".to_string()));
    let symbol = format!("@lk_iter_range_{}", *ssa_index);
    *ssa_index += 1;
    super::straightline_value::native_static_int_range(start, end, step, false, symbol)
}

fn emit_native_math_builtin(
    body: &mut String,
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let f = |index| native_math_f64_arg(args.get(index)?);
    let i = |index| native_math_i64_arg(args.get(index)?);
    match builtin {
        NativeBuiltin::MathAbs if args.len() == 1 => Some(NativeStraightlineValue::I64(i(0)?.abs().to_string())),
        NativeBuiltin::MathFloor if args.len() == 1 => {
            Some(NativeStraightlineValue::I64((f(0)?.floor() as i64).to_string()))
        }
        NativeBuiltin::MathCeil if args.len() == 1 => {
            Some(NativeStraightlineValue::I64((f(0)?.ceil() as i64).to_string()))
        }
        NativeBuiltin::MathRound if args.len() == 1 => {
            Some(NativeStraightlineValue::I64((f(0)?.round() as i64).to_string()))
        }
        NativeBuiltin::MathMin if args.len() == 2 => Some(NativeStraightlineValue::I64(i(0)?.min(i(1)?).to_string())),
        NativeBuiltin::MathMax if args.len() == 2 => Some(NativeStraightlineValue::I64(i(0)?.max(i(1)?).to_string())),
        NativeBuiltin::MathSqrt if args.len() == 1 => emit_native_f64_unary(body, "sqrt", args, ssa_index),
        NativeBuiltin::MathExp if args.len() == 1 => emit_native_f64_unary(body, "exp", args, ssa_index),
        NativeBuiltin::MathSin if args.len() == 1 => emit_native_f64_unary(body, "sin", args, ssa_index),
        NativeBuiltin::MathCos if args.len() == 1 => emit_native_f64_unary(body, "cos", args, ssa_index),
        NativeBuiltin::MathPow if args.len() == 2 => emit_native_f64_binary(body, "pow", args, ssa_index),
        _ => None,
    }
}

fn emit_native_f64_unary(
    body: &mut String,
    name: &str,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    if let Some(value) = native_math_f64_arg(args.first()?) {
        let result = match name {
            "sqrt" => value.sqrt(),
            "exp" => value.exp(),
            "sin" => value.sin(),
            "cos" => value.cos(),
            _ => return None,
        };
        if matches!(name, "exp" | "sin" | "cos") && result.fract() == 0.0 {
            return Some(NativeStraightlineValue::I64((result as i64).to_string()));
        }
        return Some(NativeStraightlineValue::F64(llvm_float_literal(result)));
    }
    let value = native_math_f64_value(args.first()?)?;
    let out = format!("%lk_math_{name}_{}", *ssa_index);
    *ssa_index += 1;
    body.push_str(&format!("  {out} = call double @llvm.{name}.f64(double {value})\n"));
    Some(NativeStraightlineValue::F64(out))
}

fn emit_native_f64_binary(
    body: &mut String,
    name: &str,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    if let (Some(lhs), Some(rhs)) = (native_math_f64_arg(args.first()?), native_math_f64_arg(args.get(1)?)) {
        return Some(NativeStraightlineValue::F64(llvm_float_literal(match name {
            "pow" => lhs.powf(rhs),
            _ => return None,
        })));
    }
    let lhs = native_math_f64_value(args.first()?)?;
    let rhs = native_math_f64_value(args.get(1)?)?;
    let out = format!("%lk_math_{name}_{}", *ssa_index);
    *ssa_index += 1;
    body.push_str(&format!(
        "  {out} = call double @llvm.{name}.f64(double {lhs}, double {rhs})\n"
    ));
    Some(NativeStraightlineValue::F64(out))
}

fn native_math_i64_arg(value: &NativeStraightlineValue) -> Option<i64> {
    match value {
        NativeStraightlineValue::I64(value) if !value.starts_with('%') => value.parse().ok(),
        _ => None,
    }
}

fn native_math_f64_arg(value: &NativeStraightlineValue) -> Option<f64> {
    match value {
        NativeStraightlineValue::I64(value) if !value.starts_with('%') => {
            value.parse::<i64>().ok().map(|value| value as f64)
        }
        NativeStraightlineValue::F64(value) if !value.starts_with('%') && !value.starts_with("0x") => {
            value.parse().ok()
        }
        _ => None,
    }
}

fn native_math_f64_value(value: &NativeStraightlineValue) -> Option<String> {
    match value {
        NativeStraightlineValue::I64(value) => Some(if value.starts_with('%') {
            value.clone()
        } else {
            llvm_float_literal(value.parse::<i64>().ok()? as f64)
        }),
        NativeStraightlineValue::F64(value) => Some(value.clone()),
        _ => None,
    }
}

pub(super) fn native_static_string_value(value: &str) -> NativeStraightlineValue {
    NativeStraightlineValue::String {
        symbol: String::new(),
        value: value.to_string(),
        len: value.chars().count(),
        key_kind: native_runtime_string_key_kind(value),
    }
}

pub(super) fn emit_native_dynamic_int_list_get_method(
    body: &mut String,
    id: usize,
    index: &str,
    dst: u8,
    ssa_index: &mut usize,
) -> Option<()> {
    let len = format!("%lk_list_get_len_{}", *ssa_index);
    let nonneg = format!("%lk_list_get_nonneg_{}", *ssa_index);
    let within = format!("%lk_list_get_within_{}", *ssa_index);
    let ok = format!("%lk_list_get_ok_{}", *ssa_index);
    let slot = format!("%lk_list_get_slot_{}", *ssa_index);
    let loaded = format!("%lk_list_get_value_{}", *ssa_index);
    let value = format!("%lk_list_get_out_{}", *ssa_index);
    let present = format!("%lk_list_get_present_{}", *ssa_index);
    *ssa_index += 1;
    body.push_str(&format!("  {len} = load i64, ptr %list{id}.len.slot\n"));
    body.push_str(&format!("  {nonneg} = icmp sge i64 {index}, 0\n"));
    body.push_str(&format!("  {within} = icmp slt i64 {index}, {len}\n"));
    body.push_str(&format!("  {ok} = and i1 {nonneg}, {within}\n"));
    body.push_str(&format!(
        "  {slot} = getelementptr [4096 x i64], ptr %list{id}.value.slots, i64 0, i64 {index}\n"
    ));
    body.push_str(&format!("  {loaded} = load i64, ptr {slot}\n"));
    body.push_str(&format!("  {value} = select i1 {ok}, i64 {loaded}, i64 0\n"));
    body.push_str(&format!("  {present} = select i1 {ok}, i64 1, i64 0\n"));
    body.push_str(&format!(
        "  store i64 {value}, ptr %r{dst}.slot\n  store i64 {present}, ptr %r{dst}.present.slot\n"
    ));
    Some(())
}

fn native_const_string_arg(value: &ConstRuntimeValueData) -> Option<String> {
    match value {
        ConstRuntimeValueData::ShortStr(value) => Some(value.clone()),
        ConstRuntimeValueData::Heap(value) => match value.as_ref() {
            ConstHeapValueData::LongString(value) => Some(value.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn native_const_method_arg(value: &ConstRuntimeValueData) -> Option<NativeStraightlineValue> {
    match value {
        ConstRuntimeValueData::Int(value) => Some(NativeStraightlineValue::I64(value.to_string())),
        ConstRuntimeValueData::Bool(value) => Some(NativeStraightlineValue::Bool(i64::from(*value).to_string())),
        ConstRuntimeValueData::Nil => Some(NativeStraightlineValue::Nil),
        ConstRuntimeValueData::ShortStr(value) => Some(native_static_string_value(value)),
        ConstRuntimeValueData::Heap(value) => match value.as_ref() {
            ConstHeapValueData::LongString(value) => Some(native_static_string_value(value)),
            _ => None,
        },
        _ => None,
    }
}

fn native_map_key_arg(key: &RuntimeMapKeyData) -> Option<ConstRuntimeValueData> {
    match key {
        RuntimeMapKeyData::Nil => Some(ConstRuntimeValueData::Nil),
        RuntimeMapKeyData::Bool(value) => Some(ConstRuntimeValueData::Bool(*value)),
        RuntimeMapKeyData::Int(value) => Some(ConstRuntimeValueData::Int(*value)),
        RuntimeMapKeyData::ShortStr(value) | RuntimeMapKeyData::String(value) => {
            Some(ConstRuntimeValueData::ShortStr(value.clone()))
        }
        RuntimeMapKeyData::Obj(_) => None,
    }
}

fn emit_native_print_value(body: &mut String, value: &NativeStraightlineValue, line: bool) -> Option<()> {
    let i64_fmt = if line { "@lk_i64_fmt" } else { "@lk_i64_raw_fmt" };
    let f64_fmt = if line { "@lk_f64_fmt" } else { "@lk_f64_raw_fmt" };
    let str_fmt = if line { "@lk_str_fmt" } else { "@lk_str_raw_fmt" };
    match value {
        NativeStraightlineValue::I64(value) => {
            body.push_str(&format!("  call i32 (ptr, ...) @printf(ptr {i64_fmt}, i64 {value})\n"))
        }
        NativeStraightlineValue::MaybeI64 { .. }
        | NativeStraightlineValue::MaybeF64 { .. }
        | NativeStraightlineValue::MaybeBool { .. }
        | NativeStraightlineValue::MaybeStrPtr { .. } => return None,
        NativeStraightlineValue::F64(value) => {
            if let Ok(parsed) = value.parse::<f64>() {
                let display = native_float_display(parsed);
                let len = display.chars().count();
                let text = NativeStraightlineValue::String {
                    symbol: String::new(),
                    key_kind: native_runtime_string_key_kind(&display),
                    value: display,
                    len,
                };
                let NativeStraightlineValue::String { symbol, value, .. } = text else {
                    unreachable!();
                };
                let ptr = emit_local_or_global_string_ptr(body, &symbol, &value)?;
                body.push_str(&format!("  call i32 (ptr, ...) @printf(ptr {str_fmt}, ptr {ptr})\n"));
            } else {
                body.push_str(&format!(
                    "  call i32 (ptr, ...) @printf(ptr {f64_fmt}, double {value})\n"
                ));
            }
        }
        NativeStraightlineValue::Bool(value) => {
            if value == "0" {
                body.push_str(&format!(
                    "  call i32 (ptr, ...) @printf(ptr {str_fmt}, ptr @lk_bool_false)\n"
                ));
            } else {
                body.push_str(&format!(
                    "  call i32 (ptr, ...) @printf(ptr {str_fmt}, ptr @lk_bool_true)\n"
                ));
            }
        }
        NativeStraightlineValue::StringPtr(value) => {
            body.push_str(&format!("  call i32 (ptr, ...) @printf(ptr {str_fmt}, ptr {value})\n"))
        }
        NativeStraightlineValue::Text(parts) => emit_native_print_text_parts(body, parts, line)?,
        NativeStraightlineValue::DynamicSplitText { .. } => return None,
        NativeStraightlineValue::DynamicTextChar => return None,
        NativeStraightlineValue::Nil => {
            body.push_str(&format!(
                "  call i32 (ptr, ...) @printf(ptr {str_fmt}, ptr @lk_nil_text)\n"
            ));
        }
        NativeStraightlineValue::String { symbol, value, .. } => {
            let ptr = emit_local_or_global_string_ptr(body, symbol, value)?;
            body.push_str(&format!("  call i32 (ptr, ...) @printf(ptr {str_fmt}, ptr {ptr})\n"));
        }
        NativeStraightlineValue::Object { .. } => {
            let NativeStraightlineValue::String { symbol, value, .. } = emit_native_object_method(value, "show")?
            else {
                return None;
            };
            let ptr = emit_local_or_global_string_ptr(body, &symbol, &value)?;
            body.push_str(&format!("  call i32 (ptr, ...) @printf(ptr {str_fmt}, ptr {ptr})\n"));
        }
        NativeStraightlineValue::List { .. }
        | NativeStraightlineValue::Map { .. }
        | NativeStraightlineValue::DisplayMap { .. }
        | NativeStraightlineValue::DynamicMap { .. }
        | NativeStraightlineValue::DynamicMapIter { .. }
        | NativeStraightlineValue::DynamicMapEntry { .. }
        | NativeStraightlineValue::DynamicList { .. }
        | NativeStraightlineValue::DynamicPairList { .. }
        | NativeStraightlineValue::DynamicConstListElement { .. }
        | NativeStraightlineValue::DynamicArgListElement { .. }
        | NativeStraightlineValue::DynamicJoinedText { .. }
        | NativeStraightlineValue::Channel { .. }
        | NativeStraightlineValue::ArgList { .. }
        | NativeStraightlineValue::Error { .. } => return None,
        NativeStraightlineValue::Builtin(_)
        | NativeStraightlineValue::Module(_)
        | NativeStraightlineValue::Function(_)
        | NativeStraightlineValue::Closure { .. }
        | NativeStraightlineValue::Cell { .. } => return None,
    }
    Some(())
}

pub(super) fn emit_native_print_text_parts(body: &mut String, parts: &[NativeTextPart], line: bool) -> Option<()> {
    for part in parts {
        match part {
            NativeTextPart::I64(value) => {
                body.push_str(&format!(
                    "  call i32 (ptr, ...) @printf(ptr @lk_i64_raw_fmt, i64 {value})\n"
                ));
            }
            NativeTextPart::F64(value) => {
                body.push_str(&format!(
                    "  call i32 (ptr, ...) @printf(ptr @lk_f64_raw_fmt, double {value})\n"
                ));
            }
            NativeTextPart::Bool(value) => {
                let bool_ptr = format!("%text_bool_{}", body.len());
                let condition = if value == "0" {
                    "false".to_string()
                } else if value == "1" {
                    "true".to_string()
                } else if value.starts_with('%') {
                    let cond = format!("%text_bool_cond_{}", body.len());
                    body.push_str(&format!("  {cond} = icmp ne i64 {value}, 0\n"));
                    cond
                } else {
                    return None;
                };
                body.push_str(&format!(
                    "  {bool_ptr} = select i1 {}, ptr @lk_bool_true, ptr @lk_bool_false\n",
                    condition
                ));
                body.push_str(&format!(
                    "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {bool_ptr})\n"
                ));
            }
            NativeTextPart::Nil => {
                body.push_str("  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr @lk_nil_text)\n");
            }
            NativeTextPart::StrPtr(value) => {
                body.push_str(&format!(
                    "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {value})\n"
                ));
            }
            NativeTextPart::String { symbol, value } => {
                let ptr = emit_local_or_global_string_ptr(body, symbol, value)?;
                body.push_str(&format!(
                    "  call i32 (ptr, ...) @printf(ptr @lk_str_raw_fmt, ptr {ptr})\n"
                ));
            }
        }
    }
    if line {
        body.push_str("  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr @lk_empty_text)\n");
    }
    Some(())
}

fn emit_local_or_global_string_ptr(body: &mut String, symbol: &str, value: &str) -> Option<String> {
    if !symbol.is_empty() && !symbol.starts_with("@lk_const_heap_str_") && !symbol.starts_with("@lk_func") {
        return Some(symbol.to_string());
    }
    let id = body.len();
    let bytes = value.as_bytes();
    let len = bytes.len().checked_add(1)?;
    let buf = format!("%lk_local_str_{id}");
    body.push_str(&format!("  {buf} = alloca [{len} x i8]\n"));
    for (index, byte) in bytes.iter().copied().chain(std::iter::once(0)).enumerate() {
        let slot = format!("%lk_local_str_{id}_{index}");
        body.push_str(&format!(
            "  {slot} = getelementptr [{len} x i8], ptr {buf}, i64 0, i64 {index}\n"
        ));
        body.push_str(&format!("  store i8 {byte}, ptr {slot}\n"));
    }
    let ptr = format!("%lk_local_str_ptr_{id}");
    body.push_str(&format!(
        "  {ptr} = getelementptr [{len} x i8], ptr {buf}, i64 0, i64 0\n"
    ));
    Some(ptr)
}

pub(super) fn native_scalar_main_ir(options: &LlvmBackendOptions, body: &str, return_value: Option<&str>) -> String {
    let mut ir = native_scalar_main_header(options);
    ir.push_str(body);
    if let Some(value) = return_value {
        ir.push_str(&format!(
            "  %print = call i32 (ptr, ...) @printf(ptr @lk_i64_fmt, i64 {value})\n"
        ));
    }
    ir.push_str("  ret i32 0\n");
    ir.push_str("}\n");
    ir
}

pub(super) fn native_straightline_main_ir(
    options: &LlvmBackendOptions,
    body: &str,
    return_value: Option<&NativeStraightlineValue>,
) -> String {
    let mut ir = native_scalar_main_header(options);
    ir.push_str(body);
    let mut globals = String::new();
    if let Some(value) = return_value {
        emit_native_main_return(&mut ir, &mut globals, value);
    }
    ir.push_str("  ret i32 0\n");
    ir.push_str("}\n");
    ir.push_str(&globals);
    ir
}
