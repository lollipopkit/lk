use anyhow::{Result, anyhow};
use chrono::Datelike;
use lk_core::{
    module::Module,
    val::{RuntimeVal, Val},
    vm::{NativeArgs32, NativeFunction32, NativeRuntime32},
};
use std::collections::HashMap;

use crate::runtime_native::{runtime_string_arg, runtime_string_value};

#[derive(Debug)]
pub struct DateTimeModule {
    functions: HashMap<String, Val>,
}

impl Default for DateTimeModule {
    fn default() -> Self {
        Self::new()
    }
}

impl DateTimeModule {
    pub fn new() -> Self {
        let mut functions = HashMap::new();
        functions.insert("now".to_string(), runtime_native(now32, 0));
        functions.insert("format".to_string(), runtime_native(format32, 2));
        functions.insert("parse".to_string(), runtime_native(parse32, 2));
        functions.insert("add".to_string(), runtime_native(add_seconds32, 2));
        functions.insert("sub".to_string(), runtime_native(sub_seconds32, 2));
        functions.insert("day_of_week".to_string(), runtime_native(day_of_week32, 1));
        functions.insert("day_of_year".to_string(), runtime_native(day_of_year32, 1));
        functions.insert("is_weekend".to_string(), runtime_native(is_weekend32, 1));

        Self { functions }
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

    fn exports(&self) -> HashMap<String, Val> {
        self.functions.clone()
    }
}

fn runtime_native(function: fn(NativeArgs32<'_>, &mut NativeRuntime32<'_>) -> Result<RuntimeVal>, arity: u16) -> Val {
    Val::runtime_native32(NativeFunction32::Plain(function), arity)
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
    let format = runtime_string_arg(args.get(1).expect("checked arity"), &runtime.state.heap, "format")?;
    let formatted = utc_datetime(timestamp)?.format(format.as_ref()).to_string();
    Ok(runtime_string_value(&formatted, runtime.heap_mut()))
}

fn parse32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, "parse", 2)?;
    let datetime = runtime_string_arg(args.get(0).expect("checked arity"), &runtime.state.heap, "parse")?;
    let format = runtime_string_arg(args.get(1).expect("checked arity"), &runtime.state.heap, "parse")?;
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
