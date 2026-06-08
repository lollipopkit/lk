#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::{
        expr::Expr,
        stmt::{Program, Stmt, run_program, run_program_default, stmt_parser::StmtParser},
        token::Tokenizer,
        typ::TypeChecker,
        val::{HeapStore, HeapValue, LiteralVal, RuntimeVal, ShortStr, TypedMap},
        vm::{
            ProgramResult, VmContext, call_runtime_callable_runtime, call_runtime_callable_test,
            runtime_value_to_callable_shared,
        },
    };
    use anyhow::Result;

    fn expect_return_int(result: &ProgramResult, expected: i64) {
        assert_eq!(result.first_return(), &RuntimeVal::Int(expected));
    }

    fn expect_return_nil(result: &ProgramResult) {
        assert_eq!(result.first_return(), &RuntimeVal::Nil);
    }

    fn expect_return_str(result: &ProgramResult, expected: &str) {
        assert_eq!(
            result.first_return(),
            &RuntimeVal::ShortStr(ShortStr::new(expected).expect("short test string"))
        );
    }

    #[test]
    fn test_function_definition_parsing() -> Result<()> {
        let source = "fn add(a, b) { return a + b; }";
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let stmt = parser.parse_statement()?;

        if let Stmt::Function { name, params, body, .. } = stmt {
            assert_eq!(name, "add");
            assert_eq!(params, vec!["a", "b"]);
            assert!(matches!(body.as_ref(), Stmt::Block { .. }));
        } else {
            panic!("Expected Function statement, got: {:?}", stmt);
        }

        Ok(())
    }

    #[test]
    fn test_function_no_params_parsing() -> Result<()> {
        let source = "fn hello() { return \"Hello, World!\"; }";
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let stmt = parser.parse_statement()?;

        if let Stmt::Function { name, params, .. } = stmt {
            assert_eq!(name, "hello");
            assert!(params.is_empty());
        } else {
            panic!("Expected Function statement");
        }

        Ok(())
    }

    #[test]
    fn test_function_call_parsing() -> Result<()> {
        let tokens = Tokenizer::tokenize("add(1, 2)")?;
        let mut parser = crate::ast::Parser::new(&tokens);
        let expr = parser.parse()?;

        if let Expr::CallExpr(expr, args) = expr {
            if let Expr::Var(name) = *expr {
                assert_eq!(name, "add");
                assert_eq!(args.len(), 2);
                assert_eq!(args[0].as_ref(), &Expr::Literal(LiteralVal::Int(1)));
                assert_eq!(args[1].as_ref(), &Expr::Literal(LiteralVal::Int(2)));
            } else {
                panic!("Expected variable as function target, got: {:?}", expr);
            }
        } else {
            panic!("Expected function call, got: {:?}", expr);
        }

        Ok(())
    }

    #[test]
    fn test_function_call_no_args_parsing() -> Result<()> {
        let tokens = Tokenizer::tokenize("hello()")?;
        let mut parser = crate::ast::Parser::new(&tokens);
        let expr = parser.parse()?;

        if let Expr::CallExpr(expr, args) = expr {
            if let Expr::Var(name) = *expr {
                assert_eq!(name, "hello");
                assert!(args.is_empty());
            } else {
                panic!("Expected variable as function target, got: {:?}", expr);
            }
        } else {
            panic!("Expected function call");
        }

        Ok(())
    }

    #[test]
    fn test_function_execution_simple() -> Result<()> {
        let source = "fn add(a, b) { return a + b; } return add(3, 4);";
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let result = run_program_default(&program)?;
        expect_return_int(&result, 7);

        Ok(())
    }

    #[test]
    fn test_function_execution_no_params() -> Result<()> {
        let source = "fn greeting() { return \"Hello!\"; } return greeting();";
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let result = run_program_default(&program)?;
        expect_return_str(&result, "Hello!");

        Ok(())
    }

    #[test]
    fn test_function_execution_with_variables() -> Result<()> {
        let source = r#"
            fn multiply(x, y) {
                let result = x * y;
                return result;
            }
            let a = 5;
            let b = 6;
            return multiply(a, b);
        "#;
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let result = run_program_default(&program)?;
        expect_return_int(&result, 30);

        Ok(())
    }

    #[test]
    fn test_function_parameter_scope() -> Result<()> {
        let source = r#"
            let x = 10;
            fn test(x) {
                return x + 1;
            }
            return test(5);
        "#;
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let result = run_program_default(&program)?;
        // Should return 6 (5 + 1), not 11 (10 + 1)
        expect_return_int(&result, 6);

        Ok(())
    }

    #[test]
    fn test_function_returns_nil_by_default() -> Result<()> {
        let source = "fn test() { let x = 5; } return test();";
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let result = run_program_default(&program)?;
        expect_return_nil(&result);

        Ok(())
    }

    #[test]
    fn test_recursive_function() -> Result<()> {
        let source = r#"
            fn factorial(n) {
                if (n <= 1) {
                    return 1;
                } else {
                    return n * factorial(n - 1);
                }
            }
            return factorial(5);
        "#;
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let result = run_program_default(&program)?;
        expect_return_int(&result, 120);

        Ok(())
    }

    #[test]
    fn test_function_with_context_access() -> Result<()> {
        let source = r#"
            fn getUserAge() {
                return user.age;
            }
            return getUserAge();
        "#;
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut env = VmContext::new();
        let mut heap = HeapStore::new();
        let user = heap.alloc(HeapValue::Map(TypedMap::StringInt(
            [(Arc::<str>::from("age"), 25)].into_iter().collect(),
        )));
        env.define_runtime_value("user", RuntimeVal::Obj(user), heap);

        let result = run_program(&program, &mut env)?;
        expect_return_int(&result, 25);

        Ok(())
    }

    #[test]
    fn test_function_call_with_wrong_arg_count() -> Result<()> {
        let source = "fn add(a, b) { return a + b; } add(1);";
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let result = run_program_default(&program);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("expects") || err.to_string().contains("arguments"));

        Ok(())
    }

    #[test]
    fn test_undefined_function_call() -> Result<()> {
        let source = "nonexistent();";
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let result = run_program_default(&program);
        assert!(result.is_err());

        Ok(())
    }

    #[test]
    fn test_calling_non_function() -> Result<()> {
        let source = "let x = 5; x();";
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let result = run_program_default(&program);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("function") || err.to_string().contains("call"));

        Ok(())
    }

    #[test]
    fn test_nested_function_calls() -> Result<()> {
        let source = r#"
            fn add(a, b) { return a + b; }
            fn multiply(x, y) { return x * y; }
            fn compute() { return add(multiply(2, 3), 4); }
            return compute();
        "#;
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let result = run_program_default(&program)?;
        expect_return_int(&result, 10); // multiply(2, 3) = 6, add(6, 4) = 10

        Ok(())
    }

    #[test]
    fn test_named_call_basic_and_order() -> Result<()> {
        let source = r#"
            fn add({x: Int, y: Int}) { return x + y; }
            let a = add(x: 1, y: 2);
            let b = add(y: 2, x: 1);
            return a + b;
        "#;
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;
        let result = run_program_default(&program)?;
        expect_return_int(&result, 6);
        Ok(())
    }

    #[test]
    fn test_named_call_missing_required_errors() -> Result<()> {
        let source = r#"
            fn add({x: Int, y: Int}) { return x + y; }
            add(x: 1);
        "#;
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;
        let result = run_program_default(&program);
        assert!(result.is_err());
        let msg = result.err().unwrap().to_string();
        assert!(msg.contains("Missing required named argument: y"));
        Ok(())
    }

    // Named-call runtime tests will be added after type checker and runtime binding finalize.

    #[test]
    fn test_named_call_unknown_and_duplicate_errors() -> Result<()> {
        // Unknown name error
        let source1 = r#"
            fn sum(x: Int, y: Int, {z: Int}) { return x + y + z; }
            sum(1, 2, w: 3);
        "#;
        let tokens = Tokenizer::tokenize(source1)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;
        let result = run_program_default(&program);
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("Unknown named argument: w"));

        // Duplicate name error
        let source2 = r#"
            fn sum(x: Int, y: Int, {z: Int}) { return x + y + z; }
            sum(1, 2, z: 3, z: 4);
        "#;
        let tokens = Tokenizer::tokenize(source2)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;
        let result = run_program_default(&program);
        assert!(result.is_err());
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("Duplicate named argument: z")
        );
        Ok(())
    }

    #[test]
    fn test_named_call_positional_after_named_is_parse_error() -> Result<()> {
        // f(a: 1, 2) should be a parse error
        let tokens = Tokenizer::tokenize("f(a: 1, 2);")?;
        let mut parser = StmtParser::new(&tokens);
        let res = parser.parse_program();
        assert!(res.is_err());
        Ok(())
    }

    #[test]
    fn test_closure_captures_enclosing_scope() -> Result<()> {
        let closure_source = r#"
            fn outer() {
                let offset = 2;
                return |x| x + offset;
            }

            return outer();
        "#;

        let closure_tokens = Tokenizer::tokenize(closure_source)?;
        let mut closure_parser = StmtParser::new(&closure_tokens);
        let closure_program = closure_parser.parse_program()?;
        let mut closure_ctx = VmContext::new();
        let closure_result = closure_program.execute_with_ctx(&mut closure_ctx)?;
        let closure_value = closure_result.first_return();
        let crate::val::RuntimeVal::Obj(handle) = closure_value else {
            panic!("expected runtime callable object, got {:?}", closure_value.kind());
        };
        assert!(matches!(
            closure_result.state.heap.get(*handle).map(|value| value),
            Some(HeapValue::Callable(_))
        ));

        let capture_source = r#"
            fn outer() {
                let offset = 2;
                return |x| x + offset;
            }

            let add2 = outer();
            return add2(40);
        "#;

        let capture_tokens = Tokenizer::tokenize(capture_source)?;
        let mut capture_parser = StmtParser::new(&capture_tokens);
        let capture_program = capture_parser.parse_program()?;
        let capture_result = run_program_default(&capture_program)?;
        expect_return_int(&capture_result, 42);

        let lexical_source = r#"
            fn outer() {
                let offset = 2;
                return |x| x + offset;
            }

            let offset = 100;
            let add2 = outer();
            return add2(1) + offset;
        "#;

        let lexical_tokens = Tokenizer::tokenize(lexical_source)?;
        let mut lexical_parser = StmtParser::new(&lexical_tokens);
        let lexical_program = lexical_parser.parse_program()?;
        let lexical_result = run_program_default(&lexical_program)?;
        expect_return_int(&lexical_result, 103);

        Ok(())
    }

    #[test]
    fn test_outer_returns_closure_value() -> Result<()> {
        let source = "fn outer() { let offset = 2; return |x| x + offset; }";
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let stmt = parser.parse_statement()?;
        let mut env = VmContext::new();
        let program = Program::new(vec![Box::new(stmt)])?;
        let result = program.execute_with_ctx(&mut env)?;
        let module = Arc::clone(&result.module);
        let shared_state = Arc::new(Mutex::new(result.state));
        let outer = {
            let state = shared_state.lock().expect("test runtime state lock");
            state
                .globals()
                .iter()
                .find_map(|value| {
                    runtime_value_to_callable_shared(
                        value,
                        state.heap(),
                        Arc::clone(&module),
                        Arc::clone(&shared_state),
                    )
                })
                .expect("outer runtime function exported as module global")
        };

        // Introduce a runtime global with the same name after capturing; lexical semantics
        // should continue to use the captured value (2) rather than the new global (100).
        env.define_runtime_value("offset", RuntimeVal::Int(100), HeapStore::new());

        let captured_returns = call_runtime_callable_test(&outer, &[], &mut env)?;
        let captured_value = captured_returns.first().cloned().unwrap_or(RuntimeVal::Nil);
        let captured_state = Arc::clone(&outer.state);
        let captured = {
            let state = captured_state.lock().expect("test runtime state lock");
            runtime_value_to_callable_shared(
                &captured_value,
                state.heap(),
                Arc::clone(&module),
                Arc::clone(&captured_state),
            )
            .expect("outer returned runtime closure")
        };
        let mut heap = HeapStore::new();
        let value = call_runtime_callable_runtime(&captured, &[RuntimeVal::Int(0)], &mut heap, Some(&mut env))?;
        assert_eq!(value, RuntimeVal::Int(2));

        Ok(())
    }

    #[test]
    fn debug_factorial_environment() -> Result<()> {
        let source = "fn factorial(n) { if (n <= 1) { return 1; } return n * factorial(n - 1); }";
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let stmt = parser.parse_statement()?;
        let mut env = VmContext::new();
        let program = Program::new(vec![Box::new(stmt)])?;
        let result = program.execute_with_ctx(&mut env)?;
        let module = Arc::clone(&result.module);
        let shared_state = Arc::new(Mutex::new(result.state));
        let factorial = {
            let state = shared_state.lock().expect("test runtime state lock");
            state.globals().iter().find_map(|value| {
                runtime_value_to_callable_shared(value, state.heap(), Arc::clone(&module), Arc::clone(&shared_state))
            })
        };
        assert!(factorial.is_some(), "factorial defined as runtime function global");
        Ok(())
    }

    #[test]
    fn test_function_with_named_params_parsing() -> Result<()> {
        let source = "fn draw_rect(x: Int, y: Int, {w: Int, h: Int? = 100}) { return x; }";
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let stmt = parser.parse_statement()?;

        if let Stmt::Function {
            name,
            params,
            param_types,
            named_params,
            ..
        } = stmt
        {
            assert_eq!(name, "draw_rect");
            assert_eq!(params, vec!["x", "y"]);
            assert_eq!(param_types.len(), 2);
            assert_eq!(named_params.len(), 2);
            assert_eq!(named_params[0].name, "w");
            assert!(matches!(named_params[0].type_annotation, Some(crate::val::Type::Int)));
            assert!(named_params[0].default.is_none());
            assert_eq!(named_params[1].name, "h");
            assert!(matches!(
                named_params[1].type_annotation,
                Some(crate::val::Type::Optional(_))
            ));
            assert!(matches!(
                named_params[1].default,
                Some(Expr::Literal(LiteralVal::Int(100)))
            ));
        } else {
            panic!("Expected Function statement with named params, got: {:?}", stmt);
        }

        Ok(())
    }

    #[test]
    fn test_function_named_params_only() -> Result<()> {
        let source = "fn configure({host: String, timeout_ms: Int? = 1000}) { }";
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let stmt = parser.parse_statement()?;

        if let Stmt::Function {
            named_params, params, ..
        } = stmt
        {
            assert!(params.is_empty());
            assert_eq!(named_params.len(), 2);
            assert_eq!(named_params[0].name, "host");
            assert_eq!(named_params[1].name, "timeout_ms");
        } else {
            panic!("Expected Function statement with named params");
        }

        Ok(())
    }

    #[test]
    fn test_function_positional_after_named_block_is_error() -> Result<()> {
        let source = "fn bad({x: Int}, y: Int) { }";
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let res = parser.parse_statement();
        assert!(res.is_err(), "Expected error when positional param follows named block");
        Ok(())
    }

    #[test]
    fn test_named_default_can_reference_positional() -> Result<()> {
        // y defaults to x + 1 when omitted
        let source = r#"
            fn f(x: Int, {y: Int = x + 1}) { return y; }
            return f(10);
        "#;
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;
        let result = run_program_default(&program)?;
        expect_return_int(&result, 11);
        Ok(())
    }

    #[test]
    fn test_named_default_can_reference_earlier_named() -> Result<()> {
        // b default references a
        let source = r#"
            fn g({a: Int, b: Int = a + 1}) { return a + b; }
            return g(a: 2);
        "#;
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;
        let result = run_program_default(&program)?;
        expect_return_int(&result, 5);
        Ok(())
    }

    #[test]
    fn test_recursive_named_function_type_check() -> Result<()> {
        let source = r#"
            fn fact({n: Int}) -> Int {
                if (n <= 1) {
                    return 1;
                }
                return fact(n: n - 1);
            }
        "#;
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let stmt = parser.parse_statement()?;
        let mut checker = TypeChecker::new();
        assert!(stmt.type_check(&mut checker).is_ok());
        Ok(())
    }
}
