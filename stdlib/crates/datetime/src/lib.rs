use anyhow::{Result, anyhow};
use chrono::Datelike;
use lk_core::{
    val::RuntimeVal,
    vm::{NativeArgs, NativeRuntime},
};

pub mod runtime_native {
    pub use lk_stdlib_common::runtime_native::*;
}

use crate::runtime_native::{runtime_string_arg, runtime_string_value};

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "datetime", docs = "Date and time functions")]
pub struct DateTimeModule;

#[lk_stdlib_common::stdlib_exports(module = "datetime")]
impl DateTimeModule {
    #[stdlib_export(name = "now", params(), returns = Int)]
    fn now(_args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        Ok(RuntimeVal::Int(chrono::Utc::now().timestamp()))
    }

    #[stdlib_export(name = "format", params(timestamp: Int, format: String), returns = String)]
    fn format(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let timestamp = timestamp_arg(args.get(0).expect("checked arity"), "format")?;
        let format = runtime_string_arg(args.get(1).expect("checked arity"), runtime.heap(), "format")?;
        let formatted = utc_datetime(timestamp)?.format(format.as_ref()).to_string();
        Ok(runtime_string_value(&formatted, runtime.heap_mut()))
    }

    #[stdlib_export(name = "parse", params(value: String, format: String), returns = Int)]
    fn parse(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let datetime = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "parse")?;
        let format = runtime_string_arg(args.get(1).expect("checked arity"), runtime.heap(), "parse")?;
        let naive = chrono::NaiveDateTime::parse_from_str(datetime.as_ref(), format.as_ref())
            .map_err(|err| anyhow!("failed to parse datetime: {err}"))?;
        let dt = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(naive, chrono::Utc);
        Ok(RuntimeVal::Int(dt.timestamp()))
    }

    #[stdlib_export(name = "add", params(timestamp: Int, seconds: Int), returns = Int)]
    fn add(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let timestamp = timestamp_arg(args.get(0).expect("checked arity"), "add")?;
        let seconds = int_arg(args.get(1).expect("checked arity"), "add")?;
        Ok(RuntimeVal::Int(timestamp + seconds))
    }

    #[stdlib_export(name = "sub", params(timestamp: Int, seconds: Int), returns = Int)]
    fn sub(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let timestamp = timestamp_arg(args.get(0).expect("checked arity"), "sub")?;
        let seconds = int_arg(args.get(1).expect("checked arity"), "sub")?;
        Ok(RuntimeVal::Int(timestamp - seconds))
    }

    #[stdlib_export(name = "day_of_week", params(timestamp: Int), returns = Int)]
    fn day_of_week(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let timestamp = timestamp_arg(args.get(0).expect("checked arity"), "day_of_week")?;
        let day = match utc_datetime(timestamp)?.weekday() {
            chrono::Weekday::Sun => 0,
            chrono::Weekday::Mon => 1,
            chrono::Weekday::Tue => 2,
            chrono::Weekday::Wed => 3,
            chrono::Weekday::Thu => 4,
            chrono::Weekday::Fri => 5,
            chrono::Weekday::Sat => 6,
        };
        Ok(RuntimeVal::Int(day))
    }

    #[stdlib_export(name = "day_of_year", params(timestamp: Int), returns = Int)]
    fn day_of_year(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let timestamp = timestamp_arg(args.get(0).expect("checked arity"), "day_of_year")?;
        Ok(RuntimeVal::Int(i64::from(utc_datetime(timestamp)?.ordinal())))
    }

    #[stdlib_export(name = "is_weekend", params(timestamp: Int), returns = Bool)]
    fn is_weekend(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let timestamp = timestamp_arg(args.get(0).expect("checked arity"), "is_weekend")?;
        let is_weekend = matches!(
            utc_datetime(timestamp)?.weekday(),
            chrono::Weekday::Sat | chrono::Weekday::Sun
        );
        Ok(RuntimeVal::Bool(is_weekend))
    }
}

fn int_arg(value: &RuntimeVal, name: &str) -> Result<i64> {
    match value {
        RuntimeVal::Int(value) => Ok(*value),
        other => Err(anyhow!("{name} argument must be an integer, got {:?}", other.kind())),
    }
}

fn timestamp_arg(value: &RuntimeVal, name: &str) -> Result<i64> {
    match value {
        RuntimeVal::Int(value) => Ok(*value),
        other => Err(anyhow!(
            "{name} argument must be an integer timestamp, got {:?}",
            other.kind()
        )),
    }
}

fn utc_datetime(timestamp: i64) -> Result<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp, 0).ok_or_else(|| anyhow!("invalid timestamp"))
}
