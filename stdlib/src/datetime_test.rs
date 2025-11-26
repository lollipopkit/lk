#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{datetime::DateTimeModule, register_stdlib_modules};
    use anyhow::Result;
    use chrono::{TimeZone, Utc};
    use lkr_core::{
        module, module::Module, stmt, stmt::stmt_parser::StmtParser, token::Tokenizer, val, val::Val, vm, vm::VmContext,
    };

    fn get_fn(module: &DateTimeModule, name: &str) -> val::RustFunction {
        let exports = module.exports();
        let val = exports
            .get(name)
            .unwrap_or_else(|| panic!("{name} export missing"))
            .clone();
        match val {
            Val::RustFunction(func) => func,
            other => panic!("expected RustFunction, got {:?}", other),
        }
    }

    #[test]
    fn test_format_and_parse_roundtrip() -> Result<()> {
        let module = DateTimeModule::new();
        let mut env = VmContext::new();
        let ts = Utc.with_ymd_and_hms(2024, 1, 6, 12, 30, 0).unwrap().timestamp();

        let format_fn = get_fn(&module, "format");
        let formatted = format_fn(&[Val::Int(ts), Val::Str("%Y-%m-%d %H:%M".into())], &mut env)?;
        assert_eq!(formatted, Val::Str("2024-01-06 12:30".into()));

        let parse_fn = get_fn(&module, "parse");
        let parsed = parse_fn(
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

        let day_of_week = get_fn(&module, "day_of_week");
        assert_eq!(day_of_week(&[Val::Int(saturday)], &mut env)?, Val::Int(6));
        assert_eq!(day_of_week(&[Val::Int(monday)], &mut env)?, Val::Int(1));

        let is_weekend = get_fn(&module, "is_weekend");
        assert_eq!(is_weekend(&[Val::Int(saturday)], &mut env)?, Val::Bool(true));
        assert_eq!(is_weekend(&[Val::Int(monday)], &mut env)?, Val::Bool(false));
        Ok(())
    }

    #[test]
    fn test_add_and_sub_seconds() -> Result<()> {
        let module = DateTimeModule::new();
        let mut env = VmContext::new();
        let base = 1_700_000_000i64;

        let add_fn = get_fn(&module, "add");
        assert_eq!(add_fn(&[Val::Int(base), Val::Int(30)], &mut env)?, Val::Int(base + 30));

        let sub_fn = get_fn(&module, "sub");
        assert_eq!(sub_fn(&[Val::Int(base), Val::Int(45)], &mut env)?, Val::Int(base - 45));
        Ok(())
    }

    #[test]
    fn test_format_invalid_timestamp_errors() {
        let module = DateTimeModule::new();
        let mut env = VmContext::new();
        let format_fn = get_fn(&module, "format");
        let err = format_fn(&[Val::Int(i64::MAX), Val::Str("%Y".into())], &mut env)
            .expect_err("invalid timestamp should error");
        assert!(err.to_string().contains("invalid timestamp"));
    }

    #[test]
    fn test_parse_invalid_string_errors() {
        let module = DateTimeModule::new();
        let mut env = VmContext::new();
        let parse_fn = get_fn(&module, "parse");
        let err = parse_fn(&[Val::Str("not-a-date".into()), Val::Str("%Y-%m-%d".into())], &mut env)
            .expect_err("invalid datetime string should error");
        assert!(err.to_string().contains("failed to parse datetime"));
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
