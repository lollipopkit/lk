use crate::{
    llvm::{
        const_display::{native_const_list_display, native_const_map_display, native_const_object_display},
        ir_text::native_float_display,
    },
    vm::RuntimeMapKeyData,
};

use super::{NativeBuiltin, NativeModule, NativeStraightlineValue, NativeTextPart};

#[derive(Clone, Copy)]
enum NativeExportDisplay {
    Fn(&'static str, Option<u16>),
    F64(f64),
    I64(i64),
    Raw(&'static str),
}

pub(super) fn native_builtin_display(builtin: NativeBuiltin) -> String {
    let (name, arity) = match builtin {
        NativeBuiltin::Print => ("print", None),
        NativeBuiltin::Println => ("println", None),
        NativeBuiltin::Panic => ("panic", None),
        NativeBuiltin::Chan => ("chan", None),
        NativeBuiltin::BitAnd => ("__lk_bit_and", Some(2)),
        NativeBuiltin::BitNot => ("__lk_bit_not", Some(1)),
        NativeBuiltin::BitOr => ("__lk_bit_or", Some(2)),
        NativeBuiltin::CoreCallMethod => ("__lk_call_method", None),
        NativeBuiltin::CoreMakeStruct => ("__lk_make_struct", None),
        NativeBuiltin::CoreMergeFields => ("__lk_merge_fields", None),
        NativeBuiltin::CoreRegisterTrait => ("__lk_register_trait", None),
        NativeBuiltin::CoreRegisterTraitImpl => ("__lk_register_trait_impl", None),
        NativeBuiltin::CoreTypeof => ("typeof", Some(1)),
        NativeBuiltin::Recv => ("recv", Some(1)),
        NativeBuiltin::Send => ("send", Some(2)),
        NativeBuiltin::DatetimeAdd => ("add", Some(2)),
        NativeBuiltin::DatetimeDayOfWeek => ("day_of_week", Some(1)),
        NativeBuiltin::DatetimeDayOfYear => ("day_of_year", Some(1)),
        NativeBuiltin::DatetimeFormat => ("format", Some(2)),
        NativeBuiltin::DatetimeIsWeekend => ("is_weekend", Some(1)),
        NativeBuiltin::DatetimeNow => ("now", Some(0)),
        NativeBuiltin::DatetimeSub => ("sub", Some(2)),
        NativeBuiltin::OsClock
        | NativeBuiltin::OsEpoch
        | NativeBuiltin::OsHostname
        | NativeBuiltin::OsArch
        | NativeBuiltin::OsName
        | NativeBuiltin::OsDirCurrent
        | NativeBuiltin::OsDirTemp => ("os::<native>", Some(0)),
        NativeBuiltin::OsDirList => ("os::<native>", Some(1)),
        NativeBuiltin::OsEnvGet => ("os::<native>", None),
        NativeBuiltin::OsEnvSet => ("os::<native>", Some(2)),
        NativeBuiltin::OsEnvUnset => ("os::<native>", Some(1)),
        NativeBuiltin::IterRange => ("range", None),
        NativeBuiltin::IterMap => ("map", Some(2)),
        NativeBuiltin::IterFilter => ("filter", Some(2)),
        NativeBuiltin::IterReduce => ("reduce", None),
        NativeBuiltin::IterTake => ("take", Some(2)),
        NativeBuiltin::IterSkip => ("skip", Some(2)),
        NativeBuiltin::IterChain => ("chain", Some(2)),
        NativeBuiltin::IterFlatten => ("flatten", Some(1)),
        NativeBuiltin::IterUnique => ("unique", Some(1)),
        NativeBuiltin::IterChunk => ("chunk", Some(2)),
        NativeBuiltin::IterEnumerate => ("enumerate", Some(1)),
        NativeBuiltin::IterZip => ("zip", Some(2)),
        NativeBuiltin::IterModuleMethod(method) => (method, Some(1)),
        NativeBuiltin::IoRead => ("stdin_read", None),
        NativeBuiltin::IoStderrFlush => ("stderr_flush", Some(0)),
        NativeBuiltin::IoStderrWrite => ("stderr_write", Some(1)),
        NativeBuiltin::IoStderrWriteln => ("stderr_writeln", Some(1)),
        NativeBuiltin::IoStdoutFlush => ("stdout_flush", Some(0)),
        NativeBuiltin::IoStdoutWrite => ("stdout_write", Some(1)),
        NativeBuiltin::IoStdoutWriteln => ("stdout_writeln", Some(1)),
        NativeBuiltin::JsonParse | NativeBuiltin::TomlParse | NativeBuiltin::YamlParse => ("parse", Some(1)),
        NativeBuiltin::TimeNow => ("now", Some(0)),
        NativeBuiltin::TimeSleep => ("sleep", Some(1)),
        NativeBuiltin::TimeSince => ("since", Some(2)),
        NativeBuiltin::TcpClose => ("close", Some(1)),
        NativeBuiltin::TcpConnect => ("connect", Some(2)),
        NativeBuiltin::TcpRead => ("read", None),
        NativeBuiltin::TcpWrite => ("write", Some(2)),
        NativeBuiltin::StreamCollect => ("collect", None),
        NativeBuiltin::StreamFromList => ("from_list", Some(1)),
        NativeBuiltin::StringLen => ("len", Some(1)),
        NativeBuiltin::StringModuleMethod(method) => (method, None),
        NativeBuiltin::ListConcat => ("concat", Some(2)),
        NativeBuiltin::ListContains => ("contains", Some(2)),
        NativeBuiltin::ListFirst => ("first", Some(1)),
        NativeBuiltin::ListGet => ("get", Some(2)),
        NativeBuiltin::ListIndexOf => ("index_of", Some(2)),
        NativeBuiltin::ListInsert => ("insert", Some(3)),
        NativeBuiltin::ListIsEmpty => ("is_empty", Some(1)),
        NativeBuiltin::ListJoin => ("join", Some(2)),
        NativeBuiltin::ListLast => ("last", Some(1)),
        NativeBuiltin::ListLen => ("len", Some(1)),
        NativeBuiltin::ListPop => ("pop", Some(1)),
        NativeBuiltin::ListPush => ("push", Some(2)),
        NativeBuiltin::ListRemoveAt => ("remove_at", Some(2)),
        NativeBuiltin::ListReverse => ("reverse", Some(1)),
        NativeBuiltin::ListSet => ("set", Some(3)),
        NativeBuiltin::ListSlice => ("slice", None),
        NativeBuiltin::ListSort => ("sort", Some(1)),
        NativeBuiltin::MathAbs => ("abs", Some(1)),
        NativeBuiltin::MathSqrt => ("sqrt", Some(1)),
        NativeBuiltin::MathFloor => ("floor", Some(1)),
        NativeBuiltin::MathCeil => ("ceil", Some(1)),
        NativeBuiltin::MathRound => ("round", Some(1)),
        NativeBuiltin::MathMin => ("min", Some(2)),
        NativeBuiltin::MathMax => ("max", Some(2)),
        NativeBuiltin::MathPow => ("pow", Some(2)),
        NativeBuiltin::MathExp => ("exp", Some(1)),
        NativeBuiltin::MathSin => ("sin", Some(1)),
        NativeBuiltin::MathCos => ("cos", Some(1)),
        NativeBuiltin::MathModuleMethod(method) => (method, None),
        NativeBuiltin::FibIterative => ("iterative", Some(1)),
        NativeBuiltin::GreetingsMessage => ("message", Some(1)),
        NativeBuiltin::MathlibDouble => ("double", Some(1)),
        NativeBuiltin::MapModuleMethod(method) => (method, None),
        NativeBuiltin::MapDelete => ("delete", Some(2)),
        NativeBuiltin::MapSet => ("set", Some(3)),
        NativeBuiltin::MapMutate => ("mutate", None),
    };
    match arity {
        Some(arity) => format!("<native fn {name}({arity} args)>"),
        None => format!("<native fn {name}(...)>"),
    }
}

pub(super) fn native_module_display(module: NativeModule) -> Option<String> {
    let mut entries = module_display_entries(module)?;
    entries.sort_by_key(|(name, _)| *name);

    let mut out = String::from("{");
    for (index, (name, display)) in entries.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(name);
        out.push_str(": ");
        match display {
            NativeExportDisplay::Fn(function, arity) => {
                out.push_str(&native_function_display(function, *arity));
            }
            NativeExportDisplay::F64(value) => out.push_str(&native_float_display(*value)),
            NativeExportDisplay::I64(value) => out.push_str(&value.to_string()),
            NativeExportDisplay::Raw(value) => out.push_str(value),
        }
    }
    out.push('}');
    Some(out)
}

pub(super) fn native_arg_list_display(values: &[NativeStraightlineValue]) -> Option<String> {
    let mut out = String::from("[");
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&native_value_display(value)?);
    }
    out.push(']');
    Some(out)
}

