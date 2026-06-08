use anyhow::{Result, anyhow};
use chrono::Datelike;
use lk_core::{
    module::{ModuleProvider, ModuleRegistry, RuntimeNativeExport, runtime_export_from_plain_native_entries},
    val::RuntimeVal,
    vm::{NativeArgs, NativeRuntime, RuntimeExport},
};

pub mod runtime_native {
    pub use lk_stdlib_common::runtime_native::*;
}

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

impl ModuleProvider for DateTimeModule {
    fn name(&self) -> &str {
        "datetime"
    }

    fn description(&self) -> &str {
        "Date and time functions"
    }

    fn register(&self, _registry: &mut lk_core::module::ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport::plain("now", now, 0),
                RuntimeNativeExport::plain("format", format, 2),
                RuntimeNativeExport::plain("parse", parse, 2),
                RuntimeNativeExport::plain("add", add_seconds, 2),
                RuntimeNativeExport::plain("sub", sub_seconds, 2),
                RuntimeNativeExport::plain("day_of_week", day_of_week, 1),
                RuntimeNativeExport::plain("day_of_year", day_of_year, 1),
                RuntimeNativeExport::plain("is_weekend", is_weekend, 1),
            ],
            &[],
        ))
    }
}

pub fn register(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("datetime", Box::new(DateTimeModule::new()))
}

fn expect_arity(args: NativeArgs<'_>, name: &str, arity: usize) -> Result<()> {
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

fn now(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    if args.len() != 0 {
        return Err(anyhow!("now() takes no arguments"));
    }
    Ok(RuntimeVal::Int(chrono::Utc::now().timestamp_micros()))
}

fn format(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, "format", 2)?;
    let timestamp = timestamp_arg(args.get(0).expect("checked arity"), "format")?;
    let format = runtime_string_arg(args.get(1).expect("checked arity"), runtime.heap(), "format")?;
    let formatted = utc_datetime(timestamp)?.format(format.as_ref()).to_string();
    Ok(runtime_string_value(&formatted, runtime.heap_mut()))
}

fn parse(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, "parse", 2)?;
    let datetime = runtime_string_arg(args.get(0).expect("checked arity"), runtime.heap(), "parse")?;
    let format = runtime_string_arg(args.get(1).expect("checked arity"), runtime.heap(), "parse")?;
    let naive = chrono::NaiveDateTime::parse_from_str(datetime.as_ref(), format.as_ref())
        .map_err(|err| anyhow!("failed to parse datetime: {err}"))?;
    let dt = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(naive, chrono::Utc);
    Ok(RuntimeVal::Int(dt.timestamp()))
}

fn add_seconds(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, "add", 2)?;
    let timestamp = timestamp_arg(args.get(0).expect("checked arity"), "add")?;
    let seconds = int_arg(args.get(1).expect("checked arity"), "add")?;
    Ok(RuntimeVal::Int(timestamp + seconds))
}

fn sub_seconds(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, "sub", 2)?;
    let timestamp = timestamp_arg(args.get(0).expect("checked arity"), "sub")?;
    let seconds = int_arg(args.get(1).expect("checked arity"), "sub")?;
    Ok(RuntimeVal::Int(timestamp - seconds))
}

fn day_of_week(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
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

fn day_of_year(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, "day_of_year", 1)?;
    let timestamp = timestamp_arg(args.get(0).expect("checked arity"), "day_of_year")?;
    Ok(RuntimeVal::Int(i64::from(utc_datetime(timestamp)?.ordinal())))
}

fn is_weekend(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    expect_arity(args, "is_weekend", 1)?;
    let timestamp = timestamp_arg(args.get(0).expect("checked arity"), "is_weekend")?;
    let is_weekend = matches!(
        utc_datetime(timestamp)?.weekday(),
        chrono::Weekday::Sat | chrono::Weekday::Sun
    );
    Ok(RuntimeVal::Bool(is_weekend))
}
