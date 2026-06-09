use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use lk_stdlib::{StdlibArity, StdlibConstValue, StdlibExportKind, StdlibReturnKind, stdlib_catalog};

use crate::llvm::{
    ir_text::llvm_float_literal,
    scalar::kind::NativeScalarKind,
    straightline_value::{NativeBuiltin, NativeModule, NativeStraightlineValue, native_runtime_string_key_kind},
};

pub(super) fn stdlib_module_name(name: &str) -> Option<&'static str> {
    stdlib_catalog()
        .module(name)
        .map(|module| leak_catalog_string(module.name.clone()))
}

pub(super) fn stdlib_global_name(name: &str) -> Option<&'static str> {
    stdlib_catalog()
        .global(name)
        .map(|global| leak_catalog_string(global.name.clone()))
}

pub(super) fn stdlib_global_builtin(name: &str) -> Option<NativeStraightlineValue> {
    lowering_key_to_value(stdlib_catalog().global(name)?.lowering_key?)
}

pub(super) fn stdlib_module_display(name: &str) -> Option<String> {
    stdlib_catalog().module(name).map(|module| module.display.clone())
}

pub(super) fn stdlib_builtin_display(builtin: NativeBuiltin) -> Option<String> {
    let key = native_builtin_lowering_key(builtin)?;
    let catalog = stdlib_catalog();
    if let Some(global) = catalog.global_by_lowering_key(&key) {
        return Some(catalog_function_display(&global.name, global.arity));
    }
    let export = catalog.export_by_lowering_key(&key)?;
    Some(export.display.clone())
}

pub(in crate::llvm) fn stdlib_builtin_return_kind(
    builtin: NativeBuiltin,
    arg_count: usize,
) -> Option<NativeScalarKind> {
    let key = native_builtin_lowering_key(builtin)?;
    let catalog = stdlib_catalog();
    if let Some(global) = catalog.global_by_lowering_key(&key) {
        return arity_matches(global.arity, arg_count)
            .then_some(global.return_kind)
            .flatten()
            .map(stdlib_return_kind_to_native);
    }
    let export = catalog.export_by_lowering_key(&key)?;
    arity_matches(export.arity?, arg_count)
        .then_some(export.return_kind)
        .flatten()
        .map(stdlib_return_kind_to_native)
}

pub(super) fn stdlib_module_index(
    module: NativeModule,
    key: NativeStraightlineValue,
) -> Option<NativeStraightlineValue> {
    let NativeStraightlineValue::String { value: key, .. } = key else {
        return None;
    };
    let mut path: Vec<&str> = module.name().split('.').collect();
    path.push(&key);
    let export = stdlib_catalog().export_path(&path)?;
    if export.kind == StdlibExportKind::Module {
        let mut nested = module.name().to_string();
        nested.push('.');
        nested.push_str(&key);
        return Some(NativeStraightlineValue::Module(NativeModule::new(leak_catalog_string(
            nested,
        ))));
    }
    if let Some(lowering_key) = export.lowering_key {
        if let Some(value) = lowering_key_to_value(lowering_key) {
            return Some(value);
        }
    }
    export.const_value.as_ref().and_then(const_value_to_native)
}

pub(super) fn stdlib_export_path_value(path: &[&str]) -> Option<NativeStraightlineValue> {
    let export = stdlib_catalog().export_path(path)?;
    if export.kind == StdlibExportKind::Module {
        return Some(NativeStraightlineValue::Module(NativeModule::new(leak_catalog_string(
            path.join("."),
        ))));
    }
    if let Some(lowering_key) = export.lowering_key
        && let Some(value) = lowering_key_to_value(lowering_key)
    {
        return Some(value);
    }
    export.const_value.as_ref().and_then(const_value_to_native)
}

