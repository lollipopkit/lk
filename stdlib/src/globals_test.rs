#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use anyhow::Result;
    use lk_core::{
        module, stmt,
        stmt::import::ModuleResolver,
        stmt::stmt_parser::StmtParser,
        token::Tokenizer,
        val::{CallableValue, HeapValue, RuntimeVal},
        vm::{self, NativeFunction},
    };

    fn execute_with_stdlib_globals(source: &str) -> Result<lk_core::vm::ProgramResult> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = module::ModuleRegistry::new();
        crate::register_stdlib_modules(&mut registry)?;
        crate::register_stdlib_globals(&mut registry);

        let resolver = Arc::new(stmt::ModuleResolver::with_registry(registry));
        let mut env = vm::VmContext::new().with_resolver(resolver);
        program.execute_with_ctx(&mut env)
    }

    #[test]
    fn test_global_printf_and_panic_available() -> Result<()> {
        // Program uses print/println (globals) and returns a value
        let result = execute_with_stdlib_globals("print(\"hello {}\", 1); println(\" world\"); return 42;")?;
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
            let _ = program.execute_with_ctx(&mut env);
        }));

        assert!(result.is_err(), "expected panic, but code did not panic");
        Ok(())
    }

    #[test]
    fn test_stdlib_globals_use_runtime_native_abi() {
        let mut registry = module::ModuleRegistry::new();
        crate::register_stdlib_globals(&mut registry);

        for name in [
            "print",
            "println",
            "panic",
            "assert",
            "assert_eq",
            "assert_ne",
            "spawn",
            "chan",
            "send",
            "recv",
            "chan::try_send",
            "chan::try_recv",
            "select$block",
        ] {
            let export = registry.get_runtime_builtin(name).expect("builtin present");
            let state = export.state_lock().expect("runtime export state lock");
            let RuntimeVal::Obj(handle) = export.value() else {
                panic!("{name} should be heap callable");
            };
            let Some(HeapValue::Callable(CallableValue::RuntimeNative { function, .. })) = state.heap().get(*handle)
            else {
                panic!("{name} should use RuntimeNative");
            };
            if matches!(
                name,
                // `spawn` needs full state to snapshot a closure's captures
                // and globals into the goroutine's private heap.
                "print" | "println" | "panic" | "assert" | "assert_eq" | "assert_ne" | "spawn"
            ) {
                assert!(matches!(function, NativeFunction::FullState(_)), "{name}");
            } else {
                assert!(matches!(function, NativeFunction::Plain(_)), "{name}");
            }
        }
    }

    #[test]
    fn test_global_assertions_execute_without_use() -> Result<()> {
        let source = r#"
            assert(true);
            assert(1);
            assert("ok");
            assert_eq(1, 1.0);
            assert_eq(["a", 2], ["a", 2.0]);
            assert_eq({"a": 1, "b": ["x"]}, {"b": ["x"], "a": 1.0});
            assert_ne(1, 2);
            return 42;
        "#;
        let result = execute_with_stdlib_globals(source)?;
        assert_eq!(result.first_return(), &RuntimeVal::Int(42));
        Ok(())
    }

    #[test]
    fn test_global_assertions_panic_on_failure() {
        for (source, expected) in [
            ("assert(false);", "assertion failed"),
            ("assert(nil, \"missing\");", "assertion failed: missing"),
            ("assert_eq(1, 2);", "assertion failed: expected 2, got 1"),
            (
                "assert_eq(1, 2, \"math broke\");",
                "assertion failed: expected 2, got 1 - math broke",
            ),
            ("assert_ne(1, 1);", "assertion failed: values should not be equal"),
            (
                "assert_ne(1, 1, \"duplicate\");",
                "assertion failed: values should not be equal - duplicate",
            ),
        ] {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                execute_with_stdlib_globals(source).expect("program should panic during execution");
            }));
            let payload = result.expect_err("expected assertion panic");
            let message = if let Some(message) = payload.downcast_ref::<String>() {
                message.as_str()
            } else if let Some(message) = payload.downcast_ref::<&str>() {
                message
            } else {
                ""
            };
            assert!(
                message.contains(expected),
                "panic for source `{source}` should contain `{expected}`, got `{message}`"
            );
        }
    }

    #[test]
    fn test_global_assertions_reject_bad_arity_and_named_args() {
        for (source, expected) in [
            ("assert();", "assert() expects 1 or 2 arguments"),
            ("assert(true, \"ok\", \"extra\");", "assert() expects 1 or 2 arguments"),
            ("assert_eq(1);", "assert_eq() expects 2 or 3 arguments"),
            (
                "assert_eq(1, 1, \"ok\", \"extra\");",
                "assert_eq() expects 2 or 3 arguments",
            ),
            ("assert_ne(1);", "assert_ne() expects 2 or 3 arguments"),
            (
                "assert(cond: true);",
                "Compiler missing named-call signature for `assert`",
            ),
        ] {
            let err = execute_with_stdlib_globals(source).expect_err("expected assertion argument error");
            assert!(
                err.to_string().contains(expected),
                "error for source `{source}` should contain `{expected}`, got `{err}`"
            );
        }
    }

    #[test]
    fn test_removed_lk_source_modules_are_not_registered() {
        let registry = module::ModuleRegistry::new();
        let resolver = ModuleResolver::with_registry(registry);

        for module in ["alg", "collections", "func", "math_ext", "test_minimal"] {
            assert!(
                resolver.resolve_runtime_module(module).is_err(),
                "removed LK source module `{module}` should not be registered as a runtime module"
            );
        }
    }

    #[test]
    fn test_global_channel_helpers_execute_on_runtime_native() -> Result<()> {
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

        let result = program.execute_with_ctx(&mut env)?;
        // v2 error model: recv returns the value directly (closed raises).
        assert_eq!(result.first_return(), &RuntimeVal::Int(7));
        Ok(())
    }
}
