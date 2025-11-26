#[cfg(test)]
mod tests {
    use crate::{
        expr::{Expr, Pattern},
        stmt::{Program, Stmt, run_program_default, stmt_parser::StmtParser},
        token::Tokenizer,
        val::Val,
    };
    use std::sync::Arc;

    fn parse_program(source: &str) -> Program {
        let tokens = Tokenizer::tokenize(source).expect("Failed to tokenize");
        let mut parser = StmtParser::new(&tokens);
        parser.parse_program().expect("Failed to parse program")
    }

    #[test]
    fn test_destructuring_single_variable() {
        // Test basic single variable binding (should work like before)
        let program = r#"
            let x = 42;
            return x;
        "#;

        let program = parse_program(program);
        let result = run_program_default(&program).unwrap();

        assert_eq!(result, Val::Int(42));
    }

    #[test]
    fn test_destructuring_array() {
        // Test array destructuring
        let program = r#"
            let [first, second, third] = [1, 2, 3];
            return first + second + third;
        "#;

        let program = parse_program(program);
        let result = run_program_default(&program).unwrap();

        assert_eq!(result, Val::Int(6));
    }

    #[test]
    fn test_destructuring_array_with_rest() {
        // Test array destructuring with rest pattern
        let program = r#"
            let [first, second, ..rest] = [1, 2, 3, 4, 5];
            return first + second;
        "#;

        let program = parse_program(program);
        let result = run_program_default(&program).unwrap();

        assert_eq!(result, Val::Int(3)); // 1 + 2
    }

    #[test]
    fn test_destructuring_object() {
        // Test object/map destructuring
        let program = r#"
            let {"name": name, "age": age} = {"name": "Alice", "age": 30, "city": "NYC"};
            return name;
        "#;

        let program = parse_program(program);
        let result = run_program_default(&program).unwrap();

        assert_eq!(result, Val::Str(Arc::from("Alice")));
    }

    #[test]
    fn test_destructuring_object_with_rest() {
        // Test object/map destructuring with rest pattern
        let program = r#"
            let {"name": name, ..rest} = {"name": "Bob", "age": 25, "city": "LA"};
            return name;
        "#;

        let program = parse_program(program);
        let result = run_program_default(&program).unwrap();

        assert_eq!(result, Val::Str(Arc::from("Bob")));
    }

    #[test]
    fn test_destructuring_nested_patterns() {
        // Test nested destructuring
        let program = r#"
            let [first, {"data": value}] = [1, {"data": 42, "meta": "info"}];
            return first + value;
        "#;

        let program = parse_program(program);
        let result = run_program_default(&program).unwrap();

        assert_eq!(result, Val::Int(43));
    }

    #[test]
    fn test_destructuring_wildcard() {
        // Test wildcard pattern
        let program = r#"
            let [_, value, _] = [10, 20, 30];
            return value;
        "#;

        let program = parse_program(program);
        let result = run_program_default(&program).unwrap();

        assert_eq!(result, Val::Int(20));
    }

    #[test]
    fn test_destructuring_mismatch_error() {
        // Test pattern mismatch error
        let program = r#"
            let [x, y, z] = [1, 2]; // Not enough elements
            return x;
        "#;

        let program = parse_program(program);
        let result = run_program_default(&program);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Pattern does not match"));
    }

    #[test]
    fn test_destructuring_with_type_annotation() {
        // Test destructuring with type annotation
        let program = r#"
            let [x, y]: List<Int> = [1, 2];
            return x + y;
        "#;

        let program = parse_program(program);
        let result = run_program_default(&program).unwrap();

        assert_eq!(result, Val::Int(3));
    }

    #[test]
    fn test_destructuring_string_as_array() {
        // Test destructuring string as array of characters
        let program = r#"
            let [first, second, ..rest] = "hello";
            return first + second;
        "#;

        let program = parse_program(program);
        let result = run_program_default(&program).unwrap();

        // Should concatenate first two characters
        match result {
            Val::Str(s) => assert_eq!(s.as_ref(), "he"),
            _ => panic!("Expected string result"),
        }
    }

    #[test]
    fn test_destructuring_or_pattern() {
        // Test OR pattern in destructuring
        let program = r#"
            let [1 | 2, value] = [2, 42];
            return value;
        "#;

        let program = parse_program(program);
        let result = run_program_default(&program).unwrap();

        assert_eq!(result, Val::Int(42));
    }

    #[test]
    fn test_destructuring_empty_array() {
        // Test destructuring empty array
        let program = r#"
            let [] = [];
            return "success";
        "#;

        let program = parse_program(program);
        let result = run_program_default(&program).unwrap();

        assert_eq!(result, Val::Str(Arc::from("success")));
    }

    #[test]
    fn test_destructuring_empty_object() {
        // Test destructuring empty object
        let program = r#"
            let {} = {};
            return "success";
        "#;

        let program = parse_program(program);
        let result = run_program_default(&program).unwrap();

        assert_eq!(result, Val::Str(Arc::from("success")));
    }

    #[test]
    fn test_destructuring_display_formatting() {
        // Test that destructuring statements display correctly
        let stmt = Stmt::Let {
            pattern: Pattern::List {
                patterns: vec![
                    Pattern::Variable("first".to_string()),
                    Pattern::Variable("second".to_string()),
                ],
                rest: Some("rest".to_string()),
            },
            type_annotation: None,
            value: Box::new(Expr::Val(Val::List(Arc::from(vec![
                Val::Int(1),
                Val::Int(2),
                Val::Int(3),
            ])))),
            span: None,
            is_const: false,
        };

        let display_str = format!("{}", stmt);
        println!("Actual display: {}", display_str);
        // Check for the key components rather than exact format
        assert!(display_str.contains("let"));
        assert!(display_str.contains("first"));
        assert!(display_str.contains("second"));
        assert!(display_str.contains("rest"));
        assert!(display_str.contains("1"));
        assert!(display_str.contains("2"));
        assert!(display_str.contains("3"));
    }

    #[test]
    fn test_destructuring_complex_pattern_display() {
        // Test complex pattern display
        let stmt = Stmt::Let {
            pattern: Pattern::Map {
                patterns: vec![
                    ("name".to_string(), Pattern::Variable("name".to_string())),
                    (
                        "age".to_string(),
                        Pattern::Range {
                            start: Box::new(Expr::Val(Val::Int(0))),
                            end: Box::new(Expr::Val(Val::Int(120))),
                            inclusive: true,
                        },
                    ),
                ],
                rest: None,
            },
            type_annotation: None,
            value: Box::new(Expr::Val(Val::Map(Arc::new(Default::default())))),
            span: None,
            is_const: false,
        };

        let display_str = format!("{}", stmt);
        assert!(display_str.contains("let {\"name\": name, \"age\": 0..=120} = {};"));
    }
}
