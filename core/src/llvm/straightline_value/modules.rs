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
        (NativeModule::Io, "stdout_write") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IoStdoutWrite)),
        (NativeModule::Io, "stdout_writeln") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IoStdoutWriteln)),
        (NativeModule::Io, "stderr_write") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IoStderrWrite)),
        (NativeModule::Io, "stdout_flush") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IoStdoutFlush)),
        (NativeModule::Io, "read") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IoRead)),
        (NativeModule::Json, "parse") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::JsonParse)),
        (NativeModule::Math, "pi") => Some(NativeStraightlineValue::F64(llvm_float_literal(std::f64::consts::PI))),
        (NativeModule::Math, "e") => Some(NativeStraightlineValue::F64(llvm_float_literal(std::f64::consts::E))),
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
        (NativeModule::Fib, "iterative") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::FibIterative)),
        (NativeModule::Greetings, "message") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::GreetingsMessage)),
        (NativeModule::Mathlib, "double") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MathlibDouble)),
        (NativeModule::Map, "delete") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MapDelete)),
        (NativeModule::Map, "set") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MapSet)),
        (NativeModule::Map, "mutate") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::MapMutate)),
        (NativeModule::Toml, "parse") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::TomlParse)),
        (NativeModule::Time, "now") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::TimeNow)),
        (NativeModule::Time, "sleep") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::TimeSleep)),
        (NativeModule::Time, "since") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::TimeSince)),
        (NativeModule::Tcp, "connect") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::TcpConnect)),
        (NativeModule::Tcp, "write") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::TcpWrite)),
        (NativeModule::Tcp, "read") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::TcpRead)),
        (NativeModule::Tcp, "close") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::TcpClose)),
        (NativeModule::Stream, "from_list") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::StreamFromList)),
        (NativeModule::Stream, "collect") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::StreamCollect)),
        (NativeModule::Stream, "range") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterRange)),
        (NativeModule::Stream, "map") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterMap)),
        (NativeModule::Stream, "filter") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterFilter)),
        (NativeModule::Stream, "take") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterTake)),
        (NativeModule::Stream, "skip") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterSkip)),
        (NativeModule::Stream, "chain") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::IterChain)),
        (NativeModule::String, "len") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::StringLen)),
        (NativeModule::Yaml, "parse") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::YamlParse)),
        _ => None,
    }
}