fn lowering_key_to_value(key: &str) -> Option<NativeStraightlineValue> {
    let builtin = match key {
        "core.print" => NativeBuiltin::Print,
        "core.println" => NativeBuiltin::Println,
        "core.assert" => NativeBuiltin::Assert,
        "core.assert_eq" => NativeBuiltin::AssertEq,
        "core.assert_ne" => NativeBuiltin::AssertNe,
        "core.panic" => NativeBuiltin::Panic,
        "core.chan" => NativeBuiltin::Chan,
        "core.send" => NativeBuiltin::Send,
        "core.recv" => NativeBuiltin::Recv,
        "bytes.to_string_utf8" => NativeBuiltin::BytesToStringUtf8,
        "datetime.add" => NativeBuiltin::DatetimeAdd,
        "datetime.day_of_week" => NativeBuiltin::DatetimeDayOfWeek,
        "datetime.day_of_year" => NativeBuiltin::DatetimeDayOfYear,
        "datetime.format" => NativeBuiltin::DatetimeFormat,
        "datetime.is_weekend" => NativeBuiltin::DatetimeIsWeekend,
        "datetime.now" => NativeBuiltin::DatetimeNow,
        "datetime.sub" => NativeBuiltin::DatetimeSub,
        "env.get_or" => NativeBuiltin::EnvGetOr,
        "encoding.json.parse" => NativeBuiltin::JsonParse,
        "encoding.toml.parse" => NativeBuiltin::TomlParse,
        "encoding.yaml.parse" => NativeBuiltin::YamlParse,
        "fs.exists" => NativeBuiltin::FsExists,
        "fs.read_dir" => NativeBuiltin::FsReadDir,
        "fs.temp_dir" => NativeBuiltin::FsTempDir,
        "io.std.flush" => NativeBuiltin::IoStdFlush,
        "io.std.read_to_string" => NativeBuiltin::IoStdReadToString,
        "io.std.stderr" => NativeBuiltin::IoStdStderr,
        "io.std.stdin" => NativeBuiltin::IoStdStdin,
        "io.std.stdout" => NativeBuiltin::IoStdStdout,
        "io.std.write" => NativeBuiltin::IoStdWrite,
        "io.std.writeln" => NativeBuiltin::IoStdWriteln,
        "iter.chain" => NativeBuiltin::IterChain,
        "iter.chunk" => NativeBuiltin::IterChunk,
        "iter.collect" => NativeBuiltin::IterModuleMethod("collect"),
        "iter.enumerate" => NativeBuiltin::IterEnumerate,
        "iter.filter" => NativeBuiltin::IterFilter,
        "iter.flatten" => NativeBuiltin::IterFlatten,
        "iter.map" => NativeBuiltin::IterMap,
        "iter.next" => NativeBuiltin::IterModuleMethod("next"),
        "iter.range" => NativeBuiltin::IterRange,
        "iter.reduce" => NativeBuiltin::IterReduce,
        "iter.skip" => NativeBuiltin::IterSkip,
        "iter.take" => NativeBuiltin::IterTake,
        "iter.unique" => NativeBuiltin::IterUnique,
        "iter.zip" => NativeBuiltin::IterZip,
        "math.abs" => NativeBuiltin::MathAbs,
        "math.ceil" => NativeBuiltin::MathCeil,
        "math.cos" => NativeBuiltin::MathCos,
        "math.exp" => NativeBuiltin::MathExp,
        "math.floor" => NativeBuiltin::MathFloor,
        "math.max" => NativeBuiltin::MathMax,
        "math.min" => NativeBuiltin::MathMin,
        "math.pow" => NativeBuiltin::MathPow,
        "math.round" => NativeBuiltin::MathRound,
        "math.sin" => NativeBuiltin::MathSin,
        "math.sqrt" => NativeBuiltin::MathSqrt,
        "math.acos" => NativeBuiltin::MathModuleMethod("acos"),
        "math.asin" => NativeBuiltin::MathModuleMethod("asin"),
        "math.atan" => NativeBuiltin::MathModuleMethod("atan"),
        "math.atan2" => NativeBuiltin::MathModuleMethod("atan2"),
        "math.cbrt" => NativeBuiltin::MathModuleMethod("cbrt"),
        "math.clamp" => NativeBuiltin::MathModuleMethod("clamp"),
        "math.cosh" => NativeBuiltin::MathModuleMethod("cosh"),
        "math.fract" => NativeBuiltin::MathModuleMethod("fract"),
        "math.hypot" => NativeBuiltin::MathModuleMethod("hypot"),
        "math.is_inf" => NativeBuiltin::MathModuleMethod("is_inf"),
        "math.is_nan" => NativeBuiltin::MathModuleMethod("is_nan"),
        "math.log" => NativeBuiltin::MathModuleMethod("log"),
        "math.log10" => NativeBuiltin::MathModuleMethod("log10"),
        "math.log2" => NativeBuiltin::MathModuleMethod("log2"),
        "math.sign" => NativeBuiltin::MathModuleMethod("sign"),
        "math.sinh" => NativeBuiltin::MathModuleMethod("sinh"),
        "math.tan" => NativeBuiltin::MathModuleMethod("tan"),
        "math.tanh" => NativeBuiltin::MathModuleMethod("tanh"),
        "math.to_float" => NativeBuiltin::MathModuleMethod("to_float"),
        "math.to_int" => NativeBuiltin::MathModuleMethod("to_int"),
        "math.trunc" => NativeBuiltin::MathModuleMethod("trunc"),
        "net.socket.addr" => NativeBuiltin::SocketAddr,
        "net.tcp.close" => NativeBuiltin::TcpClose,
        "net.tcp.connect" => NativeBuiltin::TcpConnect,
        "net.tcp.read" => NativeBuiltin::TcpRead,
        "net.tcp.write" => NativeBuiltin::TcpWrite,
        "os.arch" => NativeBuiltin::OsArch,
        "os.clock" => NativeBuiltin::OsClock,
        "os.epoch" => NativeBuiltin::OsEpoch,
        "os.hostname" => NativeBuiltin::OsHostname,
        "os.os" => NativeBuiltin::OsName,
        "path.sep" => NativeBuiltin::PathSep,
        "process.cwd" => NativeBuiltin::ProcessCwd,
        "stream.chain" => NativeBuiltin::IterChain,
        "stream.collect" => NativeBuiltin::StreamCollect,
        "stream.filter" => NativeBuiltin::IterFilter,
        "stream.from_list" => NativeBuiltin::StreamFromList,
        "stream.map" => NativeBuiltin::IterMap,
        "stream.range" => NativeBuiltin::IterRange,
        "stream.skip" => NativeBuiltin::IterSkip,
        "stream.take" => NativeBuiltin::IterTake,
        "string.len" => NativeBuiltin::StringLen,
        key if key.starts_with("string.") => {
            NativeBuiltin::StringModuleMethod(leak_catalog_string(key["string.".len()..].to_string()))
        }
        "time.now" => NativeBuiltin::TimeNow,
        "time.since" => NativeBuiltin::TimeSince,
        "time.sleep" => NativeBuiltin::TimeSleep,
        "math.e" | "math.epsilon" | "math.inf" | "math.max_float" | "math.max_int" | "math.min_int" | "math.nan"
        | "math.pi" => return None,
        _ => return None,
    };
    Some(NativeStraightlineValue::Builtin(builtin))
}

