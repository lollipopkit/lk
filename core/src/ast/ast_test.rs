#[cfg(test)]
mod test {
    use crate::{
        ast::Parser,
        expr::Expr,
        op::BinOp,
        token::{Token, Tokenizer},
        val::Val,
    };

    fn list_expr(values: Vec<Expr>) -> Expr {
        Expr::List(values.into_iter().map(Box::new).collect())
    }

    fn map_expr(pairs: Vec<(Expr, Expr)>) -> Expr {
        Expr::Map(
            pairs
                .into_iter()
                .map(|(key, value)| (Box::new(key), Box::new(value)))
                .collect(),
        )
    }

    #[test]
    fn basic() {
        let tokens = vec![
            Token::Id("req".to_string()),
            Token::Dot,
            Token::Id("user".to_string()),
            Token::Dot,
            Token::Id("age".to_string()),
            Token::Gt,
            Token::Int(18),
        ];
        let expr = Expr::Bin(
            Box::new(Expr::Access(
                Box::new(Expr::Access(
                    Box::new(Expr::Var("req".to_string())),
                    Box::new(Expr::Val(Val::from_str("user"))),
                )),
                Box::new(Expr::Val(Val::from_str("age"))),
            )),
            BinOp::Gt,
            Box::new(Expr::Val(18.into())),
        );
        let parsed = Parser::new(&tokens).parse().unwrap();
        assert_eq!(parsed, expr);
    }

    #[test]
    fn paren() {
        let r = r#"
        (
            true
            ||
            false
        )
        "#;

        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = Expr::Paren(Box::new(Expr::Val(Val::Bool(true))));
        assert_eq!(parsed, expected);
    }

    #[test]
    fn complex() {
        let r = r#"
        (
            time != 0 
            ||
            col.pub == true
        )
        &&
        random > 0.5
        "#;

        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = Expr::And(
            Box::new(Expr::Paren(Box::new(Expr::Or(
                Box::new(Expr::Bin(
                    Box::new(Expr::Var("time".to_string())),
                    BinOp::Ne,
                    Box::new(Expr::Val(0.into())),
                )),
                Box::new(Expr::Bin(
                    Box::new(Expr::Access(
                        Box::new(Expr::Var("col".to_string())),
                        Box::new(Expr::Val(Val::from_str("pub"))),
                    )),
                    BinOp::Eq,
                    Box::new(Expr::Val(true.into())),
                )),
            )))),
            Box::new(Expr::Bin(
                Box::new(Expr::Var("random".to_string())),
                BinOp::Gt,
                Box::new(Expr::Val(0.5.into())),
            )),
        );
        assert_eq!(parsed, expected);
    }

    #[test]
    fn access_str_int_str() {
        let r = "list.0.name";

        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = Expr::Access(
            Box::new(Expr::Access(
                Box::new(Expr::Var("list".to_string())),
                Box::new(Expr::Val(Val::Int(0))),
            )),
            Box::new(Expr::Val(Val::from_str("name"))),
        );
        assert_eq!(parsed, expected);
    }

