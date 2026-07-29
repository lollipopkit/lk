#[cfg(test)]
mod tests {
    use lk_core::vm::ModuleResolver;
    use lk_core::vm::ProgramExec;
    use std::sync::Arc;

    use crate::{datetime::DateTimeModule, register_stdlib_modules, runtime_native::runtime_string_value};
    use anyhow::{Result, anyhow};
    use chrono::{TimeZone, Utc};
    use lk_core::{
        module::ModuleRegistry,
        stmt::stmt_parser::StmtParser,
        token::Tokenizer,
        val::{HeapStore, HeapValue, RuntimeVal},
        vm::{NativeArgs, NativeFunction, NativeRuntime, ProgramResult, RuntimeModuleState, VmContext},
    };

    fn run(source: &str) -> Result<ProgramResult> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        program.execute_with_ctx(&mut env)
    }

    fn datetime_native(name: &str) -> Result<(u16, NativeFunction)> {
        crate::runtime_native::runtime_native_export(&DateTimeModule::new(), name)
    }

    fn call_datetime(name: &str, args: &[RuntimeVal]) -> Result<RuntimeVal> {
        let (_, function) = datetime_native(name)?;
        let NativeFunction::Plain(function) = function else {
            return Err(anyhow!("{name} must use plain RuntimeNative"));
        };
        let mut state = RuntimeModuleState::default();
        let mut runtime = NativeRuntime::new(&mut state, None, None);
        function(NativeArgs::new(args), &mut runtime)
    }

    fn call_datetime_strings(name: &str, left: &str, right: &str) -> Result<RuntimeVal> {
        let (_, function) = datetime_native(name)?;
        let NativeFunction::Plain(function) = function else {
            return Err(anyhow!("{name} must use plain RuntimeNative"));
        };
        let mut state = RuntimeModuleState::default();
        let left = runtime_string_value(left, state.heap_mut());
        let right = runtime_string_value(right, state.heap_mut());
        let args = [left, right];
        let mut runtime = NativeRuntime::new(&mut state, None, None);
        function(NativeArgs::new(&args), &mut runtime)
    }

    fn runtime_str<'a>(value: &'a RuntimeVal, heap: &'a HeapStore) -> Option<&'a str> {
        match value {
            RuntimeVal::ShortStr(value) => Some(value.as_str()),
            RuntimeVal::Obj(handle) => match heap.get(*handle) {
                Some(HeapValue::String(value)) => Some(value.as_ref()),
                _ => None,
            },
            _ => None,
        }
    }

    #[test]
    fn test_format_and_parse_roundtrip() -> Result<()> {
        let ts = Utc.with_ymd_and_hms(2024, 1, 6, 12, 30, 0).unwrap().timestamp();

        let formatted = run("use datetime; return datetime.format(1704544200, \"%Y-%m-%d %H:%M\");")?;
        assert_eq!(
            runtime_str(formatted.first_return(), formatted.state.heap()),
            Some("2024-01-06 12:30")
        );
        assert_eq!(
            call_datetime_strings("parse", "2024-01-06 12:30", "%Y-%m-%d %H:%M")?,
            RuntimeVal::Int(ts)
        );
        Ok(())
    }

    /// `parse` accepts **whatever `format` can write** — a date alone and a time
    /// alone included.
    ///
    /// It used to try only `NaiveDateTime`, which needs both halves, so the pair
    /// could not round-trip: `format(t, "%Y-%m-%d")` gave `1970-01-02` and
    /// parsing it back with the same format string answered chrono's "input is
    /// not enough for unique date and time". A format string describes the text
    /// on both sides; the two directions have to agree about what it describes.
    #[test]
    fn parse_accepts_every_shape_format_writes() -> Result<()> {
        // Date only → midnight UTC, which is the half `format` dropped.
        assert_eq!(
            call_datetime_strings("parse", "1970-01-02", "%Y-%m-%d")?,
            RuntimeVal::Int(86400)
        );
        // Time only → that time on the epoch day.
        assert_eq!(
            call_datetime_strings("parse", "01:01:01", "%H:%M:%S")?,
            RuntimeVal::Int(3661)
        );
        // Before the epoch too.
        assert_eq!(
            call_datetime_strings("parse", "1969-12-31", "%Y-%m-%d")?,
            RuntimeVal::Int(-86400)
        );
        // And the error names the format instead of describing chrono's parser.
        let error = call_datetime_strings("parse", "zz", "%Y-%m-%d").expect_err("not a date");
        let text = format!("{error:#}");
        assert!(text.contains("does not match the format"), "unexpected error: {text}");
        Ok(())
    }

    #[test]
    fn test_day_of_week_and_weekend() -> Result<()> {
        let saturday = Utc.with_ymd_and_hms(2024, 1, 6, 0, 0, 0).unwrap().timestamp();
        let monday = Utc.with_ymd_and_hms(2024, 1, 8, 0, 0, 0).unwrap().timestamp();

        assert_eq!(
            call_datetime("day_of_week", &[RuntimeVal::Int(saturday)])?,
            RuntimeVal::Int(6)
        );
        assert_eq!(
            call_datetime("day_of_week", &[RuntimeVal::Int(monday)])?,
            RuntimeVal::Int(1)
        );
        assert_eq!(
            call_datetime("is_weekend", &[RuntimeVal::Int(saturday)])?,
            RuntimeVal::Bool(true)
        );
        assert_eq!(
            call_datetime("is_weekend", &[RuntimeVal::Int(monday)])?,
            RuntimeVal::Bool(false)
        );
        Ok(())
    }

    #[test]
    fn test_add_sub_and_day_of_year() -> Result<()> {
        let base = 1_700_000_000i64;
        assert_eq!(
            run("use datetime; return datetime.add(1700000000, 30);")?.first_return(),
            &RuntimeVal::Int(base + 30)
        );
        assert_eq!(
            run("use datetime; return datetime.sub(1700000000, 45);")?.first_return(),
            &RuntimeVal::Int(base - 45)
        );
        assert_eq!(
            call_datetime("day_of_year", &[RuntimeVal::Int(1704544200)])?,
            RuntimeVal::Int(6)
        );
        Ok(())
    }

    #[test]
    fn test_format_invalid_timestamp_errors() {
        let err = call_datetime_strings("format", "not-used", "%Y").expect_err("format should reject wrong first arg");
        assert!(err.to_string().contains("integer timestamp"));

        let err =
            call_datetime("format", &[RuntimeVal::Int(i64::MAX)]).expect_err("wrong arity should error before format");
        assert!(err.to_string().contains("takes exactly 2 arguments"));
    }

    #[test]
    fn test_parse_invalid_string_errors() {
        let err =
            call_datetime_strings("parse", "not-a-date", "%Y-%m-%d").expect_err("invalid datetime string should error");
        // The text names the value and the format, not chrono's own parser
        // requirement ("input is not enough for unique date and time") — a
        // sentence about a library the program never mentioned.
        let text = err.to_string();
        assert!(text.contains("not-a-date"), "unexpected error: {text}");
        assert!(text.contains("%Y-%m-%d"), "unexpected error: {text}");
    }

    #[test]
    fn test_datetime_functions_use_runtime_native_abi() -> Result<()> {
        for name in [
            "now",
            "format",
            "parse",
            "add",
            "sub",
            "day_of_week",
            "day_of_year",
            "is_weekend",
        ] {
            let (arity, function) = datetime_native(name)?;
            assert!(matches!(function, NativeFunction::Plain(_)));
            assert_ne!(arity, lk_core::vm::NativeEntry::VARIADIC);
        }
        Ok(())
    }

    #[test]
    fn test_datetime_now() -> Result<()> {
        let result = run("use datetime; return datetime.now();")?;
        let RuntimeVal::Int(timestamp) = result.first_return() else {
            panic!("Expected integer timestamp");
        };
        assert!(*timestamp > 0, "Timestamp should be positive");
        Ok(())
    }

    #[test]
    fn test_datetime_format() -> Result<()> {
        let formatted = run("use datetime; return datetime.format(1672531200, \"%Y-%m-%d\");")?;
        assert_eq!(
            runtime_str(formatted.first_return(), formatted.state.heap()),
            Some("2023-01-01")
        );
        Ok(())
    }
}