pub(super) fn native_display_map_display(entries: &[(RuntimeMapKeyData, NativeStraightlineValue)]) -> Option<String> {
    let mut out = String::from("{");
    for (index, (key, value)) in entries.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&native_map_key_display(key)?);
        out.push_str(": ");
        out.push_str(&native_value_display(value)?);
    }
    out.push('}');
    Some(out)
}

fn native_value_display(value: &NativeStraightlineValue) -> Option<String> {
    match value {
        NativeStraightlineValue::Nil => Some("nil".to_string()),
        NativeStraightlineValue::Bool(value) if value == "0" => Some("false".to_string()),
        NativeStraightlineValue::Bool(value) if value == "1" => Some("true".to_string()),
        NativeStraightlineValue::I64(value) if !value.starts_with('%') => Some(value.clone()),
        NativeStraightlineValue::F64(value) if !value.starts_with('%') => native_f64_display(value),
        NativeStraightlineValue::String { value, .. } => Some(value.clone()),
        NativeStraightlineValue::Text(parts) => native_text_display(parts),
        NativeStraightlineValue::List { elements, .. } => native_const_list_display(elements),
        NativeStraightlineValue::Map { entries, .. } => native_const_map_display(entries),
        NativeStraightlineValue::DisplayMap { entries, .. } => native_display_map_display(entries),
        NativeStraightlineValue::Object { type_name, fields, .. } => native_const_object_display(type_name, fields),
        NativeStraightlineValue::Function(_)
        | NativeStraightlineValue::Closure { .. }
        | NativeStraightlineValue::Builtin(_) => super::native_static_callable_display(value),
        NativeStraightlineValue::Module(_) => super::native_static_module_display(value),
        NativeStraightlineValue::ArgList { elements } => native_arg_list_display(elements),
        NativeStraightlineValue::DynamicArgListElement { .. } => None,
        _ => None,
    }
}