fn native_builtin_lowering_key(builtin: NativeBuiltin) -> Option<String> {
    let key = match builtin {
        NativeBuiltin::Print => "core.print",
        NativeBuiltin::Println => "core.println",
        NativeBuiltin::Assert => "core.assert",
        NativeBuiltin::AssertEq => "core.assert_eq",
        NativeBuiltin::AssertNe => "core.assert_ne",
        NativeBuiltin::Panic => "core.panic",
        NativeBuiltin::Chan => "core.chan",
        NativeBuiltin::Send => "core.send",
        NativeBuiltin::Recv => "core.recv",
        NativeBuiltin::BytesToStringUtf8 => "bytes.to_string_utf8",
        NativeBuiltin::DatetimeAdd => "datetime.add",
        NativeBuiltin::DatetimeDayOfWeek => "datetime.day_of_week",
        NativeBuiltin::DatetimeDayOfYear => "datetime.day_of_year",
        NativeBuiltin::DatetimeFormat => "datetime.format",
        NativeBuiltin::DatetimeIsWeekend => "datetime.is_weekend",
        NativeBuiltin::DatetimeNow => "datetime.now",
        NativeBuiltin::DatetimeSub => "datetime.sub",
        NativeBuiltin::EnvGetOr => "env.get_or",
        NativeBuiltin::JsonParse => "encoding.json.parse",
        NativeBuiltin::TomlParse => "encoding.toml.parse",
        NativeBuiltin::YamlParse => "encoding.yaml.parse",
        NativeBuiltin::FsExists => "fs.exists",
        NativeBuiltin::FsReadDir => "fs.read_dir",
        NativeBuiltin::FsTempDir => "fs.temp_dir",
        NativeBuiltin::IoStdFlush => "io.std.flush",
        NativeBuiltin::IoStdReadToString => "io.std.read_to_string",
        NativeBuiltin::IoStdStderr => "io.std.stderr",
        NativeBuiltin::IoStdStdin => "io.std.stdin",
        NativeBuiltin::IoStdStdout => "io.std.stdout",
        NativeBuiltin::IoStdWrite => "io.std.write",
        NativeBuiltin::IoStdWriteln => "io.std.writeln",
        NativeBuiltin::IterChain => "iter.chain",
        NativeBuiltin::IterChunk => "iter.chunk",
        NativeBuiltin::IterEnumerate => "iter.enumerate",
        NativeBuiltin::IterFilter => "iter.filter",
        NativeBuiltin::IterFlatten => "iter.flatten",
        NativeBuiltin::IterMap => "iter.map",
        NativeBuiltin::IterRange => "iter.range",
        NativeBuiltin::IterReduce => "iter.reduce",
        NativeBuiltin::IterSkip => "iter.skip",
        NativeBuiltin::IterTake => "iter.take",
        NativeBuiltin::IterUnique => "iter.unique",
        NativeBuiltin::IterZip => "iter.zip",
        NativeBuiltin::IterModuleMethod(method) => return Some(format!("iter.{method}")),
        NativeBuiltin::MathAbs => "math.abs",
        NativeBuiltin::MathCeil => "math.ceil",
        NativeBuiltin::MathCos => "math.cos",
        NativeBuiltin::MathExp => "math.exp",
        NativeBuiltin::MathFloor => "math.floor",
        NativeBuiltin::MathMax => "math.max",
        NativeBuiltin::MathMin => "math.min",
        NativeBuiltin::MathPow => "math.pow",
        NativeBuiltin::MathRound => "math.round",
        NativeBuiltin::MathSin => "math.sin",
        NativeBuiltin::MathSqrt => "math.sqrt",
        NativeBuiltin::MathModuleMethod(method) => return Some(format!("math.{method}")),
        NativeBuiltin::SocketAddr => "net.socket.addr",
        NativeBuiltin::TcpClose => "net.tcp.close",
        NativeBuiltin::TcpConnect => "net.tcp.connect",
        NativeBuiltin::TcpRead => "net.tcp.read",
        NativeBuiltin::TcpWrite => "net.tcp.write",
        NativeBuiltin::OsArch => "os.arch",
        NativeBuiltin::OsClock => "os.clock",
        NativeBuiltin::OsEpoch => "os.epoch",
        NativeBuiltin::OsHostname => "os.hostname",
        NativeBuiltin::OsName => "os.os",
        NativeBuiltin::PathSep => "path.sep",
        NativeBuiltin::ProcessCwd => "process.cwd",
        NativeBuiltin::StreamCollect => "stream.collect",
        NativeBuiltin::StreamFromList => "stream.from_list",
        NativeBuiltin::StringLen => "string.len",
        NativeBuiltin::StringModuleMethod(method) => return Some(format!("string.{method}")),
        NativeBuiltin::TimeNow => "time.now",
        NativeBuiltin::TimeSince => "time.since",
        NativeBuiltin::TimeSleep => "time.sleep",
        _ => return None,
    };
    Some(key.to_string())
}