    #[test]
    fn access_first_int_paths() {
        let r = ".name";

        let t = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&t).parse();
        assert!(parsed.is_err());
    }

    #[test]
    fn empty_list() {
        let r = "[]";

        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = Expr::List(vec![]);
        assert_eq!(parsed, expected);
    }

    #[test]
    fn simple_list() {
        let r = "[1, 2, 3]";

        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = list_expr(vec![
            Expr::Val(Val::Int(1)),
            Expr::Val(Val::Int(2)),
            Expr::Val(Val::Int(3)),
        ]);
        assert_eq!(parsed, expected);
    }

    #[test]
    fn mixed_list() {
        let r = r#"[1, "hello", true]"#;

        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = list_expr(vec![
            Expr::Val(Val::Int(1)),
            Expr::Val(Val::from_str("hello")),
            Expr::Val(Val::Bool(true)),
        ]);
        assert_eq!(parsed, expected);
    }

    #[test]
    fn list_with_expressions() {
        let r = "[1 + 2, 3 * 4]";

        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = list_expr(vec![Expr::Val(Val::Int(3)), Expr::Val(Val::Int(12))]);
        assert_eq!(parsed, expected);
    }

    #[test]
    fn nested_list() {
        let r = "[[1, 2], [3, 4]]";

        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = list_expr(vec![
            list_expr(vec![Expr::Val(Val::Int(1)), Expr::Val(Val::Int(2))]),
            list_expr(vec![Expr::Val(Val::Int(3)), Expr::Val(Val::Int(4))]),
        ]);
        assert_eq!(parsed, expected);
    }

    #[test]
    fn list_with_trailing_comma() {
        let r = "[1, 2, 3,]";

        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = list_expr(vec![
            Expr::Val(Val::Int(1)),
            Expr::Val(Val::Int(2)),
            Expr::Val(Val::Int(3)),
        ]);
        assert_eq!(parsed, expected);
    }

    #[test]
    fn empty_map() {
        let r = "{}";

        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = Expr::Map(vec![]);
        assert_eq!(parsed, expected);
    }

    #[test]
    fn simple_map() {
        let r = r#"{"name": "Alice", "age": 30}"#;

        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = map_expr(vec![
            (Expr::Val(Val::from_str("name")), Expr::Val(Val::from_str("Alice"))),
            (Expr::Val(Val::from_str("age")), Expr::Val(Val::Int(30))),
        ]);
        assert_eq!(parsed, expected);
    }

    #[test]
    fn map_with_expressions() {
        let r = r#"{"sum": 1 + 2, "product": 3 * 4}"#;

        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = map_expr(vec![
            (Expr::Val(Val::from_str("sum")), Expr::Val(Val::Int(3))),
            (Expr::Val(Val::from_str("product")), Expr::Val(Val::Int(12))),
        ]);
        assert_eq!(parsed, expected);
    }

    #[test]
    fn call_named_expr_parsing() {
        let ts = Tokenizer::tokenize("f(a: 1, b: 2)").unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        match parsed {
            Expr::CallNamed(_callee, pos, named) => {
                assert_eq!(pos.len(), 0);
                assert_eq!(named.len(), 2);
                assert_eq!(named[0].0, "a");
                assert_eq!(named[1].0, "b");
            }
            other => panic!("Expected CallNamed, got {:?}", other),
        }
    }

    #[test]
    fn map_with_different_key_types() {
        let r = r#"{42: "number", true: "bool", "key": "string"}"#;

        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = map_expr(vec![
            (Expr::Val(Val::Int(42)), Expr::Val(Val::from_str("number"))),
            (Expr::Val(Val::Bool(true)), Expr::Val(Val::from_str("bool"))),
            (Expr::Val(Val::from_str("key")), Expr::Val(Val::from_str("string"))),
        ]);
        assert_eq!(parsed, expected);
    }

    #[test]
    fn nested_map() {
        let r = r#"{"user": {"name": "Alice", "age": 30}}"#;

        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = map_expr(vec![(
            Expr::Val(Val::from_str("user")),
            map_expr(vec![
                (Expr::Val(Val::from_str("name")), Expr::Val(Val::from_str("Alice"))),
                (Expr::Val(Val::from_str("age")), Expr::Val(Val::Int(30))),
            ]),
        )]);
        assert_eq!(parsed, expected);
    }

    #[test]
    fn map_with_trailing_comma() {
        let r = r#"{"a": 1, "b": 2,}"#;

        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = map_expr(vec![
            (Expr::Val(Val::from_str("a")), Expr::Val(Val::Int(1))),
            (Expr::Val(Val::from_str("b")), Expr::Val(Val::Int(2))),
        ]);
        assert_eq!(parsed, expected);
    }

    #[test]
    fn mixed_structures() {
        let r = r#"[{"name": "Alice", "scores": [90, 85]}, {"name": "Bob", "scores": [88, 92]}]"#;

        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();

        let expected = list_expr(vec![
            map_expr(vec![
                (Expr::Val(Val::from_str("name")), Expr::Val(Val::from_str("Alice"))),
                (
                    Expr::Val(Val::from_str("scores")),
                    list_expr(vec![Expr::Val(Val::Int(90)), Expr::Val(Val::Int(85))]),
                ),
            ]),
            map_expr(vec![
                (Expr::Val(Val::from_str("name")), Expr::Val(Val::from_str("Bob"))),
                (
                    Expr::Val(Val::from_str("scores")),
                    list_expr(vec![Expr::Val(Val::Int(88)), Expr::Val(Val::Int(92))]),
                ),
            ]),
        ]);
        assert_eq!(parsed, expected);
    }

    #[test]
    fn member_access_in_literals() {
        let r = r#"[user.name, user.age]"#;

        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = Expr::List(vec![
            Box::new(Expr::Access(
                Box::new(Expr::Var("user".to_string())),
                Box::new(Expr::Val(Val::from_str("name"))),
            )),
            Box::new(Expr::Access(
                Box::new(Expr::Var("user".to_string())),
                Box::new(Expr::Val(Val::from_str("age"))),
            )),
        ]);
        assert_eq!(parsed, expected);
    }

    #[test]
    fn invalid_list_syntax() {
        // Missing closing bracket
        let r = "[1, 2, 3";
        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse();
        assert!(parsed.is_err());

        // Invalid separator
        let r = "[1; 2; 3]";
        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse();
        assert!(parsed.is_err());
    }

    #[test]
    fn invalid_map_syntax() {
        // Missing closing brace
        let r = r#"{"key": "value""#;
        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse();
        assert!(parsed.is_err());

        // Missing colon
        let r = r#"{"key" "value"}"#;
        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse();
        assert!(parsed.is_err());

        // Missing value
        let r = r#"{"key":}"#;
        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse();
        assert!(parsed.is_err());
    }

    #[test]
    fn quoted_field_access_simple() {
        // Basic quoted field access
        let r = r#"data."with.&=""#;
        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = Expr::Access(
            Box::new(Expr::Var("data".to_string())),
            Box::new(Expr::Val(Val::from_str("with.&="))),
        );
        assert_eq!(parsed, expected);
    }

    #[test]
    fn quoted_field_access_nested() {
        // Nested quoted field access
        let r = r#"req."user"."name""#;
        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = Expr::Access(
            Box::new(Expr::Access(
                Box::new(Expr::Var("req".to_string())),
                Box::new(Expr::Val(Val::from_str("user"))),
            )),
            Box::new(Expr::Val(Val::from_str("name"))),
        );
        assert_eq!(parsed, expected);
    }

    #[test]
    fn mixed_quoted_unquoted_access() {
        // Mix of quoted and unquoted field access
        let r = r#"req.user."special-field".data"#;
        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = Expr::Access(
            Box::new(Expr::Access(
                Box::new(Expr::Access(
                    Box::new(Expr::Var("req".to_string())),
                    Box::new(Expr::Val(Val::from_str("user"))),
                )),
                Box::new(Expr::Val(Val::from_str("special-field"))),
            )),
            Box::new(Expr::Val(Val::from_str("data"))),
        );
        assert_eq!(parsed, expected);
    }

    #[test]
    fn quoted_field_with_special_chars() {
        // Field name with various special characters
        let r = r#"data."field-with@special#chars$""#;
        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = Expr::Access(
            Box::new(Expr::Var("data".to_string())),
            Box::new(Expr::Val(Val::from_str("field-with@special#chars$"))),
        );
        assert_eq!(parsed, expected);
    }

    #[test]
    fn quoted_field_numeric_mixed() {
        // Mix of quoted fields, numeric indices, and regular fields
        let r = r#"files.0."name".value"#;
        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = Expr::Access(
            Box::new(Expr::Access(
                Box::new(Expr::Access(
                    Box::new(Expr::Var("files".to_string())),
                    Box::new(Expr::Val(Val::Int(0))),
                )),
                Box::new(Expr::Val(Val::from_str("name"))),
            )),
            Box::new(Expr::Val(Val::from_str("value"))),
        );
        assert_eq!(parsed, expected);
    }

    #[test]
    fn quoted_field_in_expression() {
        // Quoted field access in comparison expression
        let r = r#"config."debug-mode" == true"#;
        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = Expr::Bin(
            Box::new(Expr::Access(
                Box::new(Expr::Var("config".to_string())),
                Box::new(Expr::Val(Val::from_str("debug-mode"))),
            )),
            BinOp::Eq,
            Box::new(Expr::Val(true.into())),
        );
        assert_eq!(parsed, expected);
    }

    #[test]
    fn quoted_field_with_spaces() {
        // Field name with spaces
        let r = r#"data."field with spaces""#;
        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = Expr::Access(
            Box::new(Expr::Var("data".to_string())),
            Box::new(Expr::Val(Val::from_str("field with spaces"))),
        );
        assert_eq!(parsed, expected);
    }

    #[test]
    fn quoted_field_with_quotes_inside() {
        // Field name with single quotes inside double quotes
        let r = r#"data."field's name""#;
        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = Expr::Access(
            Box::new(Expr::Var("data".to_string())),
            Box::new(Expr::Val(Val::from_str("field's name"))),
        );
        assert_eq!(parsed, expected);
    }

    #[test]
    fn single_quoted_field_access() {
        // Using single quotes instead of double quotes
        let r = r#"data.'special-field'"#;
        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = Expr::Access(
            Box::new(Expr::Var("data".to_string())),
            Box::new(Expr::Val(Val::from_str("special-field"))),
        );
        assert_eq!(parsed, expected);
    }

    #[test]
    fn complex_quoted_field_expression() {
        // Complex expression with multiple quoted fields
        let r = r#"req."user-data"."is-active" && config."debug-enabled" == false"#;
        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = Expr::And(
            Box::new(Expr::Access(
                Box::new(Expr::Access(
                    Box::new(Expr::Var("req".to_string())),
                    Box::new(Expr::Val(Val::from_str("user-data"))),
                )),
                Box::new(Expr::Val(Val::from_str("is-active"))),
            )),
            Box::new(Expr::Bin(
                Box::new(Expr::Access(
                    Box::new(Expr::Var("config".to_string())),
                    Box::new(Expr::Val(Val::from_str("debug-enabled"))),
                )),
                BinOp::Eq,
                Box::new(Expr::Val(false.into())),
            )),
        );
        assert_eq!(parsed, expected);
    }
}
