use lk_stdlib::{StdlibConstValue, StdlibExportKind, stdlib_catalog};

use crate::llvm::{
    ir_text::llvm_float_literal,
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

fn lowering_key_to_value(key: &str) -> Option<NativeStraightlineValue> {
    let builtin = match key {
        "core.print" => NativeBuiltin::Print,
        "core.println" => NativeBuiltin::Println,
        "core.panic" => NativeBuiltin::Panic,
        "core.chan" => NativeBuiltin::Chan,
        "core.send" => NativeBuiltin::Send,
        "core.recv" => NativeBuiltin::Recv,
        "datetime.add" => NativeBuiltin::DatetimeAdd,
        "datetime.day_of_week" => NativeBuiltin::DatetimeDayOfWeek,
        "datetime.day_of_year" => NativeBuiltin::DatetimeDayOfYear,
        "datetime.format" => NativeBuiltin::DatetimeFormat,
        "datetime.is_weekend" => NativeBuiltin::DatetimeIsWeekend,
        "datetime.now" => NativeBuiltin::DatetimeNow,
        "datetime.sub" => NativeBuiltin::DatetimeSub,
        "encoding.json.parse" => NativeBuiltin::JsonParse,
        "encoding.toml.parse" => NativeBuiltin::TomlParse,
        "encoding.yaml.parse" => NativeBuiltin::YamlParse,
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
        "os.arch" => NativeBuiltin::OsArch,
        "os.clock" => NativeBuiltin::OsClock,
        "os.epoch" => NativeBuiltin::OsEpoch,
        "os.hostname" => NativeBuiltin::OsHostname,
        "os.os" => NativeBuiltin::OsName,
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
    Box::leak(value.into_boxed_str())
}
