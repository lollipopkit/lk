#[cfg(test)]
mod tests {
    use crate::{
        stmt::{StmtParser, run_program},
        token::Tokenizer,
        val::Val,
        vm::VmContext,
    };
    use std::{collections::HashMap, sync::Arc};

    fn parse_and_execute_stmt(stmt_code: &str, ctx: &Val) -> Result<Val, anyhow::Error> {
        let tokens = Tokenizer::tokenize(stmt_code)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;
        let mut env = VmContext::new();
        if let Val::Map(m) = ctx {
            for (k, v) in m.iter() {
                env.define(k.to_string(), v.clone());
            }
        }
        run_program(&program, &mut env)
    }

    #[test]
    fn test_if_let_simple_variable() {
        let ctx: Val = [("data".to_string(), Val::Int(42))]
            .into_iter()
            .collect::<HashMap<String, Val>>()
            .into();

        let result = parse_and_execute_stmt("if let x = data { return x; }", &ctx).unwrap();

        assert_eq!(result, Val::Int(42));
    }

    #[test]
    fn test_if_let_literal_match() {
        let ctx: Val = [("status".to_string(), Val::Str("ok".into()))]
            .into_iter()
            .collect::<HashMap<String, Val>>()
            .into();

        let result = parse_and_execute_stmt(r#"if let "ok" = status { return 1; } else { return 0; }"#, &ctx).unwrap();

        assert_eq!(result, Val::Int(1));
    }

    #[test]
    fn test_if_let_literal_no_match() {
        let ctx: Val = [("status".to_string(), Val::Str("error".into()))]
            .into_iter()
            .collect::<HashMap<String, Val>>()
            .into();

        let result = parse_and_execute_stmt(r#"if let "ok" = status { return 1; } else { return 0; }"#, &ctx).unwrap();

        assert_eq!(result, Val::Int(0));
    }

    #[test]
    fn test_if_let_list_destructuring() {
        let ctx: Val = [(
            "list".to_string(),
            Val::List(Arc::from(vec![Val::Int(1), Val::Int(2), Val::Int(3)])),
        )]
        .into_iter()
        .collect::<HashMap<String, Val>>()
        .into();

        let result = parse_and_execute_stmt(
            "if let [first, second, third] = list { return first + second + third; }",
            &ctx,
        )
        .unwrap();

        assert_eq!(result, Val::Int(6));
    }

    #[test]
    fn test_if_let_list_with_rest() {
        let ctx: Val = [(
            "list".to_string(),
            Val::List(Arc::from(vec![Val::Int(1), Val::Int(2), Val::Int(3), Val::Int(4)])),
        )]
        .into_iter()
        .collect::<HashMap<String, Val>>()
        .into();

        let result = parse_and_execute_stmt("if let [first, ..rest] = list { return first; }", &ctx).unwrap();

        assert_eq!(result, Val::Int(1));
    }

    #[test]
    fn test_if_let_map_destructuring() {
        let ctx: Val = [(
            "user".to_string(),
            ([
                ("name".to_string(), Val::Str("Alice".into())),
                ("age".to_string(), Val::Int(30)),
            ]
            .into_iter()
            .collect::<HashMap<String, Val>>()
            .into()),
        )]
        .into_iter()
        .collect::<HashMap<String, Val>>()
        .into();

        let result = parse_and_execute_stmt(r#"if let {"name": name} = user { return name; }"#, &ctx).unwrap();

        assert_eq!(result, Val::Str("Alice".into()));
    }

    #[test]
    fn test_if_let_wildcard() {
        let ctx: Val = [("data".to_string(), Val::Int(42))]
            .into_iter()
            .collect::<HashMap<String, Val>>()
            .into();

        let result = parse_and_execute_stmt("if let _ = data { return 1; } else { return 0; }", &ctx).unwrap();

        assert_eq!(result, Val::Int(1));
    }

    #[test]
    fn test_if_let_nested_pattern() {
        let ctx: Val = [(
            "data".to_string(),
            ([(
                "items".to_string(),
                Val::List(Arc::from(vec![Val::Str("first".into()), Val::Str("second".into())])),
            )]
            .into_iter()
            .collect::<HashMap<String, Val>>()
            .into()),
        )]
        .into_iter()
        .collect::<HashMap<String, Val>>()
        .into();

        let result =
            parse_and_execute_stmt(r#"if let {"items": [first, second]} = data { return first; }"#, &ctx).unwrap();

        assert_eq!(result, Val::Str("first".into()));
    }

    #[test]
    fn test_if_let_or_pattern() {
        let ctx: Val = [("status".to_string(), Val::Int(200))]
            .into_iter()
            .collect::<HashMap<String, Val>>()
            .into();

        let result =
            parse_and_execute_stmt("if let 200 | 201 | 202 = status { return 1; } else { return 0; }", &ctx).unwrap();

        assert_eq!(result, Val::Int(1));
    }

    #[test]
    fn test_if_let_guard_pattern() {
        let ctx: Val = [("value".to_string(), Val::Int(15))]
            .into_iter()
            .collect::<HashMap<String, Val>>()
            .into();

        let result =
            parse_and_execute_stmt("if let x if x > 10 = value { return x; } else { return 0; }", &ctx).unwrap();

        assert_eq!(result, Val::Int(15));
    }

    #[test]
    fn test_if_let_guard_pattern_no_match() {
        let ctx: Val = [("value".to_string(), Val::Int(5))]
            .into_iter()
            .collect::<HashMap<String, Val>>()
            .into();

        let result =
            parse_and_execute_stmt("if let x if x > 10 = value { return x; } else { return 0; }", &ctx).unwrap();

        assert_eq!(result, Val::Int(0));
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

        assert_eq!(result, Val::Str("adult".into()));
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

        assert_eq!(result, Val::Int(42));
    }

    #[test]
    fn test_if_let_complex_expression() {
        let ctx: Val = [(
            "data".to_string(),
            Val::List(Arc::from(vec![
                ([
                    ("id".to_string(), Val::Int(1)),
                    ("value".to_string(), Val::Str("test".into())),
                ]
                .into_iter()
                .collect::<HashMap<String, Val>>()
                .into()),
            ])),
        )]
        .into_iter()
        .collect::<HashMap<String, Val>>()
        .into();

        let result =
            parse_and_execute_stmt(r#"if let [{"id": id, "value": value}] = data { return value; }"#, &ctx).unwrap();

        assert_eq!(result, Val::Str("test".into()));
    }

    #[test]
    fn test_if_let_no_else_branch() {
        let ctx: Val = [("data".to_string(), Val::Nil)]
            .into_iter()
            .collect::<HashMap<String, Val>>()
            .into();

        let result = parse_and_execute_stmt("if let 42 = data { return 1; }", &ctx).unwrap();

        assert_eq!(result, Val::Nil); // No match, no else, returns nil
    }
}