fn native_f64_display(value: &str) -> Option<String> {
    if value.starts_with("0x") {
        return Some(value.to_string());
    }
    Some(value.parse::<f64>().ok()?.to_string())
}

fn native_map_key_display(key: &RuntimeMapKeyData) -> Option<String> {
    match key {
        RuntimeMapKeyData::Nil => Some("nil".to_string()),
        RuntimeMapKeyData::Bool(value) => Some(value.to_string()),
        RuntimeMapKeyData::Int(value) => Some(value.to_string()),
        RuntimeMapKeyData::ShortStr(value) | RuntimeMapKeyData::String(value) => Some(value.clone()),
        RuntimeMapKeyData::Obj(value) => Some(format!("<obj:{}>", value)),
    }
}

fn native_text_display(parts: &[NativeTextPart]) -> Option<String> {
    let mut out = String::new();
    for part in parts {
        match part {
            NativeTextPart::I64(value) | NativeTextPart::F64(value) | NativeTextPart::Bool(value) => {
                if value.starts_with('%') {
                    return None;
                }
                out.push_str(value);
            }
            NativeTextPart::Nil => out.push_str("nil"),
            NativeTextPart::String { value, .. } => out.push_str(value),
            NativeTextPart::StrPtr(_) => return None,
        }
    }
    Some(out)
}

