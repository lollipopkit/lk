use crate::llvm::ir_text::llvm_float_literal;

use super::{NativeBuiltin, NativeModule, NativeStraightlineValue};

pub(super) fn native_static_module_index(
    module: NativeModule,
    key: NativeStraightlineValue,
) -> Option<NativeStraightlineValue> {
    let NativeStraightlineValue::String { value: key, .. } = key else {
        return None;
    };
    match (module, key.as_str()) {
        (NativeModule::Datetime, "now") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::DatetimeNow)),
        (NativeModule::Datetime, "format") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::DatetimeFormat)),
        (NativeModule::Datetime, "add") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::DatetimeAdd)),
        (NativeModule::Datetime, "sub") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::DatetimeSub)),
        (NativeModule::Datetime, "day_of_week") => {
            Some(NativeStraightlineValue::Builtin(NativeBuiltin::DatetimeDayOfWeek))
        }
        (NativeModule::Datetime, "day_of_year") => {
            Some(NativeStraightlineValue::Builtin(NativeBuiltin::DatetimeDayOfYear))
        }
        (NativeModule::Datetime, "is_weekend") => {
            Some(NativeStraightlineValue::Builtin(NativeBuiltin::DatetimeIsWeekend))
        }
        (NativeModule::Os, "clock") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::OsClock)),
        (NativeModule::Os, "epoch") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::OsEpoch)),
        (NativeModule::Os, "hostname") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::OsHostname)),
        (NativeModule::Os, "arch") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::OsArch)),
        (NativeModule::Os, "os") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::OsName)),
        (NativeModule::Os, "dir_current") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::OsDirCurrent)),
        (NativeModule::Os, "dir_temp") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::OsDirTemp)),
        (NativeModule::Os, "dir_list") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::OsDirList)),
        (NativeModule::Os, "env") => Some(NativeStraightlineValue::Module(NativeModule::OsEnv)),
        (NativeModule::OsEnv, "get") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::OsEnvGet)),
        (NativeModule::OsEnv, "set") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::OsEnvSet)),
        (NativeModule::OsEnv, "unset") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::OsEnvUnset)),
        (NativeModule::Iter, "range") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterRange)),
        (NativeModule::Iter, "map") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterMap)),
        (NativeModule::Iter, "filter") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterFilter)),
        (NativeModule::Iter, "reduce") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterReduce)),
        (NativeModule::Iter, "take") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterTake)),
        (NativeModule::Iter, "skip") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterSkip)),
        (NativeModule::Iter, "chain") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterChain)),
        (NativeModule::Iter, "flatten") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterFlatten)),
        (NativeModule::Iter, "unique") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterUnique)),
        (NativeModule::Iter, "chunk") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterChunk)),
        (NativeModule::Iter, "enumerate") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterEnumerate)),
        (NativeModule::Iter, "zip") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterZip)),
        (NativeModule::Iter, "next") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterModuleMethod(
            "next",
        ))),
        (NativeModule::Iter, "collect") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterModuleMethod(
            "collect",
        ))),
        (NativeModule::Json, "parse") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::JsonParse)),
        (NativeModule::Math, "pi") => Some(NativeStraightlineValue::F64(llvm_float_literal(std::f64::consts::PI))),
        (NativeModule::Math, "e") => Some(NativeStraightlineValue::F64(llvm_float_literal(std::f64::consts::E))),
        (NativeModule::Math, "inf") => Some(NativeStraightlineValue::F64(llvm_float_literal(f64::INFINITY))),
        (NativeModule::Math, "nan") => Some(NativeStraightlineValue::F64(llvm_float_literal(f64::NAN))),
        (NativeModule::Math, "max_int") => Some(NativeStraightlineValue::I64(i64::MAX.to_string())),
        (NativeModule::Math, "min_int") => Some(NativeStraightlineValue::I64(i64::MIN.to_string())),
        (NativeModule::Math, "max_float") => Some(NativeStraightlineValue::F64(llvm_float_literal(f64::MAX))),
        (NativeModule::Math, "epsilon") => Some(NativeStraightlineValue::F64(llvm_float_literal(f64::EPSILON))),
        (NativeModule::Math, "abs") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathAbs)),
        (NativeModule::Math, "sqrt") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathSqrt)),
        (NativeModule::Math, "floor") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathFloor)),
        (NativeModule::Math, "ceil") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathCeil)),
        (NativeModule::Math, "round") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathRound)),
        (NativeModule::Math, "min") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathMin)),
        (NativeModule::Math, "max") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathMax)),
        (NativeModule::Math, "pow") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathPow)),
        (NativeModule::Math, "exp") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathExp)),
        (NativeModule::Math, "sin") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathSin)),
        (NativeModule::Math, "cos") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathCos)),
        (NativeModule::Math, "tan") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathModuleMethod("tan"))),
        (NativeModule::Math, "asin") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathModuleMethod(
            "asin",
        ))),
        (NativeModule::Math, "acos") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathModuleMethod(
            "acos",
        ))),
        (NativeModule::Math, "atan") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathModuleMethod(
            "atan",
        ))),
        (NativeModule::Math, "atan2") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathModuleMethod(
            "atan2",
        ))),
        (NativeModule::Math, "log") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathModuleMethod("log"))),
        (NativeModule::Math, "log10") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathModuleMethod(
            "log10",
        ))),
        (NativeModule::Math, "log2") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathModuleMethod(
            "log2",
        ))),
        (NativeModule::Math, "clamp") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathModuleMethod(
            "clamp",
        ))),
        (NativeModule::Math, "hypot") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathModuleMethod(
            "hypot",
        ))),
        (NativeModule::Math, "cbrt") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathModuleMethod(
            "cbrt",
        ))),
        (NativeModule::Math, "sinh") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathModuleMethod(
            "sinh",
        ))),
        (NativeModule::Math, "cosh") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathModuleMethod(
            "cosh",
        ))),
        (NativeModule::Math, "tanh") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathModuleMethod(
            "tanh",
        ))),
        (NativeModule::Math, "trunc") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathModuleMethod(
            "trunc",
        ))),
        (NativeModule::Math, "fract") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathModuleMethod(
            "fract",
        ))),
        (NativeModule::Math, "sign") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathModuleMethod(
            "sign",
        ))),
        (NativeModule::Math, "to_int") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathModuleMethod(
            "to_int",
        ))),
        (NativeModule::Math, "to_float") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathModuleMethod(
            "to_float",
        ))),
        (NativeModule::Math, "is_nan") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathModuleMethod(
            "is_nan",
        ))),
        (NativeModule::Math, "is_inf") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathModuleMethod(
            "is_inf",
        ))),
        (NativeModule::Fib, "iterative") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::FibIterative)),
        (NativeModule::Greetings, "message") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::GreetingsMessage)),
        (NativeModule::Mathlib, "double") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathlibDouble)),
        (NativeModule::Map, "len") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MapModuleMethod("len"))),
        (NativeModule::Map, "keys") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MapModuleMethod("keys"))),
        (NativeModule::Map, "values") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MapModuleMethod(
            "values",
        ))),
        (NativeModule::Map, "has") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MapModuleMethod("has"))),
        (NativeModule::Map, "get") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MapModuleMethod("get"))),
        (NativeModule::Map, "delete") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MapDelete)),
        (NativeModule::Map, "set") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MapSet)),
        (NativeModule::Map, "mutate") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MapMutate)),
        (NativeModule::Toml, "parse") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::TomlParse)),
        (NativeModule::Time, "now") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::TimeNow)),
        (NativeModule::Time, "sleep") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::TimeSleep)),
        (NativeModule::Time, "since") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::TimeSince)),
        (NativeModule::Stream, "from_list") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::StreamFromList)),
        (NativeModule::Stream, "collect") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::StreamCollect)),
        (NativeModule::Stream, "range") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterRange)),
        (NativeModule::Stream, "map") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterMap)),
        (NativeModule::Stream, "filter") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterFilter)),
        (NativeModule::Stream, "take") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterTake)),
        (NativeModule::Stream, "skip") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterSkip)),
        (NativeModule::Stream, "chain") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterChain)),
        (NativeModule::String, "len") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::StringLen)),
        (NativeModule::String, "lower") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::StringModuleMethod(
            "lower",
        ))),
        (NativeModule::String, "upper") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::StringModuleMethod(
            "upper",
        ))),
        (NativeModule::String, "trim") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::StringModuleMethod(
            "trim",
        ))),
        (NativeModule::String, "starts_with") => Some(NativeStraightlineValue::Builtin(
            NativeBuiltin::StringModuleMethod("starts_with"),
        )),
        (NativeModule::String, "ends_with") => Some(NativeStraightlineValue::Builtin(
            NativeBuiltin::StringModuleMethod("ends_with"),
        )),
        (NativeModule::String, "contains") => Some(NativeStraightlineValue::Builtin(
            NativeBuiltin::StringModuleMethod("contains"),
        )),
        (NativeModule::String, "replace") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::StringModuleMethod(
            "replace",
        ))),
        (NativeModule::String, "substring") => Some(NativeStraightlineValue::Builtin(
            NativeBuiltin::StringModuleMethod("substring"),
        )),
        (NativeModule::String, "reverse") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::StringModuleMethod(
            "reverse",
        ))),
        (NativeModule::String, "repeat") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::StringModuleMethod(
            "repeat",
        ))),
        (NativeModule::String, "char") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::StringModuleMethod(
            "char",
        ))),
        (NativeModule::String, "byte") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::StringModuleMethod(
            "byte",
        ))),
        (NativeModule::String, "chars") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::StringModuleMethod(
            "chars",
        ))),
        (NativeModule::String, "find") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::StringModuleMethod(
            "find",
        ))),
        (NativeModule::String, "is_empty") => Some(NativeStraightlineValue::Builtin(
            NativeBuiltin::StringModuleMethod("is_empty"),
        )),
        (NativeModule::String, "split") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::StringModuleMethod(
            "split",
        ))),
        (NativeModule::String, "join") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::StringModuleMethod(
            "join",
        ))),
        (NativeModule::String, "format") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::StringModuleMethod(
            "format",
        ))),
        (NativeModule::String, "strip") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::StringModuleMethod(
            "strip",
        ))),
        (NativeModule::String, "strip_prefix") => Some(NativeStraightlineValue::Builtin(
            NativeBuiltin::StringModuleMethod("strip_prefix"),
        )),
        (NativeModule::String, "strip_suffix") => Some(NativeStraightlineValue::Builtin(
            NativeBuiltin::StringModuleMethod("strip_suffix"),
        )),
        (NativeModule::String, "count") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::StringModuleMethod(
            "count",
        ))),
        (NativeModule::String, "pad_left") => Some(NativeStraightlineValue::Builtin(
            NativeBuiltin::StringModuleMethod("pad_left"),
        )),
        (NativeModule::String, "pad_right") => Some(NativeStraightlineValue::Builtin(
            NativeBuiltin::StringModuleMethod("pad_right"),
        )),
        (NativeModule::String, "to_int") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::StringModuleMethod(
            "to_int",
        ))),
        (NativeModule::String, "to_float") => Some(NativeStraightlineValue::Builtin(
            NativeBuiltin::StringModuleMethod("to_float"),
        )),
        (NativeModule::String, "title") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::StringModuleMethod(
            "title",
        ))),
        (NativeModule::String, "capitalize") => Some(NativeStraightlineValue::Builtin(
            NativeBuiltin::StringModuleMethod("capitalize"),
        )),
        (NativeModule::List, "concat") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::ListConcat)),
        (NativeModule::List, "contains") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::ListContains)),
        (NativeModule::List, "first") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::ListFirst)),
        (NativeModule::List, "get") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::ListGet)),
        (NativeModule::List, "index_of") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::ListIndexOf)),
        (NativeModule::List, "insert") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::ListInsert)),
        (NativeModule::List, "is_empty") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::ListIsEmpty)),
        (NativeModule::List, "join") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::ListJoin)),
        (NativeModule::List, "last") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::ListLast)),
        (NativeModule::List, "len") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::ListLen)),
        (NativeModule::List, "pop") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::ListPop)),
        (NativeModule::List, "push") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::ListPush)),
        (NativeModule::List, "remove_at") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::ListRemoveAt)),
        (NativeModule::List, "reverse") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::ListReverse)),
        (NativeModule::List, "set") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::ListSet)),
        (NativeModule::List, "slice") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::ListSlice)),
        (NativeModule::List, "sort") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::ListSort)),
        (NativeModule::Yaml, "parse") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::YamlParse)),
        _ => None,
    }
}
