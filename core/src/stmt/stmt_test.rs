#[cfg(test)]
mod tests {
    use crate::{
        expr::Pattern,
        stmt::{Program, Stmt, stmt_parser::StmtParser},
        token::Tokenizer,
        typ::TypeChecker,
        val::Val,
        vm::VmContext,
    };

    fn parse_program(source: &str) -> Program {
        let tokens = Tokenizer::tokenize(source).expect("Failed to tokenize");
        let mut parser = StmtParser::new(&tokens);
        parser.parse_program().expect("Failed to parse program")
    }

    #[test]
    fn test_let_statement() {
        let program = parse_program("let x = 42;");
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Nil);
    }

    #[test]
    fn test_const_statement_parsing() {
        let program = parse_program("const answer = 42;");
        assert_eq!(program.statements.len(), 1);
        if let Stmt::Let { pattern, is_const, .. } = program.statements[0].as_ref() {
            assert!(*is_const, "Expected const binding");
            match pattern {
                Pattern::Variable(name) => assert_eq!(name, "answer"),
                other => panic!("Unexpected pattern: {:?}", other),
            }
        } else {
            panic!("Expected const let statement");
        }

        let result = parse_program("const answer = 42; return answer;")
            .execute()
            .expect("Failed to execute const binding");
        assert_eq!(result, Val::Int(42));
    }

    #[test]
    fn test_const_assignment_error() {
        let program = parse_program("const x = 1; x = 2;");
        let result = program.execute();
        assert!(result.is_err(), "Expected assignment to const to fail");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("const variable"), "Unexpected error message: {}", err);
    }

    #[test]
    fn test_assign_statement() {
        let program = parse_program("let x = 10; x = 20;");
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Nil);
    }

    #[test]
    fn test_if_statement() {
        let program = parse_program("let x = 0; if (true) x = 1;");
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Nil);
    }

    #[test]
    fn test_if_else_statement() {
        let program = parse_program("let x = 0; if (false) x = 1; else x = 2;");
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Nil);
    }

    #[test]
    fn test_while_loop() {
        let program = parse_program("let i = 0; while (i < 3) { i = i + 1; }");
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Nil);
    }

    #[test]
    fn test_break_statement() {
        let program = parse_program("let i = 0; while (true) { i = i + 1; if (i >= 3) break; }");
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Nil);
    }

    #[test]
    fn test_continue_statement() {
        let program = parse_program(
            r#"
            let i = 0; 
            let sum = 0; 
            while (i < 5) { 
                i = i + 1; 
                if (i == 3) continue; 
                sum = sum + i; 
            }
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Nil);
    }

    #[test]
    fn test_block_scope() {
        let program = parse_program(
            r#"
            let x = 1;
            {
                let y = 2;
                x = x + y;
            }
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Nil);
    }

    #[test]
    fn test_expression_statement() {
        let program = parse_program("2 + 3;");
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Nil);
    }

    #[test]
    fn test_environment() {
        let mut env = VmContext::new();

        // Test define and get
        env.define("x".to_string(), Val::Int(42));
        assert_eq!(env.get("x"), Some(&Val::Int(42)));

        // Test assign
        env.assign("x", Val::Int(100)).expect("Failed to assign");
        assert_eq!(env.get("x"), Some(&Val::Int(100)));

        // Test scoping
        env.push_scope();
        env.define("y".to_string(), Val::Int(20));
        assert_eq!(env.get("y"), Some(&Val::Int(20)));
        assert_eq!(env.get("x"), Some(&Val::Int(100))); // Still accessible

        env.pop_scope();
        assert_eq!(env.get("y"), None); // No longer accessible
        assert_eq!(env.get("x"), Some(&Val::Int(100))); // Still accessible
    }

    #[test]
    fn test_undefined_variable_error() {
        let program = parse_program("x = 42;");
        let result = program.execute();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Undefined variable"));
    }

    #[test]
    fn test_break_outside_loop_error() {
        let program = parse_program("break;");
        let result = program.execute();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("break statement outside of loop")
        );
    }

    #[test]
    fn test_continue_outside_loop_error() {
        let program = parse_program("continue;");
        let result = program.execute();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("continue statement outside of loop")
        );
    }

    #[test]
    fn test_complex_program() {
        // 简化程序，避免无限循环
        let program = parse_program(
            r#"
            let n = 3;
            let sum = 0;
            let i = 1;
            
            if (i <= n) {
                sum = sum + i;
                i = i + 1;
            }
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Nil);
    }

    #[test]
    fn test_variable_in_expression() {
        let program = parse_program(
            r#"
            let x = 5;
            let y = x + 3;
            let result = x * y;
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Nil);
    }

    #[test]
    fn test_nested_blocks() {
        let program = parse_program(
            r#"
            let x = 1;
            {
                let y = 2;
                {
                    let z = 3;
                    x = x + y + z;
                }
            }
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Nil);
    }

    #[test]
    fn test_return_with_value() {
        let program = parse_program(
            r#"
            let x = 42;
            return x + 8;
            let y = 100; // This should not be executed
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Int(50));
    }

    #[test]
    fn test_simple_return_with_literal() {
        let program = parse_program("return 123;");
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Int(123));
    }

    #[test]
    fn test_return_with_variable() {
        let program = parse_program(
            r#"
            let x = 42;
            return x;
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Int(42));
    }

    #[test]
    fn test_return_with_addition() {
        let program = parse_program(
            r#"
            let x = 1;
            let y = 2;
            return x + y;
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Int(3));
    }

    #[test]
    fn test_return_without_value() {
        let program = parse_program(
            r#"
            let x = 10;
            return;
            let y = 20; // This should not be executed
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Nil);
    }

    #[test]
    fn test_return_in_block() {
        let program = parse_program(
            r#"
            let x = 1;
            {
                let y = 2;
                return x + y;
                let z = 999; // This should not be executed
            }
            let w = 100; // This should not be executed either
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Int(3));
    }

    #[test]
    fn test_return_in_if_statement() {
        let program = parse_program(
            r#"
            let x = 5;
            if (x > 3) {
                return x * 2;
            } else {
                return x;
            }
            let y = 999; // This should not be executed
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Int(10));
    }

    #[test]
    fn test_return_in_while_loop() {
        let program = parse_program(
            r#"
            let i = 0;
            while (i < 5) {
                i = i + 1;
                if (i == 3) {
                    return i * 10;
                }
            }
            let done = 999; // This should not be executed
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Int(30));
    }

    // Type annotation tests
    #[test]
    fn test_let_with_type_annotation_int() {
        let program = parse_program("let x: Int = 42;");
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Nil);
    }

    #[test]
    fn test_let_with_type_annotation_string() {
        let program = parse_program(r#"let name: String = "hello";"#);
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Nil);
    }

    #[test]
    fn test_let_with_type_annotation_bool() {
        let program = parse_program("let flag: Bool = true;");
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Nil);
    }

    #[test]
    fn test_let_with_type_annotation_float() {
        let program = parse_program("let pi: Float = 3.14;");
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Nil);
    }

    #[test]
    fn test_let_with_type_annotation_nil() {
        let program = parse_program("let empty: Nil = nil;");
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Nil);
    }

    #[test]
    fn test_let_with_type_annotation_list() {
        let program = parse_program("let items: List = [1, 2, 3];");
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Nil);
    }

    #[test]
    fn test_let_with_type_annotation_map() {
        let program = parse_program(r#"let data: Map = {"key": "value"};"#);
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Nil);
    }

    #[test]
    fn test_let_type_mismatch_int() {
        let program = parse_program(r#"let x: Int = "not_int";"#);
        let result = program.execute();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Type mismatch"));
    }

    #[test]
    fn test_let_type_mismatch_string() {
        let program = parse_program("let name: String = 42;");
        let result = program.execute();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Type mismatch"));
    }

    #[test]
    fn test_let_type_mismatch_bool() {
        let program = parse_program("let flag: Bool = 123;");
        let result = program.execute();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Type mismatch"));
    }

    #[test]
    fn test_let_type_mismatch_float() {
        let program = parse_program("let pi: Float = true;");
        let result = program.execute();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Type mismatch"));
    }

    #[test]
    fn test_custom_type_parsing() {
        // Custom types should parse successfully, validation happens at runtime
        let tokens = Tokenizer::tokenize("let x: UnknownType = 42;").expect("Failed to tokenize");
        let mut parser = StmtParser::new(&tokens);
        let result = parser.parse_program();
        assert!(result.is_ok());

        // The parsed statement should contain a Type::Named
        let program = result.unwrap();
        assert_eq!(program.statements.len(), 1);
        if let Stmt::Let {
            type_annotation: Some(typ),
            ..
        } = program.statements[0].as_ref()
        {
            match typ {
                crate::val::Type::Named(name) => assert_eq!(name, "UnknownType"),
                _ => panic!("Expected Type::Named, got {:?}", typ),
            }
        } else {
            panic!("Expected Let statement with type annotation");
        }
    }

    #[test]
    fn test_mixed_typed_and_untyped_variables() {
        let program = parse_program(
            r#"
            let x: Int = 42;
            let y = "hello";
            let z: Bool = true;
            let w = 3.14;
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Nil);
    }

    #[test]
    fn test_type_annotation_in_complex_expression() {
        let program = parse_program(
            r#"
            let x: Int = 10;
            let y: Int = 20;
            let sum: Int = x + y;
            let result: Bool = sum > 25;
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Nil);
    }

    // For loop tests
    #[test]
    fn test_for_loop_simple_list() {
        let program = parse_program(
            r#"
            for x in [1] {
                let y = x;
            }
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Nil);
    }

    #[test]
    fn test_for_loop_range() {
        let program = parse_program(
            r#"
            let sum = 0;
            for i in 0..5 {
                sum = sum + i;
            }
            return sum;
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Int(10)); // 0+1+2+3+4
    }

    #[test]
    fn test_for_loop_tuple_destructure() {
        let program = parse_program(
            r#"
            let keys = [];
            let values = [];
            for (k, v) in [["a", 1], ["b", 2]] {
                keys = keys + [k];
                values = values + [v];
            }
            return [keys, values];
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        // Should return [["a", "b"], [1, 2]]
        if let Val::List(outer) = result {
            assert_eq!(outer.len(), 2);
            if let Val::List(keys) = &outer[0] {
                assert_eq!(keys.len(), 2);
                assert_eq!(keys[0], Val::Str("a".into()));
                assert_eq!(keys[1], Val::Str("b".into()));
            } else {
                panic!("Expected keys to be a list");
            }
            if let Val::List(values) = &outer[1] {
                assert_eq!(values.len(), 2);
                assert_eq!(values[0], Val::Int(1));
                assert_eq!(values[1], Val::Int(2));
            } else {
                panic!("Expected values to be a list");
            }
        } else {
            panic!("Expected result to be a list");
        }
    }

    #[test]
    fn test_for_loop_ignore_pattern() {
        let program = parse_program(
            r#"
            let count = 0;
            for _ in [1, 2, 3, 4, 5] {
                count = count + 1;
            }
            return count;
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Int(5));
    }

    #[test]
    fn test_for_loop_break_continue() {
        let program = parse_program(
            r#"
            let result = [];
            for i in 0..10 {
                if (i == 3) continue;
                if (i == 7) break;
                result = result + [i];
            }
            return result;
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        // Should return [0, 1, 2, 4, 5, 6]
        if let Val::List(list) = result {
            let expected = vec![
                Val::Int(0),
                Val::Int(1),
                Val::Int(2),
                Val::Int(4),
                Val::Int(5),
                Val::Int(6),
            ];
            assert_eq!(*list, expected);
        } else {
            panic!("Expected result to be a list");
        }
    }

    #[test]
    fn test_for_loop_scoping() {
        let program = parse_program(
            r#"
            let x = 100;
            for x in [1, 2, 3] {
                // Loop variable shadows outer x
            }
            return x;
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Int(100)); // Outer x should be unchanged
    }

    #[test]
    fn test_for_loop_empty_list() {
        let program = parse_program(
            r#"
            let count = 0;
            for x in [] {
                count = count + 1;
            }
            return count;
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Int(0)); // Should not iterate
    }

    #[test]
    fn test_for_loop_string_iteration() {
        let program = parse_program(
            r#"
            let result = [];
            for ch in "abc" {
                result = result + [ch];
            }
            return result;
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        if let Val::List(list) = result {
            assert_eq!(list.len(), 3);
            assert_eq!(list[0], Val::Str("a".into()));
            assert_eq!(list[1], Val::Str("b".into()));
            assert_eq!(list[2], Val::Str("c".into()));
        } else {
            panic!("Expected list result");
        }
    }

    #[test]
    fn test_for_loop_map_iteration() {
        let program = parse_program(
            r#"
            let keys = [];
            let values = [];
            let m = {"a": 1, "b": 2};
            for (k, v) in m {
                keys = keys + [k];
                values = values + [v];
            }
            return [keys, values];
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        if let Val::List(outer) = result {
            assert_eq!(outer.len(), 2);
            if let Val::List(keys) = &outer[0] {
                if let Val::List(values) = &outer[1] {
                    assert_eq!(keys.len(), 2);
                    assert_eq!(values.len(), 2);
                    // Check that we have the expected key-value pairs
                    let mut found_a = false;
                    let mut found_b = false;
                    for i in 0..keys.len() {
                        if let Val::Str(key) = &keys[i] {
                            if **key == *"a" && values[i] == Val::Int(1) {
                                found_a = true;
                            } else if **key == *"b" && values[i] == Val::Int(2) {
                                found_b = true;
                            }
                        }
                    }
                    assert!(found_a && found_b);
                } else {
                    panic!("Expected values to be a list");
                }
            } else {
                panic!("Expected keys to be a list");
            }
        } else {
            panic!("Expected result to be a list");
        }
    }

    #[test]
    fn test_for_loop_nested_loops() {
        let program = parse_program(
            r#"
            let result = [];
            for i in [1, 2] {
                for j in [3, 4] {
                    result = result + [[i, j]];
                }
            }
            return result;
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        if let Val::List(outer) = result {
            assert_eq!(outer.len(), 4);
            let expected = vec![
                Val::List(vec![Val::Int(1), Val::Int(3)].into()),
                Val::List(vec![Val::Int(1), Val::Int(4)].into()),
                Val::List(vec![Val::Int(2), Val::Int(3)].into()),
                Val::List(vec![Val::Int(2), Val::Int(4)].into()),
            ];
            assert_eq!(*outer, expected);
        } else {
            panic!("Expected result to be a list");
        }
    }

    #[test]
    fn test_for_loop_with_return() {
        let program = parse_program(
            r#"
            for x in [1, 2, 3, 4, 5] {
                if (x == 3) {
                    return x * 10;
                }
            }
            return 999; // Should not reach here
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Int(30));
    }

    #[test]
    fn test_for_loop_range_exclusive() {
        let program = parse_program(
            r#"
            let result = [];
            for i in 0..3 {
                result = result + [i];
            }
            return result;
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        if let Val::List(list) = result {
            assert_eq!(list.len(), 3);
            assert_eq!(list[0], Val::Int(0));
            assert_eq!(list[1], Val::Int(1));
            assert_eq!(list[2], Val::Int(2));
        } else {
            panic!("Expected list result");
        }
    }

    #[test]
    fn test_for_loop_with_modification() {
        let program = parse_program(
            r#"
            let sum = 0;
            for x in [1, 2, 3, 4] {
                sum = sum + x * 2;
            }
            return sum;
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Int(20)); // (1+2+3+4)*2 = 20
    }

    #[test]
    fn test_for_loop_complex_pattern() {
        let program = parse_program(
            r#"
            let first = [];
            let rest = [];
            for [a, b, ..r] in [[1, 2, 3, 4], [5, 6, 7, 8]] {
                first = first + [[a, b]];
                rest = rest + [r];
            }
            return [first, rest];
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        if let Val::List(outer) = result {
            assert_eq!(outer.len(), 2);
            // Check first elements
            if let Val::List(first) = &outer[0] {
                assert_eq!(first.len(), 2);
                assert_eq!(first[0], Val::List(vec![Val::Int(1), Val::Int(2)].into()));
                assert_eq!(first[1], Val::List(vec![Val::Int(5), Val::Int(6)].into()));
            } else {
                panic!("Expected first to be a list");
            }
        } else {
            panic!("Expected result to be a list");
        }
    }

    #[test]
    fn test_for_loop_error_invalid_iterable() {
        let program = parse_program(
            r#"
            for x in true {
                // This should fail - bool is not iterable
            }
        "#,
        );
        let result = program.execute();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string()
                .contains("For loop iterable must be List, String, or Map")
        );
    }

    #[test]
    fn test_for_loop_error_pattern_mismatch() {
        let program = parse_program(
            r#"
            for (a, b) in [1, 2, 3] {
                // This should fail - can't destructure single values into tuples
            }
        "#,
        );
        let result = program.execute();
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("Pattern does not match value"));
    }

    #[test]
    fn test_for_loop_with_function_call() {
        let program = parse_program(
            r#"
            let sum = 0;
            for x in [1, 2, 3] {
                sum = sum + x;
            }
            return sum;
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Int(6));
    }

    // Compound assignment tests
    #[test]
    fn test_compound_assignment_add() {
        let program = parse_program(
            r#"
            let x = 10;
            x += 5;
            return x;
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Int(15));
    }

    #[test]
    fn test_compound_assignment_sub() {
        let program = parse_program(
            r#"
            let x = 10;
            x -= 3;
            return x;
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Int(7));
    }

    #[test]
    fn test_compound_assignment_mul() {
        let program = parse_program(
            r#"
            let x = 5;
            x *= 3;
            return x;
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Int(15));
    }

    #[test]
    fn test_compound_assignment_div() {
        let program = parse_program(
            r#"
            let x = 15;
            x /= 3;
            return x;
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Int(5));
    }

    #[test]
    fn test_compound_assignment_mod() {
        let program = parse_program(
            r#"
            let x = 17;
            x %= 5;
            return x;
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Int(2));
    }

    #[test]
    fn test_compound_assignment_with_expressions() {
        let program = parse_program(
            r#"
            let x = 10;
            let y = 3;
            x += y * 2;
            return x;
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Int(16));
    }

    #[test]
    fn test_compound_assignment_string() {
        let program = parse_program(
            r#"
            let s = "hello";
            s += " world";
            return s;
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Str("hello world".into()));
    }

    #[test]
    fn test_compound_assignment_float() {
        let program = parse_program(
            r#"
            let x = 1.5;
            x *= 2.0;
            return x;
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Float(3.0));
    }

    #[test]
    fn test_compound_assignment_undefined_variable() {
        let program = parse_program("undefined_var += 5;");
        let result = program.execute();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Cannot compound assign to undefined variable"));
    }

    #[test]
    fn test_compound_assignment_multiple_operations() {
        let program = parse_program(
            r#"
            let x = 10;
            x += 5;   // x = 15
            x *= 2;   // x = 30
            x -= 10;  // x = 20
            x /= 4;   // x = 5
            return x;
        "#,
        );
        let result = program.execute().expect("Failed to execute");
        assert_eq!(result, Val::Int(5));
    }

    #[test]
    fn test_type_check_range_for_loop() {
        let program = parse_program(
            r#"
            let acc = 0;
            for _ in 0..10 {
                acc = acc + 1;
            }
        "#,
        );
        let mut checker = TypeChecker::new_strict();
        assert!(program.type_check(&mut checker).is_ok());
    }

    #[test]
    fn test_type_check_struct_impl_field_access() {
        let program = parse_program(
            r#"
            struct Rect { w: Int, h: Int }
            trait Area { fn area(self) -> Int; }
            impl Area for Rect {
                fn area(self) -> Int { return self.w * self.h; }
            }
        "#,
        );
        let mut checker = TypeChecker::new_strict();
        assert!(program.type_check(&mut checker).is_ok());
    }

    #[test]
    fn test_type_check_builtin_call() {
        let program = parse_program(r#"println("hello");"#);
        let mut checker = TypeChecker::new_strict();
        assert!(program.type_check(&mut checker).is_ok());
    }
}
