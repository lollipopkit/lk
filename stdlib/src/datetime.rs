use anyhow::Result;
use chrono::Datelike;
use lkr_core::module::Module;
use lkr_core::val::Val;
use lkr_core::vm::VmContext;
use std::collections::HashMap;

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

        // Register datetime functions as Rust functions
        functions.insert("now".to_string(), Val::RustFunction(Self::now));
        functions.insert("format".to_string(), Val::RustFunction(Self::format));
        functions.insert("parse".to_string(), Val::RustFunction(Self::parse));
        functions.insert("add".to_string(), Val::RustFunction(Self::add_seconds));
        functions.insert("sub".to_string(), Val::RustFunction(Self::sub_seconds));
        functions.insert("day_of_week".to_string(), Val::RustFunction(Self::day_of_week));
        functions.insert("day_of_year".to_string(), Val::RustFunction(Self::day_of_year));
        functions.insert("is_weekend".to_string(), Val::RustFunction(Self::is_weekend));

        Self { functions }
    }

    /// Get current timestamp as Unix epoch
    fn now(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if !args.is_empty() {
            return Err(anyhow::anyhow!("now() takes no arguments"));
        }

        use chrono::{DateTime, Utc};
        let now: DateTime<Utc> = Utc::now();
        let timestamp = now.timestamp_micros();
        Ok(Val::Int(timestamp))
    }

    /// Format timestamp to string
    fn format(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow::anyhow!(
                "format() takes exactly 2 arguments: timestamp and format_string"
            ));
        }

        let timestamp = match &args[0] {
            Val::Int(ts) => *ts,
            _ => {
                return Err(anyhow::anyhow!("first argument must be an integer timestamp"));
            }
        };

        let format_str = match &args[1] {
            Val::Str(fmt) => &**fmt,
            _ => return Err(anyhow::anyhow!("second argument must be a format string")),
        };

        use chrono::{DateTime, Utc};
        let dt = DateTime::<Utc>::from_timestamp(timestamp, 0).ok_or_else(|| anyhow::anyhow!("invalid timestamp"))?;

        let formatted = dt.format(format_str).to_string();
        Ok(Val::Str(formatted.into()))
    }

    /// Parse string to timestamp
    fn parse(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow::anyhow!(
                "parse() takes exactly 2 arguments: datetime_string and format_string"
            ));
        }

        let datetime_str = match &args[0] {
            Val::Str(s) => &**s,
            _ => return Err(anyhow::anyhow!("first argument must be a datetime string")),
        };

        let format_str = match &args[1] {
            Val::Str(fmt) => &**fmt,
            _ => return Err(anyhow::anyhow!("second argument must be a format string")),
        };

        use chrono::{DateTime, NaiveDateTime, Utc};
        let naive = NaiveDateTime::parse_from_str(datetime_str, format_str)
            .map_err(|e| anyhow::anyhow!("failed to parse datetime: {}", e))?;
        let dt = DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc);

        Ok(Val::Int(dt.timestamp()))
    }

    /// Add seconds to timestamp
    fn add_seconds(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow::anyhow!(
                "add_seconds() takes exactly 2 arguments: timestamp and seconds"
            ));
        }

        let timestamp = match &args[0] {
            Val::Int(ts) => *ts,
            _ => {
                return Err(anyhow::anyhow!("first argument must be an integer timestamp"));
            }
        };

        let seconds = match &args[1] {
            Val::Int(s) => *s,
            _ => return Err(anyhow::anyhow!("second argument must be an integer")),
        };

        Ok(Val::Int(timestamp + seconds))
    }

    /// Subtract seconds from timestamp
    fn sub_seconds(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow::anyhow!(
                "sub_seconds() takes exactly 2 arguments: timestamp and seconds"
            ));
        }
        let timestamp = match &args[0] {
            Val::Int(ts) => *ts,
            _ => {
                return Err(anyhow::anyhow!("first argument must be an integer timestamp"));
            }
        };
        let seconds = match &args[1] {
            Val::Int(s) => *s,
            _ => return Err(anyhow::anyhow!("second argument must be an integer")),
        };
        Ok(Val::Int(timestamp - seconds))
    }

    /// Get day of week (0 = Sunday, 1 = Monday, ..., 6 = Saturday)
    fn day_of_week(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow::anyhow!("day_of_week() takes exactly 1 argument: timestamp"));
        }

        let timestamp = match &args[0] {
            Val::Int(ts) => *ts,
            _ => return Err(anyhow::anyhow!("argument must be an integer timestamp")),
        };

        use chrono::{DateTime, Utc, Weekday};
        let dt = DateTime::<Utc>::from_timestamp(timestamp, 0).ok_or_else(|| anyhow::anyhow!("invalid timestamp"))?;

        let day_num = match dt.weekday() {
            Weekday::Sun => 0,
            Weekday::Mon => 1,
            Weekday::Tue => 2,
            Weekday::Wed => 3,
            Weekday::Thu => 4,
            Weekday::Fri => 5,
            Weekday::Sat => 6,
        };

        Ok(Val::Int(day_num))
    }

    /// Get day of year (1-366)
    fn day_of_year(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow::anyhow!("day_of_year() takes exactly 1 argument: timestamp"));
        }

        let timestamp = match &args[0] {
            Val::Int(ts) => *ts,
            _ => return Err(anyhow::anyhow!("argument must be an integer timestamp")),
        };

        use chrono::{DateTime, Utc};
        let dt = DateTime::<Utc>::from_timestamp(timestamp, 0).ok_or_else(|| anyhow::anyhow!("invalid timestamp"))?;

        Ok(Val::Int(dt.ordinal() as i64))
    }

    /// Check if date is weekend (Saturday or Sunday)
    fn is_weekend(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow::anyhow!("is_weekend() takes exactly 1 argument: timestamp"));
        }

        let timestamp = match &args[0] {
            Val::Int(ts) => *ts,
            _ => return Err(anyhow::anyhow!("argument must be an integer timestamp")),
        };

        use chrono::{DateTime, Utc, Weekday};
        let dt = DateTime::<Utc>::from_timestamp(timestamp, 0).ok_or_else(|| anyhow::anyhow!("invalid timestamp"))?;

        let is_weekend = matches!(dt.weekday(), Weekday::Sat | Weekday::Sun);
        Ok(Val::Bool(is_weekend))
    }
}

impl Module for DateTimeModule {
    fn name(&self) -> &str {
        "datetime"
    }

    fn description(&self) -> &str {
        "Date and time functions"
    }

    fn register(&self, _registry: &mut lkr_core::module::ModuleRegistry) -> Result<()> {
        // Don't register functions globally - they should be accessed via module.function()
        Ok(())
    }

    fn exports(&self) -> HashMap<String, Val> {
        self.functions.clone()
    }
}
