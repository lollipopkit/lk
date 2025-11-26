#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use anyhow::Result;
    use lkr_core::{module, stmt, stmt::stmt_parser::StmtParser, token::Tokenizer, val::Val, vm};

    #[test]
    fn test_global_printf_and_panic_available() -> Result<()> {
        // Program uses print/println (globals) and returns a value
        let source = "print(\"hello {}\", 1); println(\" world\"); return 42;";
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        // Create registry, register modules + globals
        let mut registry = module::ModuleRegistry::new();
        crate::register_stdlib_modules(&mut registry)?;
        crate::register_stdlib_globals(&mut registry);

        // Create environment with this registry
        let resolver = Arc::new(stmt::ModuleResolver::with_registry(registry));
        let mut env = vm::VmContext::new().with_resolver(resolver);
        let mut machine = vm::Vm::new();

        let result = program.execute_with_vm(&mut machine, &mut env)?;
        assert_eq!(result, Val::Int(42));
        Ok(())
    }

    #[test]
    fn test_global_panic_panics_with_backtrace() -> Result<()> {
        let source = "panic(\"boom\");";
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = module::ModuleRegistry::new();
        crate::register_stdlib_modules(&mut registry)?;
        crate::register_stdlib_globals(&mut registry);

        let resolver = Arc::new(stmt::ModuleResolver::with_registry(registry));
        let mut env = vm::VmContext::new().with_resolver(resolver);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut machine = vm::Vm::new();
            let _ = program.execute_with_vm(&mut machine, &mut env);
        }));

        assert!(result.is_err(), "expected panic, but code did not panic");
        Ok(())
    }
}
