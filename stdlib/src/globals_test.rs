#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use anyhow::Result;
    use lk_core::{
        module, stmt,
        stmt::stmt_parser::StmtParser,
        token::Tokenizer,
        val::{CallableValue, HeapValue, RuntimeVal, TypedList},
        vm::{self, NativeFunction32},
    };

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

        let result = program.execute32_with_ctx(&mut env)?;
        assert_eq!(result.first_return(), &RuntimeVal::Int(42));
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
            let _ = program.execute32_with_ctx(&mut env);
        }));

        assert!(result.is_err(), "expected panic, but code did not panic");
        Ok(())
    }

    #[test]
    fn test_stdlib_globals_use_runtime_native32_abi() {
        let mut registry = module::ModuleRegistry::new();
        crate::register_stdlib_globals(&mut registry);

        for name in [
            "print",
            "println",
            "panic",
            "spawn",
            "chan",
            "send",
            "recv",
            "chan::try_send",
            "chan::try_recv",
            "select$block",
        ] {
            let export = registry.get_runtime_builtin(name).expect("builtin present");
            let state = export.state.lock().expect("runtime export state lock");
            let RuntimeVal::Obj(handle) = export.value else {
                panic!("{name} should be heap callable");
            };
            let Some(HeapValue::Callable(CallableValue::RuntimeNative32 { function, .. })) = state.heap.get(handle)
            else {
                panic!("{name} should use RuntimeNative32");
            };
            assert!(matches!(function, NativeFunction32::Plain(_)));
        }
    }

    #[test]
    fn test_global_channel_helpers_execute_on_runtime_native32() -> Result<()> {
        let source = r#"
            let ch = chan(2);
            send(ch, 7);
            return recv(ch);
        "#;
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = module::ModuleRegistry::new();
        crate::register_stdlib_modules(&mut registry)?;
        crate::register_stdlib_globals(&mut registry);

        let resolver = Arc::new(stmt::ModuleResolver::with_registry(registry));
        let mut env = vm::VmContext::new().with_resolver(resolver);

        let result = program.execute32_with_ctx(&mut env)?;
        let RuntimeVal::Obj(handle) = result.first_return() else {
            panic!("expected list object");
        };
        let Some(HeapValue::List(TypedList::Mixed(values))) = result.state.heap.get(*handle) else {
            panic!("expected mixed list return");
        };
        assert_eq!(values, &vec![RuntimeVal::Bool(true), RuntimeVal::Int(7)]);
        Ok(())
    }
}
