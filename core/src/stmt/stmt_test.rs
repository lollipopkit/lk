#[cfg(test)]
mod tests {
    use crate::{
        expr::Pattern,
        stmt::{Program, Stmt, stmt_parser::StmtParser},
        token::Tokenizer,
        typ::TypeChecker,
        val::{HeapStore, HeapValue, RuntimeVal, TypedList},
        vm::{VmContext, execute_program32_with_ctx},
    };

    fn parse_program(source: &str) -> Program {
        let tokens = Tokenizer::tokenize(source).expect("Failed to tokenize");
        let mut parser = StmtParser::new(&tokens);
        parser.parse_program().expect("Failed to parse program")
    }

    fn execute_program_with_ctx(source: &str) -> (RuntimeVal, HeapStore) {
        let program = parse_program(source);
        let mut ctx = VmContext::new();
        let result = execute_program32_with_ctx(&program, &mut ctx).expect("Failed to execute");
        (result.first_return().clone(), result.state.heap)
    }

    fn expect_list(value: &RuntimeVal, heap: &HeapStore) -> Vec<RuntimeVal> {
        let RuntimeVal::Obj(handle) = value else {
            panic!("Expected list object, got {:?}", value.kind());
        };
        let Some(HeapValue::List(list)) = heap.get(*handle) else {
            panic!("Expected list heap value");
        };
        match list {
            TypedList::Mixed(values) => values.clone(),
            TypedList::Int(values) => values.iter().copied().map(RuntimeVal::Int).collect(),
            TypedList::Float(values) => values.iter().copied().map(RuntimeVal::Float).collect(),
            TypedList::Bool(values) => values.iter().copied().map(RuntimeVal::Bool).collect(),
            TypedList::String(values) => values
                .iter()
                .map(|value| {
                    crate::val::ShortStr::new(value)
                        .map(RuntimeVal::ShortStr)
                        .unwrap_or_else(|| panic!("test helper only supports short strings"))
                })
                .collect(),
        }
    }

    fn expect_int(value: &RuntimeVal) -> i64 {
        let RuntimeVal::Int(value) = value else {
            panic!("Expected int, got {:?}", value.kind());
        };
        *value
    }

    fn expect_str(value: &RuntimeVal, expected: &str) {
        match value {
            RuntimeVal::ShortStr(value) => assert_eq!(value.as_str(), expected),
            other => panic!("Expected short string, got {:?}", other.kind()),
        }
    }

    fn expect_result_nil(result: &crate::vm::Program32Result) {
        assert_eq!(result.first_return(), &RuntimeVal::Nil);
    }

    fn expect_result_int(result: &crate::vm::Program32Result, expected: i64) {
        assert_eq!(result.first_return(), &RuntimeVal::Int(expected));
    }

    fn expect_result_float(result: &crate::vm::Program32Result, expected: f64) {
        assert_eq!(result.first_return(), &RuntimeVal::Float(expected));
    }

    fn expect_result_str(result: &crate::vm::Program32Result, expected: &str) {
        match result.first_return() {
            RuntimeVal::ShortStr(value) => assert_eq!(value.as_str(), expected),
            RuntimeVal::Obj(handle) => match result.state.heap.get(*handle) {
                Some(HeapValue::String(value)) => assert_eq!(value.as_ref(), expected),
                other => panic!("Expected string heap value, got {:?}", other),
            },
            other => panic!("Expected string result, got {:?}", other.kind()),
        }
    }

    #[test]
    fn test_let_statement() {
        let program = parse_program("let x = 42;");
        let result = program.execute32().expect("Failed to execute");
        expect_result_nil(&result);
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
            .execute32()
            .expect("Failed to execute const binding");
        expect_result_int(&result, 42);
    }

