#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{datetime::DateTimeModule, register_stdlib_modules, runtime_native::runtime_string_value};
    use anyhow::{Result, anyhow};
    use chrono::{TimeZone, Utc};
    use lk_core::{
        module::ModuleRegistry,
        stmt::{ModuleResolver, stmt_parser::StmtParser},
        token::Tokenizer,
        val::{HeapStore, HeapValue, RuntimeVal},
        vm::{NativeArgs32, NativeFunction32, NativeRuntime32, Program32Result, RuntimeModuleState32, VmContext},
    };

    fn run32(source: &str) -> Result<Program32Result> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        program.execute32_with_ctx(&mut env)
    }

    fn datetime_native(name: &str) -> Result<(u16, NativeFunction32)> {
        crate::runtime_native::runtime_native_export(&DateTimeModule::new(), name)
    }

    fn call_datetime(name: &str, args: &[RuntimeVal]) -> Result<RuntimeVal> {
        let (_, function) = datetime_native(name)?;
        let NativeFunction32::Plain(function) = function else {
            return Err(anyhow!("{name} must use plain RuntimeNative32"));
        };
        let mut state = RuntimeModuleState32::default();
        let mut runtime = NativeRuntime32::new(&mut state, None, None);
        function(NativeArgs32::new(args), &mut runtime)
    }

    fn call_datetime_strings(name: &str, left: &str, right: &str) -> Result<RuntimeVal> {
        let (_, function) = datetime_native(name)?;
        let NativeFunction32::Plain(function) = function else {
            return Err(anyhow!("{name} must use plain RuntimeNative32"));
        };
        let mut state = RuntimeModuleState32::default();
        let left = runtime_string_value(left, &mut state.heap);
        let right = runtime_string_value(right, &mut state.heap);
        let args = [left, right];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);
        function(NativeArgs32::new(&args), &mut runtime)
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

        let formatted = run32("import datetime; return datetime.format(1704544200, \"%Y-%m-%d %H:%M\");")?;
        assert_eq!(
            runtime_str(formatted.first_return(), &formatted.state.heap),
            Some("2024-01-06 12:30")
        );
        assert_eq!(
            call_datetime_strings("parse", "2024-01-06 12:30", "%Y-%m-%d %H:%M")?,
            RuntimeVal::Int(ts)
        );
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
            run32("import datetime; return datetime.add(1700000000, 30);")?.first_return(),
            &RuntimeVal::Int(base + 30)
        );
        assert_eq!(
            run32("import datetime; return datetime.sub(1700000000, 45);")?.first_return(),
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
        assert!(err.to_string().contains("failed to parse datetime"));
    }

    #[test]
    fn test_datetime_functions_use_runtime_native32_abi() -> Result<()> {
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
            assert!(matches!(function, NativeFunction32::Plain(_)));
            assert_ne!(arity, lk_core::vm::NativeEntry32::VARIADIC);
        }
        Ok(())
    }

    #[test]
    fn test_datetime_now() -> Result<()> {
        let result = run32("import datetime; return datetime.now();")?;
        let RuntimeVal::Int(timestamp) = result.first_return() else {
            panic!("Expected integer timestamp");
        };
        assert!(*timestamp > 0, "Timestamp should be positive");
        Ok(())
    }

    #[test]
    fn test_datetime_format() -> Result<()> {
        let formatted = run32("import datetime; return datetime.format(1672531200, \"%Y-%m-%d\");")?;
        assert_eq!(
            runtime_str(formatted.first_return(), &formatted.state.heap),
            Some("2023-01-01")
        );
        Ok(())
    }
}