fn native_function_display(name: &str, arity: Option<u16>) -> String {
    match arity {
        Some(arity) => format!("<native fn {name}({arity} args)>"),
        None => format!("<native fn {name}(...)>"),
    }
}

fn module_display_entries(module: NativeModule) -> Option<Vec<(&'static str, NativeExportDisplay)>> {
    use NativeExportDisplay::{F64, Fn, I64, Raw};

    let entries = match module {
        NativeModule::Datetime => vec![
            ("add", Fn("add", Some(2))),
            ("day_of_week", Fn("day_of_week", Some(1))),
            ("day_of_year", Fn("day_of_year", Some(1))),
            ("format", Fn("format", Some(2))),
            ("is_weekend", Fn("is_weekend", Some(1))),
            ("now", Fn("now", Some(0))),
            ("sub", Fn("sub", Some(2))),
        ],
        NativeModule::Fib => vec![("iterative", Fn("iterative", Some(1)))],
        NativeModule::Greetings => vec![("message", Fn("message", Some(1)))],
        NativeModule::Os => vec![
            ("arch", Fn("os::<native>", Some(0))),
            ("clock", Fn("os::<native>", Some(0))),
            ("dir_current", Fn("os::<native>", Some(0))),
            ("dir_list", Fn("os::<native>", Some(1))),
            ("dir_temp", Fn("os::<native>", Some(0))),
            (
                "env",
                Raw(
                    "{get: <native fn os::<native>(...)>, set: <native fn os::<native>(2 args)>, unset: <native fn os::<native>(1 args)>}",
                ),
            ),
            ("env_get", Fn("os::<native>", None)),
            ("env_set", Fn("os::<native>", Some(2))),
            ("env_unset", Fn("os::<native>", Some(1))),
            ("epoch", Fn("os::<native>", Some(0))),
            ("exec", Fn("os::<native>", None)),
            ("exit", Fn("os::<native>", None)),
            ("file_append", Fn("os::<native>", Some(2))),
            ("file_delete", Fn("os::<native>", Some(1))),
            ("file_exists", Fn("os::<native>", Some(1))),
            ("file_read", Fn("os::<native>", Some(1))),
            ("file_size", Fn("os::<native>", Some(1))),
            ("file_write", Fn("os::<native>", Some(2))),
            ("hostname", Fn("os::<native>", Some(0))),
            ("mkdir", Fn("os::<native>", Some(1))),
            ("os", Fn("os::<native>", Some(0))),
            ("path_join", Fn("os::<native>", None)),
            ("path_sep", Fn("os::<native>", Some(0))),
            ("time", Fn("os::<native>", Some(0))),
        ],
        NativeModule::OsEnv => vec![
            ("get", Fn("os::<native>", None)),
            ("set", Fn("os::<native>", Some(2))),
            ("unset", Fn("os::<native>", Some(1))),
        ],
        NativeModule::Iter => vec![
            ("chain", Fn("chain", Some(2))),
            ("chunk", Fn("chunk", Some(2))),
            ("collect", Fn("collect", Some(1))),
            ("enumerate", Fn("enumerate", Some(1))),
            ("filter", Fn("filter", Some(2))),
            ("flatten", Fn("flatten", Some(1))),
            ("map", Fn("map", Some(2))),
            ("next", Fn("next", Some(1))),
            ("range", Fn("range", None)),
            ("reduce", Fn("reduce", Some(3))),
            ("skip", Fn("skip", Some(2))),
            ("take", Fn("take", Some(2))),
            ("unique", Fn("unique", Some(1))),
            ("zip", Fn("zip", Some(2))),
        ],
        NativeModule::Io => vec![
            ("read", Fn("read", Some(0))),
            ("stderr_flush", Fn("stderr_flush", Some(0))),
            ("stderr_write", Fn("stderr_write", Some(1))),
            ("stderr_writeln", Fn("stderr_writeln", Some(1))),
            ("stdin_flush", Fn("stdin_flush", Some(0))),
            ("stdin_read", Fn("stdin_read", None)),
            ("stdin_read_all", Fn("stdin_read_all", Some(0))),
            ("stdin_read_line", Fn("stdin_read_line", Some(0))),
            ("stdout_flush", Fn("stdout_flush", Some(0))),
            ("stdout_write", Fn("stdout_write", Some(1))),
            ("stdout_writeln", Fn("stdout_writeln", Some(1))),
        ],
        NativeModule::Json => vec![("parse", Fn("parse", Some(1)))],
        NativeModule::Math => vec![
            ("abs", Fn("abs", Some(1))),
            ("acos", Fn("acos", Some(1))),
            ("asin", Fn("asin", Some(1))),
            ("atan", Fn("atan", Some(1))),
            ("atan2", Fn("atan2", Some(2))),
            ("cbrt", Fn("cbrt", Some(1))),
            ("ceil", Fn("ceil", Some(1))),
            ("clamp", Fn("clamp", None)),
            ("cos", Fn("cos", Some(1))),
            ("cosh", Fn("cosh", Some(1))),
            ("e", F64(std::f64::consts::E)),
            ("epsilon", F64(f64::EPSILON)),
            ("exp", Fn("exp", Some(1))),
            ("floor", Fn("floor", Some(1))),
            ("fract", Fn("fract", Some(1))),
            ("hypot", Fn("hypot", Some(2))),
            ("inf", F64(f64::INFINITY)),
            ("is_inf", Fn("is_inf", Some(1))),
            ("is_nan", Fn("is_nan", Some(1))),
            ("log", Fn("log", Some(1))),
            ("log10", Fn("log10", Some(1))),
            ("log2", Fn("log2", Some(1))),
            ("max", Fn("max", Some(2))),
            ("max_float", F64(f64::MAX)),
            ("max_int", I64(i64::MAX)),
            ("min", Fn("min", Some(2))),
            ("min_int", I64(i64::MIN)),
            ("nan", F64(f64::NAN)),
            ("pi", F64(std::f64::consts::PI)),
            ("pow", Fn("pow", Some(2))),
            ("random", Fn("random", Some(0))),
            ("round", Fn("round", Some(1))),
            ("sign", Fn("sign", Some(1))),
            ("sin", Fn("sin", Some(1))),
            ("sinh", Fn("sinh", Some(1))),
            ("sqrt", Fn("sqrt", Some(1))),
            ("tan", Fn("tan", Some(1))),
            ("tanh", Fn("tanh", Some(1))),
            ("to_float", Fn("to_float", Some(1))),
            ("to_int", Fn("to_int", Some(1))),
            ("trunc", Fn("trunc", Some(1))),
        ],
        NativeModule::Mathlib => vec![("double", Fn("double", Some(1)))],
        NativeModule::Map => vec![
            ("delete", Fn("delete", Some(2))),
            ("get", Fn("get", Some(2))),
            ("has", Fn("has", Some(2))),
            ("keys", Fn("keys", Some(1))),
            ("len", Fn("len", Some(1))),
            ("mutate", Fn("mutate", Some(2))),
            ("set", Fn("set", Some(3))),
            ("values", Fn("values", Some(1))),
        ],
        NativeModule::List => vec![
            ("concat", Fn("concat", Some(2))),
            ("contains", Fn("contains", Some(2))),
            ("first", Fn("first", Some(1))),
            ("get", Fn("get", Some(2))),
            ("index_of", Fn("index_of", Some(2))),
            ("insert", Fn("insert", Some(3))),
            ("is_empty", Fn("is_empty", Some(1))),
            ("join", Fn("join", Some(2))),
            ("last", Fn("last", Some(1))),
            ("len", Fn("len", Some(1))),
            ("pop", Fn("pop", Some(1))),
            ("push", Fn("push", Some(2))),
            ("remove_at", Fn("remove_at", Some(2))),
            ("reverse", Fn("reverse", Some(1))),
            ("set", Fn("set", Some(3))),
            ("slice", Fn("slice", None)),
            ("sort", Fn("sort", Some(1))),
        ],
        NativeModule::Toml => vec![("parse", Fn("parse", Some(1)))],
        NativeModule::Time => vec![
            ("after", Fn("after", Some(1))),
            ("now", Fn("now", Some(0))),
            ("since", Fn("since", Some(2))),
            ("sleep", Fn("sleep", Some(1))),
            ("timeout", Fn("timeout", Some(1))),
        ],
        NativeModule::Tcp => vec![
            ("close", Fn("close", Some(1))),
            ("connect", Fn("connect", Some(2))),
            ("read", Fn("read", None)),
            ("write", Fn("write", Some(2))),
        ],
        NativeModule::Stream => vec![
            ("chain", Fn("chain", Some(2))),
            ("collect", Fn("collect", None)),
            ("collect_block", Fn("collect_block", None)),
            ("filter", Fn("filter", Some(2))),
            ("from_channel", Fn("from_channel", Some(1))),
            ("from_list", Fn("from_list", Some(1))),
            ("iterate", Fn("iterate", Some(2))),
            ("map", Fn("map", Some(2))),
            ("next", Fn("next", Some(1))),
            ("next_block", Fn("next_block", None)),
            ("range", Fn("range", None)),
            ("repeat", Fn("repeat", Some(1))),
            ("skip", Fn("skip", Some(2))),
            ("subscribe", Fn("subscribe", Some(1))),
            ("take", Fn("take", Some(2))),
        ],
        NativeModule::String => vec![
            ("byte", Fn("byte", Some(2))),
            ("capitalize", Fn("capitalize", Some(1))),
            ("char", Fn("char", Some(2))),
            ("chars", Fn("chars", Some(1))),
            ("contains", Fn("contains", Some(2))),
            ("count", Fn("count", Some(2))),
            ("ends_with", Fn("ends_with", Some(2))),
            ("find", Fn("find", None)),
            ("format", Fn("format", None)),
            ("is_empty", Fn("is_empty", Some(1))),
            ("join", Fn("join", Some(2))),
            ("len", Fn("len", Some(1))),
            ("lower", Fn("lower", Some(1))),
            ("pad_left", Fn("pad_left", Some(3))),
            ("pad_right", Fn("pad_right", Some(3))),
            ("repeat", Fn("repeat", Some(2))),
            ("replace", Fn("replace", None)),
            ("reverse", Fn("reverse", Some(1))),
            ("split", Fn("split", Some(2))),
            ("starts_with", Fn("starts_with", Some(2))),
            ("strip", Fn("strip", Some(2))),
            ("strip_prefix", Fn("strip_prefix", Some(2))),
            ("strip_suffix", Fn("strip_suffix", Some(2))),
            ("substring", Fn("substring", Some(3))),
            ("title", Fn("title", Some(1))),
            ("to_float", Fn("to_float", Some(1))),
            ("to_int", Fn("to_int", Some(1))),
            ("trim", Fn("trim", Some(1))),
            ("upper", Fn("upper", Some(1))),
        ],
        NativeModule::Yaml => vec![("parse", Fn("parse", Some(1)))],
    };
    Some(entries)
}
