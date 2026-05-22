#[cfg(test)]
mod tests {
    use crate::{
        stmt::{StmtParser, run_program},
        token::Tokenizer,
        val::{HeapStore, RuntimeVal, ShortStr, Val, val_to_runtime_val},
        vm::{Program32Result, VmContext},
    };
    use std::collections::HashMap;

    fn parse_and_execute_stmt(stmt_code: &str, ctx: &Val) -> Result<Program32Result, anyhow::Error> {
        let tokens = Tokenizer::tokenize(stmt_code)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;
        let mut env = VmContext::new();
        if let Some(m) = ctx.as_map() {
            let mut heap = HeapStore::new();
            for (k, v) in m.iter() {
                let value = val_to_runtime_val(v, &mut heap)?;
                env.define_runtime_value(k.to_string(), value, heap.clone());
            }
        }
        run_program(&program, &mut env)
    }

    fn expect_return_int(result: &Program32Result, expected: i64) {
        assert_eq!(result.first_return(), &RuntimeVal::Int(expected));
    }

    fn expect_return_nil(result: &Program32Result) {
        assert_eq!(result.first_return(), &RuntimeVal::Nil);
    }

    fn expect_return_str(result: &Program32Result, expected: &str) {
        assert_eq!(
            result.first_return(),
            &RuntimeVal::ShortStr(ShortStr::new(expected).expect("short test string"))
        );
    }

    #[test]
    fn test_if_let_simple_variable() {
        let ctx: Val = [("data".to_string(), Val::Int(42))]
            .into_iter()
            .collect::<HashMap<String, Val>>()
            .into();

        let result = parse_and_execute_stmt("if let x = data { return x; }", &ctx).unwrap();

        expect_return_int(&result, 42);
    }

    #[test]
    fn test_if_let_literal_match() {
        let ctx: Val = [("status".to_string(), Val::from_str("ok"))]
            .into_iter()
            .collect::<HashMap<String, Val>>()
            .into();

        let result = parse_and_execute_stmt(r#"if let "ok" = status { return 1; } else { return 0; }"#, &ctx).unwrap();

        expect_return_int(&result, 1);
    }

    #[test]
    fn test_if_let_literal_no_match() {
        let ctx: Val = [("status".to_string(), Val::from_str("error"))]
            .into_iter()
            .collect::<HashMap<String, Val>>()
            .into();

        let result = parse_and_execute_stmt(r#"if let "ok" = status { return 1; } else { return 0; }"#, &ctx).unwrap();

        expect_return_int(&result, 0);
    }

    #[test]
    fn test_if_let_list_destructuring() {
        let result = parse_and_execute_stmt(
            "let list = [1, 2, 3]; if let [first, second, third] = list { return first + second + third; }",
            &Val::Nil,
        )
        .unwrap();

        expect_return_int(&result, 6);
    }

    #[test]
    fn test_if_let_list_with_rest() {
        let result = parse_and_execute_stmt(
            "let list = [1, 2, 3, 4]; if let [first, ..rest] = list { return first; }",
            &Val::Nil,
        )
        .unwrap();

        expect_return_int(&result, 1);
    }

    #[test]
    fn test_if_let_map_destructuring() {
        let result = parse_and_execute_stmt(
            r#"let user = {"name": "Alice", "age": 30}; if let {"name": name} = user { return name; }"#,
            &Val::Nil,
        )
        .unwrap();

        expect_return_str(&result, "Alice");
    }

    #[test]
    fn test_if_let_wildcard() {
        let ctx: Val = [("data".to_string(), Val::Int(42))]
            .into_iter()
            .collect::<HashMap<String, Val>>()
            .into();

        let result = parse_and_execute_stmt("if let _ = data { return 1; } else { return 0; }", &ctx).unwrap();

        expect_return_int(&result, 1);
    }

    #[test]
    fn test_if_let_nested_pattern() {
        let result = parse_and_execute_stmt(
            r#"let data = {"items": ["first", "second"]}; if let {"items": [first, second]} = data { return first; }"#,
            &Val::Nil,
        )
        .unwrap();

        expect_return_str(&result, "first");
    }

    #[test]
    fn test_if_let_or_pattern() {
        let ctx: Val = [("status".to_string(), Val::Int(200))]
            .into_iter()
            .collect::<HashMap<String, Val>>()
            .into();

        let result =
            parse_and_execute_stmt("if let 200 | 201 | 202 = status { return 1; } else { return 0; }", &ctx).unwrap();

        expect_return_int(&result, 1);
    }

    #[test]
    fn test_if_let_guard_pattern() {
        let ctx: Val = [("value".to_string(), Val::Int(15))]
            .into_iter()
            .collect::<HashMap<String, Val>>()
            .into();

        let result =
            parse_and_execute_stmt("if let x if x > 10 = value { return x; } else { return 0; }", &ctx).unwrap();

        expect_return_int(&result, 15);
    }

    #[test]
    fn test_if_let_guard_pattern_no_match() {
        let ctx: Val = [("value".to_string(), Val::Int(5))]
            .into_iter()
            .collect::<HashMap<String, Val>>()
            .into();

        let result =
            parse_and_execute_stmt("if let x if x > 10 = value { return x; } else { return 0; }", &ctx).unwrap();

        expect_return_int(&result, 0);
    }

    #[test]
    fn test_if_let_range_pattern() {
        let ctx: Val = [("age".to_string(), Val::Int(25))]
            .into_iter()
            .collect::<HashMap<String, Val>>()
            .into();

        let result = parse_and_execute_stmt(
            r#"if let 18..65 = age { return "adult"; } else { return "other"; }"#,
            &ctx,
        )
        .unwrap();

        expect_return_str(&result, "adult");
    }

    #[test]
    fn test_if_let_variable_scoping() {
        let ctx: Val = [("data".to_string(), Val::Int(42))]
            .into_iter()
            .collect::<HashMap<String, Val>>()
            .into();

        // Variable should only be accessible within the if let block
        let result = parse_and_execute_stmt(
            r#"
            let x = "outer";
            if let y = data {
                return y;
            }
            return x;
            "#,
            &ctx,
        )
        .unwrap();

        expect_return_int(&result, 42);
    }

    #[test]
    fn test_if_let_complex_expression() {
        let result = parse_and_execute_stmt(
            r#"let data = [{"id": 1, "value": "test"}]; if let [{"id": id, "value": value}] = data { return value; }"#,
            &Val::Nil,
        )
        .unwrap();

        expect_return_str(&result, "test");
    }

    #[test]
    fn test_if_let_no_else_branch() {
        let ctx: Val = [("data".to_string(), Val::Nil)]
            .into_iter()
            .collect::<HashMap<String, Val>>()
            .into();

        let result = parse_and_execute_stmt("if let 42 = data { return 1; }", &ctx).unwrap();

        expect_return_nil(&result); // No match, no else, returns nil
    }
}