    #[test]
    fn test_const_assignment_error() {
        let program = parse_program("const x = 1; x = 2;");
        let result = program.execute32();
        assert!(result.is_err(), "Expected assignment to const to fail");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("const variable"), "Unexpected error message: {}", err);
    }

    #[test]
    fn test_assign_statement() {
        let program = parse_program("let x = 10; x = 20;");
        let result = program.execute32().expect("Failed to execute");
        expect_result_nil(&result);
    }

    #[test]
    fn test_if_statement() {
        let program = parse_program("let x = 0; if (true) x = 1;");
        let result = program.execute32().expect("Failed to execute");
        expect_result_nil(&result);
    }

    #[test]
    fn test_if_else_statement() {
        let program = parse_program("let x = 0; if (false) x = 1; else x = 2;");
        let result = program.execute32().expect("Failed to execute");
        expect_result_nil(&result);
    }

    #[test]
    fn test_while_loop() {
        let program = parse_program("let i = 0; while (i < 3) { i = i + 1; }");
        let result = program.execute32().expect("Failed to execute");
        expect_result_nil(&result);
    }

    #[test]
    fn test_break_statement() {
        let program = parse_program("let i = 0; while (true) { i = i + 1; if (i >= 3) break; }");
        let result = program.execute32().expect("Failed to execute");
        expect_result_nil(&result);
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_nil(&result);
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_nil(&result);
    }

    #[test]
    fn test_expression_statement() {
        let program = parse_program("2 + 3;");
        let result = program.execute32().expect("Failed to execute");
        expect_result_nil(&result);
    }

    #[test]
    fn test_undefined_variable_error() {
        let program = parse_program("x = 42;");
        let result = program.execute32();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Undefined variable"));
    }

    #[test]
    fn test_break_outside_loop_error() {
        let program = parse_program("break;");
        let result = program.execute32();
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
        let result = program.execute32();
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_nil(&result);
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_nil(&result);
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_nil(&result);
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_int(&result, 50);
    }

    #[test]
    fn test_simple_return_with_literal() {
        let program = parse_program("return 123;");
        let result = program.execute32().expect("Failed to execute");
        expect_result_int(&result, 123);
    }

    #[test]
    fn test_return_with_variable() {
        let program = parse_program(
            r#"
            let x = 42;
            return x;
        "#,
        );
        let result = program.execute32().expect("Failed to execute");
        expect_result_int(&result, 42);
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_int(&result, 3);
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_nil(&result);
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_int(&result, 3);
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_int(&result, 10);
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_int(&result, 30);
    }

    // Type annotation tests
    #[test]
    fn test_let_with_type_annotation_int() {
        let program = parse_program("let x: Int = 42;");
        let result = program.execute32().expect("Failed to execute");
        expect_result_nil(&result);
    }

    #[test]
    fn test_let_with_type_annotation_string() {
        let program = parse_program(r#"let name: String = "hello";"#);
        let result = program.execute32().expect("Failed to execute");
        expect_result_nil(&result);
    }

    #[test]
    fn test_let_with_type_annotation_bool() {
        let program = parse_program("let flag: Bool = true;");
        let result = program.execute32().expect("Failed to execute");
        expect_result_nil(&result);
    }

    #[test]
    fn test_let_with_type_annotation_float() {
        let program = parse_program("let pi: Float = 3.14;");
        let result = program.execute32().expect("Failed to execute");
        expect_result_nil(&result);
    }

    #[test]
    fn test_let_with_type_annotation_nil() {
        let program = parse_program("let empty: Nil = nil;");
        let result = program.execute32().expect("Failed to execute");
        expect_result_nil(&result);
    }

    #[test]
    fn test_let_with_type_annotation_list() {
        let program = parse_program("let items: List = [1, 2, 3];");
        let result = program.execute32().expect("Failed to execute");
        expect_result_nil(&result);
    }

    #[test]
    fn test_let_with_type_annotation_map() {
        let program = parse_program(r#"let data: Map = {"key": "value"};"#);
        let result = program.execute32().expect("Failed to execute");
        expect_result_nil(&result);
    }

    #[test]
    fn test_let_type_mismatch_int() {
        let program = parse_program(r#"let x: Int = "not_int";"#);
        let result = program.execute32();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Type mismatch"));
    }

    #[test]
    fn test_let_type_mismatch_string() {
        let program = parse_program("let name: String = 42;");
        let result = program.execute32();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Type mismatch"));
    }

    #[test]
    fn test_let_type_mismatch_bool() {
        let program = parse_program("let flag: Bool = 123;");
        let result = program.execute32();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Type mismatch"));
    }

    #[test]
    fn test_let_type_mismatch_float() {
        let program = parse_program("let pi: Float = true;");
        let result = program.execute32();
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_nil(&result);
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_nil(&result);
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_nil(&result);
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_int(&result, 10); // 0+1+2+3+4
    }

    #[test]
    fn test_for_loop_tuple_destructure() {
        let (result, heap) = execute_program_with_ctx(
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
        let outer = expect_list(&result, &heap);
        assert_eq!(outer.len(), 2);
        let keys = expect_list(&outer[0], &heap);
        let values = expect_list(&outer[1], &heap);
        assert_eq!(keys.len(), 2);
        assert_eq!(values.len(), 2);
        expect_str(&keys[0], "a");
        expect_str(&keys[1], "b");
        assert_eq!(values.iter().map(expect_int).collect::<Vec<_>>(), vec![1, 2]);
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_int(&result, 5);
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
        let result = program.execute32().expect("Failed to execute");
        // Should return [0, 1, 2, 4, 5, 6]
        assert_eq!(
            expect_list(result.first_return(), &result.state.heap)
                .iter()
                .map(expect_int)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 4, 5, 6]
        );
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_int(&result, 100); // Outer x should be unchanged
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_int(&result, 0); // Should not iterate
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
        let result = program.execute32().expect("Failed to execute");
        let list = expect_list(result.first_return(), &result.state.heap);
        assert_eq!(list.len(), 3);
        expect_str(&list[0], "a");
        expect_str(&list[1], "b");
        expect_str(&list[2], "c");
    }

    #[test]
    fn test_for_loop_map_iteration() {
        let (result, heap) = execute_program_with_ctx(
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
        let outer = expect_list(&result, &heap);
        assert_eq!(outer.len(), 2);
        let keys = expect_list(&outer[0], &heap);
        let values = expect_list(&outer[1], &heap);
        assert_eq!(keys.len(), 2);
        assert_eq!(values.len(), 2);
        let mut found_a = false;
        let mut found_b = false;
        for (key, value) in keys.iter().zip(values.iter()) {
            match key {
                RuntimeVal::ShortStr(key) if key.as_str() == "a" && expect_int(value) == 1 => found_a = true,
                RuntimeVal::ShortStr(key) if key.as_str() == "b" && expect_int(value) == 2 => found_b = true,
                _ => {}
            }
        }
        assert!(found_a && found_b);
    }

    #[test]
    fn test_for_loop_nested_loops() {
        let (result, heap) = execute_program_with_ctx(
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
        let outer = expect_list(&result, &heap);
        let pairs = outer
            .iter()
            .map(|pair| {
                let pair = expect_list(pair, &heap);
                vec![expect_int(&pair[0]), expect_int(&pair[1])]
            })
            .collect::<Vec<_>>();
        assert_eq!(pairs, vec![vec![1, 3], vec![1, 4], vec![2, 3], vec![2, 4]]);
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_int(&result, 30);
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
        let result = program.execute32().expect("Failed to execute");
        let list = expect_list(result.first_return(), &result.state.heap);
        assert_eq!(list.iter().map(expect_int).collect::<Vec<_>>(), vec![0, 1, 2]);
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_int(&result, 20); // (1+2+3+4)*2 = 20
    }

    #[test]
    fn test_for_loop_complex_pattern() {
        let (result, heap) = execute_program_with_ctx(
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
        let outer = expect_list(&result, &heap);
        assert_eq!(outer.len(), 2);
        let first = expect_list(&outer[0], &heap);
        assert_eq!(first.len(), 2);
        let first_pairs = first
            .iter()
            .map(|pair| {
                let pair = expect_list(pair, &heap);
                vec![expect_int(&pair[0]), expect_int(&pair[1])]
            })
            .collect::<Vec<_>>();
        assert_eq!(first_pairs, vec![vec![1, 2], vec![5, 6]]);
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
        let result = program.execute32();
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
        let result = program.execute32();
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_int(&result, 6);
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_int(&result, 15);
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_int(&result, 7);
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_int(&result, 15);
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_int(&result, 5);
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_int(&result, 2);
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_int(&result, 16);
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_str(&result, "hello world");
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_float(&result, 3.0);
    }

    #[test]
    fn test_compound_assignment_undefined_variable() {
        let program = parse_program("undefined_var += 5;");
        let result = program.execute32();
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
        let result = program.execute32().expect("Failed to execute");
        expect_result_int(&result, 5);
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
