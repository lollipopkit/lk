use anyhow::{Result, anyhow};
use chrono::Datelike;
use lk_core::{
    module::{Module, RuntimeNativeExport32, runtime_export_from_plain_native_entries},
    val::RuntimeVal,
    vm::{NativeArgs32, NativeRuntime32, RuntimeExport32},
};

use crate::runtime_native::{runtime_string_arg, runtime_string_value};

#[derive(Debug)]
pub struct DateTimeModule;

impl Default for DateTimeModule {
    fn default() -> Self {
        Self::new()
    }
}

impl DateTimeModule {
    pub fn new() -> Self {
        Self
    }
}

impl Module for DateTimeModule {
    fn name(&self) -> &str {
        "datetime"
    }

    fn description(&self) -> &str {
        "Date and time functions"
    }

    fn register(&self, _registry: &mut lk_core::module::ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport32> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport32::plain("now", now32, 0),
                RuntimeNativeExport32::plain("format", format32, 2),
                RuntimeNativeExport32::plain("parse", parse32, 2),
                RuntimeNativeExport32::plain("add", add_seconds32, 2),
                RuntimeNativeExport32::plain("sub", sub_seconds32, 2),
                RuntimeNativeExport32::plain("day_of_week", day_of_week32, 1),
                RuntimeNativeExport32::plain("day_of_year", day_of_year32, 1),
                RuntimeNativeExport32::plain("is_weekend", is_weekend32, 1),
            ],
            &[],
        ))
    }
}

fn expect_arity(args: NativeArgs32<'_>, name: &str, arity: usize) -> Result<()> {
    if args.len() == arity {
        return Ok(());
    }
    Err(anyhow!("{name}() takes exactly {arity} arguments"))
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

fn now32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    if args.len() != 0 {
        return Err(anyhow!("now() takes no arguments"));
    }
    Ok(RuntimeVal::Int(chrono::Utc::now().timestamp_micros()))
}

fn format32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, "format", 2)?;
    let timestamp = timestamp_arg(args.get(0).expect("checked arity"), "format")?;
    let format = runtime_string_arg(args.get(1).expect("checked arity"), runtime.heap(), "format")?;
    let formatted = utc_datetime(timestamp)?.format(format.as_ref()).to_string();
    Ok(runtime_string_value(&formatted, runtime.heap_mut()))
}

fn parse32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, "parse", 2)?;
    let datetime = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "parse")?;
    let format = runtime_string_arg(args.get(1).expect("checked arity"), runtime.heap(), "parse")?;
    let naive = chrono::NaiveDateTime::parse_from_str(datetime.as_ref(), format.as_ref())
        .map_err(|err| anyhow!("failed to parse datetime: {err}"))?;
    let dt = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(naive, chrono::Utc);
    Ok(RuntimeVal::Int(dt.timestamp()))
}

fn add_seconds32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, "add", 2)?;
    let timestamp = timestamp_arg(args.get(0).expect("checked arity"), "add")?;
    let seconds = int_arg(args.get(1).expect("checked arity"), "add")?;
    Ok(RuntimeVal::Int(timestamp + seconds))
}

fn sub_seconds32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, "sub", 2)?;
    let timestamp = timestamp_arg(args.get(0).expect("checked arity"), "sub")?;
    let seconds = int_arg(args.get(1).expect("checked arity"), "sub")?;
    Ok(RuntimeVal::Int(timestamp - seconds))
}

fn day_of_week32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, "day_of_week", 1)?;
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

fn day_of_year32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, "day_of_year", 1)?;
    let timestamp = timestamp_arg(args.get(0).expect("checked arity"), "day_of_year")?;
    Ok(RuntimeVal::Int(i64::from(utc_datetime(timestamp)?.ordinal())))
}

fn is_weekend32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, "is_weekend", 1)?;
    let timestamp = timestamp_arg(args.get(0).expect("checked arity"), "is_weekend")?;
    let is_weekend = matches!(
        utc_datetime(timestamp)?.weekday(),
        chrono::Weekday::Sat | chrono::Weekday::Sun
    );
    Ok(RuntimeVal::Bool(is_weekend))
}
