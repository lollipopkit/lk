#[cfg(test)]
mod test {
    #[cfg(not(feature = "std"))]
    use crate::compat::prelude::*;
    use crate::{
        ast::Parser,
        expr::Expr,
        operator::BinOp,
        token::{Token, Tokenizer},
        val::LiteralVal,
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
                    Box::new(Expr::Literal(LiteralVal::from_str("user"))),
                )),
                Box::new(Expr::Literal(LiteralVal::from_str("age"))),
            )),
            BinOp::Gt,
            Box::new(Expr::Literal(LiteralVal::Int(18))),
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
        let expected = Expr::Paren(Box::new(Expr::Literal(LiteralVal::Bool(true))));
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
                    Box::new(Expr::Literal(LiteralVal::Int(0))),
                )),
                Box::new(Expr::Bin(
                    Box::new(Expr::Access(
                        Box::new(Expr::Var("col".to_string())),
                        Box::new(Expr::Literal(LiteralVal::from_str("pub"))),
                    )),
                    BinOp::Eq,
                    Box::new(Expr::Literal(LiteralVal::Bool(true))),
                )),
            )))),
            Box::new(Expr::Bin(
                Box::new(Expr::Var("random".to_string())),
                BinOp::Gt,
                Box::new(Expr::Literal(LiteralVal::Float(0.5))),
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
                Box::new(Expr::Literal(LiteralVal::Int(0))),
            )),
            Box::new(Expr::Literal(LiteralVal::from_str("name"))),
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
            Expr::Literal(LiteralVal::Int(1)),
            Expr::Literal(LiteralVal::Int(2)),
            Expr::Literal(LiteralVal::Int(3)),
        ]);
        assert_eq!(parsed, expected);
    }

    #[test]
    fn mixed_list() {
        let r = r#"[1, "hello", true]"#;

        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = list_expr(vec![
            Expr::Literal(LiteralVal::Int(1)),
            Expr::Literal(LiteralVal::from_str("hello")),
            Expr::Literal(LiteralVal::Bool(true)),
        ]);
        assert_eq!(parsed, expected);
    }

    #[test]
    fn list_with_expressions() {
        let r = "[1 + 2, 3 * 4]";

        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = list_expr(vec![
            Expr::Literal(LiteralVal::Int(3)),
            Expr::Literal(LiteralVal::Int(12)),
        ]);
        assert_eq!(parsed, expected);
    }

    #[test]
    fn nested_list() {
        let r = "[[1, 2], [3, 4]]";

        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = list_expr(vec![
            list_expr(vec![
                Expr::Literal(LiteralVal::Int(1)),
                Expr::Literal(LiteralVal::Int(2)),
            ]),
            list_expr(vec![
                Expr::Literal(LiteralVal::Int(3)),
                Expr::Literal(LiteralVal::Int(4)),
            ]),
        ]);
        assert_eq!(parsed, expected);
    }

    #[test]
    fn list_with_trailing_comma() {
        let r = "[1, 2, 3,]";

        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = list_expr(vec![
            Expr::Literal(LiteralVal::Int(1)),
            Expr::Literal(LiteralVal::Int(2)),
            Expr::Literal(LiteralVal::Int(3)),
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
            (
                Expr::Literal(LiteralVal::from_str("name")),
                Expr::Literal(LiteralVal::from_str("Alice")),
            ),
            (
                Expr::Literal(LiteralVal::from_str("age")),
                Expr::Literal(LiteralVal::Int(30)),
            ),
        ]);
        assert_eq!(parsed, expected);
    }

    #[test]
    fn map_with_expressions() {
        let r = r#"{"sum": 1 + 2, "product": 3 * 4}"#;

        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = map_expr(vec![
            (
                Expr::Literal(LiteralVal::from_str("sum")),
                Expr::Literal(LiteralVal::Int(3)),
            ),
            (
                Expr::Literal(LiteralVal::from_str("product")),
                Expr::Literal(LiteralVal::Int(12)),
            ),
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
            (
                Expr::Literal(LiteralVal::Int(42)),
                Expr::Literal(LiteralVal::from_str("number")),
            ),
            (
                Expr::Literal(LiteralVal::Bool(true)),
                Expr::Literal(LiteralVal::from_str("bool")),
            ),
            (
                Expr::Literal(LiteralVal::from_str("key")),
                Expr::Literal(LiteralVal::from_str("string")),
            ),
        ]);
        assert_eq!(parsed, expected);
    }

    #[test]
    fn nested_map() {
        let r = r#"{"user": {"name": "Alice", "age": 30}}"#;

        let ts = Tokenizer::tokenize(r).unwrap();
        let parsed = Parser::new(&ts).parse().unwrap();
        let expected = map_expr(vec![(
            Expr::Literal(LiteralVal::from_str("user")),
            map_expr(vec![
                (
                    Expr::Literal(LiteralVal::from_str("name")),
                    Expr::Literal(LiteralVal::from_str("Alice")),
                ),
                (
                    Expr::Literal(LiteralVal::from_str("age")),
                    Expr::Literal(LiteralVal::Int(30)),
                ),
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
            (
                Expr::Literal(LiteralVal::from_str("a")),
                Expr::Literal(LiteralVal::Int(1)),
            ),
            (
                Expr::Literal(LiteralVal::from_str("b")),
                Expr::Literal(LiteralVal::Int(2)),
            ),
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
                (
                    Expr::Literal(LiteralVal::from_str("name")),
                    Expr::Literal(LiteralVal::from_str("Alice")),
                ),
                (
                    Expr::Literal(LiteralVal::from_str("scores")),
                    list_expr(vec![
                        Expr::Literal(LiteralVal::Int(90)),
                        Expr::Literal(LiteralVal::Int(85)),
                    ]),
                ),
            ]),
            map_expr(vec![
                (
                    Expr::Literal(LiteralVal::from_str("name")),
                    Expr::Literal(LiteralVal::from_str("Bob")),
                ),
                (
                    Expr::Literal(LiteralVal::from_str("scores")),
                    list_expr(vec![
                        Expr::Literal(LiteralVal::Int(88)),
                        Expr::Literal(LiteralVal::Int(92)),
                    ]),
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
                Box::new(Expr::Literal(LiteralVal::from_str("name"))),
            )),
            Box::new(Expr::Access(
                Box::new(Expr::Var("user".to_string())),
                Box::new(Expr::Literal(LiteralVal::from_str("age"))),
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
            Box::new(Expr::Literal(LiteralVal::from_str("with.&="))),
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
                Box::new(Expr::Literal(LiteralVal::from_str("user"))),
            )),
            Box::new(Expr::Literal(LiteralVal::from_str("name"))),
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
                    Box::new(Expr::Literal(LiteralVal::from_str("user"))),
                )),
                Box::new(Expr::Literal(LiteralVal::from_str("special-field"))),
            )),
            Box::new(Expr::Literal(LiteralVal::from_str("data"))),
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
            Box::new(Expr::Literal(LiteralVal::from_str("field-with@special#chars$"))),
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
                    Box::new(Expr::Literal(LiteralVal::Int(0))),
                )),
                Box::new(Expr::Literal(LiteralVal::from_str("name"))),
            )),
            Box::new(Expr::Literal(LiteralVal::from_str("value"))),
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
                Box::new(Expr::Literal(LiteralVal::from_str("debug-mode"))),
            )),
            BinOp::Eq,
            Box::new(Expr::Literal(LiteralVal::Bool(true))),
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
            Box::new(Expr::Literal(LiteralVal::from_str("field with spaces"))),
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
            Box::new(Expr::Literal(LiteralVal::from_str("field's name"))),
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
            Box::new(Expr::Literal(LiteralVal::from_str("special-field"))),
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
                    Box::new(Expr::Literal(LiteralVal::from_str("user-data"))),
                )),
                Box::new(Expr::Literal(LiteralVal::from_str("is-active"))),
            )),
            Box::new(Expr::Bin(
                Box::new(Expr::Access(
                    Box::new(Expr::Var("config".to_string())),
                    Box::new(Expr::Literal(LiteralVal::from_str("debug-enabled"))),
                )),
                BinOp::Eq,
                Box::new(Expr::Literal(LiteralVal::Bool(false))),
            )),
        );
        assert_eq!(parsed, expected);
    }

    /// Deeply nested expressions used to overflow the Rust stack and abort the
    /// process (500 levels sufficed in a debug build). A host traps that on a
    /// guard page; bare metal has none, so the bound is what keeps a hostile
    /// input from walking off an MCU stack.
    #[test]
    fn deeply_nested_expressions_error_instead_of_overflowing_the_stack() {
        let depth = 10_000;
        let mut source = String::new();
        for _ in 0..depth {
            source.push('(');
        }
        source.push('1');
        for _ in 0..depth {
            source.push(')');
        }

        let tokens = Tokenizer::tokenize(&source).expect("tokenizes");
        let err = Parser::new(&tokens).parse().expect_err("must not abort");
        assert!(err.to_string().contains("too deep"), "{err}");
    }

    /// Prefix operators recurse into `parse_unary`, not `parse_expr`, so
    /// bounding only the latter left this able to abort the process.
    #[test]
    fn deeply_nested_unary_operators_error_instead_of_overflowing_the_stack() {
        let source = "!".repeat(20_000) + "true";
        let tokens = Tokenizer::tokenize(&source).expect("tokenizes");
        let err = Parser::new(&tokens).parse().expect_err("must not abort");
        assert!(err.to_string().contains("too deep"), "{err}");
    }

    /// `match` arms used to recurse straight into `parse_conditional`, which
    /// skipped the budget entirely.
    #[test]
    fn deeply_nested_match_arms_error_instead_of_overflowing_the_stack() {
        let depth = 2_000;
        let source = "match 1 { _ => ".repeat(depth) + "1" + &" }".repeat(depth);
        let tokens = Tokenizer::tokenize(&source).expect("tokenizes");
        let err = Parser::new(&tokens).parse().expect_err("must not abort");
        assert!(err.to_string().contains("too deep"), "{err}");
    }

    /// Only a bare **name** can start a macro invocation.
    ///
    /// The test used to be the open delimiter alone, so `m["a"]![0]` — unwrap a
    /// map read, then index it — was "a macro invocation reached the parser",
    /// for a spelling no macro could ever have. The workaround was to
    /// parenthesise or split the line, for an expression with no ambiguity in
    /// it.
    #[test]
    fn postfix_unwrap_is_not_a_macro_invocation() {
        let parses = |src: &str| {
            let tokens = Tokenizer::tokenize(src).expect("tokenize");
            Parser::new(&tokens).parse().is_ok()
        };
        assert!(parses(r#"m["a"]![0]"#), "unwrap a map read, then index it");
        assert!(parses("xs[0]![0]"), "unwrap a list read, then index it");
        assert!(parses("m.field![0]"), "unwrap a field read, then index it");
        assert!(
            parses("(m!)[0]"),
            "the parenthesised spelling for unwrapping a bare name"
        );
        assert!(
            parses("m[\"a\"]! + 1"),
            "a `!` not followed by a delimiter was always fine"
        );

        // A bare name *is* ambiguous, and the name goes to the macro — with the
        // message that says so, since expansion runs before the parser.
        let tokens = Tokenizer::tokenize("nope!()").expect("tokenize");
        let error = Parser::new(&tokens).parse().expect_err("no such macro");
        let text = alloc::format!("{error:#}");
        assert!(text.contains("no macro named `nope`"), "{text}");
        assert!(text.contains("(nope!)(…)"), "{text}");
    }

    /// `Expr` is parsed recursively, so its *size* is part of how deep the
    /// parser can go before the stack runs out — and the depth guard is only
    /// useful if it trips first.
    ///
    /// Adding a `Type` field to `Expr::Closure` by value (a large enum, inline)
    /// grew every parse frame enough that
    /// `deeply_nested_match_arms_error_instead_of_overflowing_the_stack` started
    /// aborting instead of erroring. Boxing fixed it; this says so out loud, so
    /// the next field either stays small or is a deliberate decision about the
    /// depth bound rather than a surprise crash.
    #[test]
    fn the_expression_node_stays_small_enough_to_recurse_over() {
        let size = core::mem::size_of::<crate::expr::Expr>();
        assert!(
            size <= 80,
            "Expr grew to {size} bytes; box the new field or re-tune the parser's depth guard"
        );
    }

    /// A `match` value is parsed by its own `Parser`; without inheriting the
    /// budget, nesting there would get a fresh allowance each level.
    #[test]
    fn nested_parsers_inherit_the_depth_budget() {
        // The value is parenthesised so each level nests as the *value* of the
        // enclosing `match`. Without the parens `match match … { … } { … }`
        // does not parse at all, and the test would pass on a syntax error
        // rather than on the depth bound.
        let mut source = String::from("1");
        for _ in 0..2_000 {
            source = alloc::format!("match ({source}) {{ _ => 1 }}");
        }
        let tokens = Tokenizer::tokenize(&source).expect("tokenizes");
        let err = Parser::new(&tokens).parse().expect_err("must not abort");
        assert!(err.to_string().contains("too deep"), "{err}");
    }

    /// An `unsafe` block evaluates to its final expression and *continues*.
    ///
    /// The obvious implementation reuses the closure-body parser, which
    /// rewrites the last statement into a `return`. That is not a type error:
    /// `let x = unsafe { 1 }; println(x);` compiles, returns 1 from the
    /// enclosing function, and silently never prints. Hence a test on the
    /// shape rather than on the value alone.
    #[test]
    fn unsafe_block_is_a_value_not_a_return() {
        use crate::stmt::Stmt;

        let tokens = Tokenizer::tokenize("unsafe { 1 }").expect("tokenizes");
        let parsed = Parser::new(&tokens).parse().expect("parses");
        let Expr::Unsafe(block) = parsed else {
            panic!("expected an unsafe block, got {parsed:?}");
        };
        let Expr::Block(statements) = *block else {
            panic!("unsafe should wrap a block");
        };
        assert!(
            matches!(statements.last().map(|s| s.as_ref()), Some(Stmt::Expr { .. })),
            "the tail must stay an expression, not become a return: {statements:?}"
        );
    }

    #[test]
    fn unsafe_blocks_nest() {
        let tokens = Tokenizer::tokenize("unsafe { unsafe { 1 } }").expect("tokenizes");
        let parsed = Parser::new(&tokens).parse().expect("parses");
        assert!(matches!(parsed, Expr::Unsafe(_)), "{parsed:?}");
    }

    #[test]
    fn unsafe_requires_braces() {
        let tokens = Tokenizer::tokenize("unsafe 1").expect("tokenizes");
        let err = Parser::new(&tokens).parse().expect_err("bare unsafe must not parse");
        assert!(err.to_string().contains("Expecting '{'"), "{err}");
    }

    /// The cap must not be so tight that ordinary nesting trips it.
    #[test]
    fn ordinary_nesting_stays_under_the_depth_cap() {
        let source = "((((1 + 2) * 3) - 4) / 5)";
        let tokens = Tokenizer::tokenize(source).expect("tokenizes");
        Parser::new(&tokens).parse().expect("ordinary nesting parses");
    }
    /// `<<` and `>>` are two adjacent comparison tokens, not tokens of their
    /// own — the lexer cannot tell a right shift from the end of
    /// `List<List<Int>>`. These pin both halves of that trade.
    #[test]
    fn shifts_parse_as_builtin_calls() {
        for (source, builtin) in [("1 << 3", "__lk_shl"), ("16 >> 2", "__lk_shr")] {
            let tokens = Tokenizer::tokenize(source).expect("tokenizes");
            let parsed = Parser::new(&tokens).parse().expect("parses");
            let Expr::Call(name, args) = &parsed else {
                panic!("expected a builtin call for {source}, got {parsed:?}");
            };
            assert_eq!(name, builtin, "{source}");
            assert_eq!(args.len(), 2, "{source}");
        }
    }

    /// Rust's precedence: tighter than comparison, looser than `+`.
    #[test]
    fn shift_binds_looser_than_addition() {
        let tokens = Tokenizer::tokenize("1 << 2 + 3").expect("tokenizes");
        let parsed = Parser::new(&tokens).parse().expect("parses");
        let Expr::Call(name, args) = &parsed else {
            panic!("expected a shift at the root, got {parsed:?}");
        };
        assert_eq!(name, "__lk_shl");
        // The parser folds the constant sum, so what matters is that the sum
        // ended up *inside* the shift rather than the shift inside the sum.
        assert!(
            matches!(
                args[1].as_ref(),
                Expr::Bin(_, BinOp::Add, _) | Expr::Literal(LiteralVal::Int(5))
            ),
            "the right operand should be the sum: {:?}",
            args[1]
        );
    }

    #[test]
    fn comparison_sees_the_shift_as_an_operand() {
        let tokens = Tokenizer::tokenize("8 >> 1 == 4").expect("tokenizes");
        let parsed = Parser::new(&tokens).parse().expect("parses");
        let Expr::Bin(left, BinOp::Eq, _) = &parsed else {
            panic!("expected a comparison at the root, got {parsed:?}");
        };
        assert!(
            matches!(left.as_ref(), Expr::Call(name, _) if name == "__lk_shr"),
            "the left operand should be the shift: {left:?}"
        );
    }

    /// Separated by a space it is two comparisons, not a shift — which is a
    /// parse error here, and deliberately not silently a shift.
    #[test]
    fn separated_comparisons_are_not_a_shift() {
        let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans("1 < < 3").expect("tokenizes");
        assert!(
            Parser::new_with_spans(&tokens, &spans).parse().is_err(),
            "`1 < < 3` must not parse as a shift"
        );
    }

    /// Macro expansion runs before parsing, so a `name!(…)` that reaches the
    /// parser is one no macro answered. It used to leave the `!` unconsumed and
    /// report "Unexpected tokens at end (found Not)" — a token the program does
    /// not contain, and no mention of macros at all.
    #[test]
    fn an_undefined_macro_says_so() {
        for source in ["nope!();", "let x = nope!();", "println(nope!());", "let y = nope![1];"] {
            let tokens = crate::token::Tokenizer::tokenize(source).expect("tokenize");
            let error = crate::stmt::StmtParser::new(&tokens)
                .parse_program()
                .expect_err("no macro named `nope`");
            let text = format!("{error:#}");
            assert!(text.contains("no macro named `nope`"), "{source} → {text}");
        }
    }
}
