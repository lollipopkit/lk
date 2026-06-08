#[cfg(test)]
mod tests {
    use crate::{
        val::{RuntimeVal, ShortStr},
        vm::{ProgramResult, execute_source},
    };

    fn parse_and_execute_stmt(stmt_code: &str) -> anyhow::Result<ProgramResult> {
        execute_source(stmt_code)
    }

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
    fn test_if_let_simple_variable() {
        let result = parse_and_execute_stmt("let data = 42; if let x = data { return x; }").unwrap();

        expect_return_int(&result, 42);
    }

    #[test]
    fn test_if_let_literal_match() {
        let result =
            parse_and_execute_stmt(r#"let status = "ok"; if let "ok" = status { return 1; } else { return 0; }"#)
                .unwrap();

        expect_return_int(&result, 1);
    }

    #[test]
    fn test_if_let_literal_no_match() {
        let result =
            parse_and_execute_stmt(r#"let status = "error"; if let "ok" = status { return 1; } else { return 0; }"#)
                .unwrap();

        expect_return_int(&result, 0);
    }

    #[test]
    fn test_if_let_list_destructuring() {
        let result = parse_and_execute_stmt(
            "let list = [1, 2, 3]; if let [first, second, third] = list { return first + second + third; }",
        )
        .unwrap();

        expect_return_int(&result, 6);
    }

    #[test]
    fn test_if_let_list_with_rest() {
        let result =
            parse_and_execute_stmt("let list = [1, 2, 3, 4]; if let [first, ..rest] = list { return first; }").unwrap();

        expect_return_int(&result, 1);
    }

    #[test]
    fn test_if_let_map_destructuring() {
        let result = parse_and_execute_stmt(
            r#"let user = {"name": "Alice", "age": 30}; if let {"name": name} = user { return name; }"#,
        )
        .unwrap();

        expect_return_str(&result, "Alice");
    }

    #[test]
    fn test_if_let_wildcard() {
        let result = parse_and_execute_stmt("let data = 42; if let _ = data { return 1; } else { return 0; }").unwrap();

        expect_return_int(&result, 1);
    }

    #[test]
    fn test_if_let_nested_pattern() {
        let result = parse_and_execute_stmt(
            r#"let data = {"items": ["first", "second"]}; if let {"items": [first, second]} = data { return first; }"#,
        )
        .unwrap();

        expect_return_str(&result, "first");
    }

    #[test]
    fn test_if_let_or_pattern() {
        let result = parse_and_execute_stmt(
            "let status = 200; if let 200 | 201 | 202 = status { return 1; } else { return 0; }",
        )
        .unwrap();

        expect_return_int(&result, 1);
    }

    #[test]
    fn test_if_let_guard_pattern() {
        let result =
            parse_and_execute_stmt("let value = 15; if let x if x > 10 = value { return x; } else { return 0; }")
                .unwrap();

        expect_return_int(&result, 15);
    }

    #[test]
    fn test_if_let_guard_pattern_no_match() {
        let result =
            parse_and_execute_stmt("let value = 5; if let x if x > 10 = value { return x; } else { return 0; }")
                .unwrap();

        expect_return_int(&result, 0);
    }

    #[test]
    fn test_if_let_range_pattern() {
        let result =
            parse_and_execute_stmt(r#"let age = 25; if let 18..65 = age { return "adult"; } else { return "other"; }"#)
                .unwrap();

        expect_return_str(&result, "adult");
    }

    #[test]
    fn test_if_let_variable_scoping() {
        // Variable should only be accessible within the if let block
        let result = parse_and_execute_stmt(
            r#"
            let data = 42;
            let x = "outer";
            if let y = data {
                return y;
            }
            return x;
            "#,
        )
        .unwrap();

        expect_return_int(&result, 42);
    }

    #[test]
    fn test_if_let_complex_expression() {
        let result = parse_and_execute_stmt(
            r#"let data = [{"id": 1, "value": "test"}]; if let [{"id": id, "value": value}] = data { return value; }"#,
        )
        .unwrap();

        expect_return_str(&result, "test");
    }

    #[test]
    fn test_if_let_no_else_branch() {
        let result = parse_and_execute_stmt("let data = nil; if let 42 = data { return 1; }").unwrap();

        expect_return_nil(&result); // No match, no else, returns nil
    }
}
