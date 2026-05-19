#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{datetime::DateTimeModule, register_stdlib_modules};
    use anyhow::Result;
    use chrono::{TimeZone, Utc};
    use lk_core::{
        module, module::Module, stmt, stmt::stmt_parser::StmtParser, token::Tokenizer, val::NativeArgs, val::Val, vm,
        vm::VmContext,
    };

    fn call_fast(module: &DateTimeModule, name: &str, args: &[Val], env: &mut VmContext) -> Result<Val> {
        let exports = module.exports();
        let val = exports
            .get(name)
            .unwrap_or_else(|| panic!("{name} export missing"))
            .clone();
        match val {
            Val::RustFastFunction(func) => func(NativeArgs::new(args), env),
            other => panic!("expected RustFastFunction, got {:?}", other),
        }
    }

    #[test]
    fn test_format_and_parse_roundtrip() -> Result<()> {
        let module = DateTimeModule::new();
        let mut env = VmContext::new();
        let ts = Utc.with_ymd_and_hms(2024, 1, 6, 12, 30, 0).unwrap().timestamp();

        let formatted = call_fast(
            &module,
            "format",
            &[Val::Int(ts), Val::Str("%Y-%m-%d %H:%M".into())],
            &mut env,
        )?;
        assert_eq!(formatted, Val::Str("2024-01-06 12:30".into()));

        let parsed = call_fast(
            &module,
            "parse",
            &[Val::Str("2024-01-06 12:30".into()), Val::Str("%Y-%m-%d %H:%M".into())],
            &mut env,
        )?;
        assert_eq!(parsed, Val::Int(ts));
        Ok(())
    }

    #[test]
    fn test_day_of_week_and_weekend() -> Result<()> {
        let module = DateTimeModule::new();
        let mut env = VmContext::new();
        let saturday = Utc.with_ymd_and_hms(2024, 1, 6, 0, 0, 0).unwrap().timestamp();
        let monday = Utc.with_ymd_and_hms(2024, 1, 8, 0, 0, 0).unwrap().timestamp();

        assert_eq!(
            call_fast(&module, "day_of_week", &[Val::Int(saturday)], &mut env)?,
            Val::Int(6)
        );
        assert_eq!(
            call_fast(&module, "day_of_week", &[Val::Int(monday)], &mut env)?,
            Val::Int(1)
        );

        assert_eq!(
            call_fast(&module, "is_weekend", &[Val::Int(saturday)], &mut env)?,
            Val::Bool(true)
        );
        assert_eq!(
            call_fast(&module, "is_weekend", &[Val::Int(monday)], &mut env)?,
            Val::Bool(false)
        );
        Ok(())
    }

    #[test]
    fn test_add_and_sub_seconds() -> Result<()> {
        let module = DateTimeModule::new();
        let mut env = VmContext::new();
        let base = 1_700_000_000i64;

        assert_eq!(
            call_fast(&module, "add", &[Val::Int(base), Val::Int(30)], &mut env)?,
            Val::Int(base + 30)
        );

        assert_eq!(
            call_fast(&module, "sub", &[Val::Int(base), Val::Int(45)], &mut env)?,
            Val::Int(base - 45)
        );
        Ok(())
    }

    #[test]
    fn test_format_invalid_timestamp_errors() {
        let module = DateTimeModule::new();
        let mut env = VmContext::new();
        let err = call_fast(
            &module,
            "format",
            &[Val::Int(i64::MAX), Val::Str("%Y".into())],
            &mut env,
        )
        .expect_err("invalid timestamp should error");
        assert!(err.to_string().contains("invalid timestamp"));
    }

    #[test]
    fn test_parse_invalid_string_errors() {
        let module = DateTimeModule::new();
        let mut env = VmContext::new();
        let err = call_fast(
            &module,
            "parse",
            &[Val::Str("not-a-date".into()), Val::Str("%Y-%m-%d".into())],
            &mut env,
        )
        .expect_err("invalid datetime string should error");
        assert!(err.to_string().contains("failed to parse datetime"));
    }

    #[test]
    fn test_datetime_functions_use_fast_native_abi() {
        let module = DateTimeModule::new();
        let exports = module.exports();
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
            let value = exports.get(name).expect("datetime function export present");
            assert!(
                matches!(value, Val::RustFastFunction(_)),
                "{name} should use RustFastFunction"
            );
        }
    }

    #[test]
    fn test_datetime_now() -> Result<()> {
        let source = "import datetime; return datetime.now();";
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        // Create registry and register stdlib modules
        let mut registry = module::ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;

        // Create environment with stdlib modules
        let resolver = Arc::new(stmt::ModuleResolver::with_registry(registry));
        let mut env = vm::VmContext::new().with_resolver(resolver);
        let mut machine = vm::Vm::new();

        let result = program.execute_with_vm(&mut machine, &mut env)?;
        if let Val::Int(timestamp) = result {
            assert!(timestamp > 0, "Timestamp should be positive");
        } else {
            panic!("Expected integer timestamp");
        }

        Ok(())
    }

    #[test]
    fn test_datetime_format() -> Result<()> {
        let source = "import datetime; return datetime.format(1672531200, \"%Y-%m-%d\");";
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        // Create registry and register stdlib modules
        let mut registry = module::ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;

        // Create environment with stdlib modules
        let resolver = Arc::new(stmt::ModuleResolver::with_registry(registry));
        let mut env = vm::VmContext::new().with_resolver(resolver);
        let mut machine = vm::Vm::new();

        let result = program.execute_with_vm(&mut machine, &mut env)?;
        assert_eq!(result, Val::Str("2023-01-01".into()));

        Ok(())
    }
}