fn catalog_function_display(name: &str, arity: StdlibArity) -> String {
    match arity {
        StdlibArity::Fixed(value) => format!("<native fn {name}({value} args)>"),
        StdlibArity::Variadic => format!("<native fn {name}(...)>"),
    }
}

fn arity_matches(arity: StdlibArity, arg_count: usize) -> bool {
    match arity {
        StdlibArity::Fixed(expected) => usize::from(expected) == arg_count,
        StdlibArity::Variadic => true,
    }
}

fn stdlib_return_kind_to_native(kind: StdlibReturnKind) -> NativeScalarKind {
    match kind {
        StdlibReturnKind::Nil => NativeScalarKind::Nil,
        StdlibReturnKind::Bool => NativeScalarKind::Bool,
        StdlibReturnKind::Int | StdlibReturnKind::RuntimeValue => NativeScalarKind::I64,
        StdlibReturnKind::Float => NativeScalarKind::F64,
        StdlibReturnKind::String => NativeScalarKind::StrPtr,
    }
}

fn const_value_to_native(value: &StdlibConstValue) -> Option<NativeStraightlineValue> {
    match value {
        StdlibConstValue::Nil => Some(NativeStraightlineValue::Nil),
        StdlibConstValue::Bool(value) => Some(NativeStraightlineValue::Bool((*value as u8).to_string())),
        StdlibConstValue::Int(value) => Some(NativeStraightlineValue::I64(value.to_string())),
        StdlibConstValue::Float(value) => Some(NativeStraightlineValue::F64(llvm_float_literal(*value))),
        StdlibConstValue::String(value) => Some(NativeStraightlineValue::String {
            symbol: String::new(),
            value: value.clone(),
            len: value.chars().count(),
            key_kind: native_runtime_string_key_kind(value),
        }),
    }
}

fn leak_catalog_string(value: String) -> &'static str {
    static INTERNED: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();

    let table = INTERNED.get_or_init(|| Mutex::new(HashMap::new()));
    let mut table = table.lock().expect("LLVM stdlib catalog intern table poisoned");
    if let Some(interned) = table.get(value.as_str()).copied() {
        return interned;
    }
    let interned = Box::leak(value.clone().into_boxed_str());
    table.insert(value, interned);
    interned
}
