#[cfg(test)]
mod tests {
    use crate::{
        expr::{Expr, MatchArm, Pattern},
        op::BinOp,
        val::Val,
        vm::VmContext,
    };
    use std::sync::Arc;

    #[test]
    fn test_literal_patterns() {
        // Test integer literal pattern
        let match_expr = Expr::Match {
            value: Box::new(Expr::Val(Val::Int(42))),
            arms: vec![
                MatchArm {
                    pattern: Pattern::Literal(Val::Int(42)),
                    body: Box::new(Expr::Val(Val::from_str("matched"))),
                },
                MatchArm {
                    pattern: Pattern::Wildcard,
                    body: Box::new(Expr::Val(Val::from_str("default"))),
                },
            ],
        };

        let mut env = VmContext::new();
        let result = match_expr.eval_with_ctx(&mut env).unwrap();
        assert_eq!(result, Val::from_str("matched"));

        // Test string literal pattern
        let match_expr = Expr::Match {
            value: Box::new(Expr::Val(Val::from_str("hello"))),
            arms: vec![
                MatchArm {
                    pattern: Pattern::Literal(Val::from_str("world")),
                    body: Box::new(Expr::Val(Val::from_str("world"))),
                },
                MatchArm {
                    pattern: Pattern::Literal(Val::from_str("hello")),
                    body: Box::new(Expr::Val(Val::from_str("hello"))),
                },
            ],
        };

        let mut env = VmContext::new();
        let result = match_expr.eval_with_ctx(&mut env).unwrap();
        assert_eq!(result, Val::from_str("hello"));
    }

    #[test]
    fn test_variable_pattern() {
        let match_expr = Expr::Match {
            value: Box::new(Expr::Val(Val::Int(100))),
            arms: vec![MatchArm {
                pattern: Pattern::Variable("x".to_string()),
                body: Box::new(Expr::Var("x".to_string())),
            }],
        };

        let mut env = VmContext::new();
        let result = match_expr.eval_with_ctx(&mut env).unwrap();
        assert_eq!(result, Val::Int(100));
    }

    #[test]
    fn test_wildcard_pattern() {
        let match_expr = Expr::Match {
            value: Box::new(Expr::Val(Val::Bool(true))),
            arms: vec![
                MatchArm {
                    pattern: Pattern::Literal(Val::Bool(false)),
                    body: Box::new(Expr::Val(Val::from_str("false"))),
                },
                MatchArm {
                    pattern: Pattern::Wildcard,
                    body: Box::new(Expr::Val(Val::from_str("wildcard"))),
                },
            ],
        };

        let mut env = VmContext::new();
        let result = match_expr.eval_with_ctx(&mut env).unwrap();
        assert_eq!(result, Val::from_str("wildcard"));
    }

    #[test]
    fn test_list_pattern() {
        // Test [first, second, ..rest] pattern
        let match_expr = Expr::Match {
            value: Box::new(Expr::Val(Val::list(Arc::from(vec![
                Val::Int(1),
                Val::Int(2),
                Val::Int(3),
                Val::Int(4),
            ])))),
            arms: vec![MatchArm {
                pattern: Pattern::List {
                    patterns: vec![
                        Pattern::Variable("first".to_string()),
                        Pattern::Variable("second".to_string()),
                    ],
                    rest: Some("rest".to_string()),
                },
                body: Box::new(Expr::Var("first".to_string())),
            }],
        };

        let mut env = VmContext::new();
        let result = match_expr.eval_with_ctx(&mut env).unwrap();
        assert_eq!(result, Val::Int(1));
    }

    #[test]
    fn test_map_pattern() {
        // Test {"name": name, "age": age} pattern
        let mut map = std::collections::HashMap::new();
        map.insert("name".to_string(), Val::from_str("Alice"));
        map.insert("age".to_string(), Val::Int(30));

        let match_expr = Expr::Match {
            value: Box::new(Expr::Val(map.into())),
            arms: vec![MatchArm {
                pattern: Pattern::Map {
                    patterns: vec![
                        ("name".to_string(), Pattern::Variable("name".to_string())),
                        ("age".to_string(), Pattern::Variable("age".to_string())),
                    ],
                    rest: None,
                },
                body: Box::new(Expr::Var("name".to_string())),
            }],
        };

        let mut env = VmContext::new();
        let result = match_expr.eval_with_ctx(&mut env).unwrap();
        assert_eq!(result, Val::from_str("Alice"));
    }

    #[test]
    fn test_or_pattern() {
        let match_expr = Expr::Match {
            value: Box::new(Expr::Val(Val::Int(2))),
            arms: vec![
                MatchArm {
                    pattern: Pattern::Or(vec![
                        Pattern::Literal(Val::Int(1)),
                        Pattern::Literal(Val::Int(2)),
                        Pattern::Literal(Val::Int(3)),
                    ]),
                    body: Box::new(Expr::Val(Val::from_str("one_two_three"))),
                },
                MatchArm {
                    pattern: Pattern::Wildcard,
                    body: Box::new(Expr::Val(Val::from_str("other"))),
                },
            ],
        };

        let mut env = VmContext::new();
        let result = match_expr.eval_with_ctx(&mut env).unwrap();
        assert_eq!(result, Val::from_str("one_two_three"));
    }

    #[test]
    fn test_guard_pattern() {
        let match_expr = Expr::Match {
            value: Box::new(Expr::Val(Val::Int(15))),
            arms: vec![
                MatchArm {
                    pattern: Pattern::Guard {
                        pattern: Box::new(Pattern::Variable("x".to_string())),
                        guard: Box::new(Expr::Bin(
                            Box::new(Expr::Var("x".to_string())),
                            BinOp::Gt,
                            Box::new(Expr::Val(Val::Int(10))),
                        )),
                    },
                    body: Box::new(Expr::Val(Val::from_str("greater_than_10"))),
                },
                MatchArm {
                    pattern: Pattern::Wildcard,
                    body: Box::new(Expr::Val(Val::from_str("not_greater"))),
                },
            ],
        };

        let mut env = VmContext::new();
        let result = match_expr.eval_with_ctx(&mut env).unwrap();
        assert_eq!(result, Val::from_str("greater_than_10"));
    }

    #[test]
    fn test_range_pattern() {
        let match_expr = Expr::Match {
            value: Box::new(Expr::Val(Val::Int(25))),
            arms: vec![
                MatchArm {
                    pattern: Pattern::Range {
                        start: Box::new(Expr::Val(Val::Int(0))),
                        end: Box::new(Expr::Val(Val::Int(18))),
                        inclusive: true,
                    },
                    body: Box::new(Expr::Val(Val::from_str("child"))),
                },
                MatchArm {
                    pattern: Pattern::Range {
                        start: Box::new(Expr::Val(Val::Int(19))),
                        end: Box::new(Expr::Val(Val::Int(64))),
                        inclusive: true,
                    },
                    body: Box::new(Expr::Val(Val::from_str("adult"))),
                },
                MatchArm {
                    pattern: Pattern::Wildcard,
                    body: Box::new(Expr::Val(Val::from_str("senior"))),
                },
            ],
        };

        let mut env = VmContext::new();
        let result = match_expr.eval_with_ctx(&mut env).unwrap();
        assert_eq!(result, Val::from_str("adult"));
    }

    #[test]
    fn test_no_match_error() {
        let match_expr = Expr::Match {
            value: Box::new(Expr::Val(Val::Int(99))),
            arms: vec![
                MatchArm {
                    pattern: Pattern::Literal(Val::Int(1)),
                    body: Box::new(Expr::Val(Val::from_str("one"))),
                },
                MatchArm {
                    pattern: Pattern::Literal(Val::Int(2)),
                    body: Box::new(Expr::Val(Val::from_str("two"))),
                },
            ],
        };

        let mut env = VmContext::new();
        let result = match_expr.eval_with_ctx(&mut env);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No pattern matched"));
    }

    #[test]
    fn test_complex_nested_pattern() {
        // Test matching nested structure like {"users": [{"name": name}, ...]}
        let user1 = {
            let mut user = std::collections::HashMap::new();
            user.insert("name".to_string(), Val::from_str("Alice"));
            user.insert("id".to_string(), Val::Int(1));
            user.into()
        };

        let user2 = {
            let mut user = std::collections::HashMap::new();
            user.insert("name".to_string(), Val::from_str("Bob"));
            user.insert("id".to_string(), Val::Int(2));
            user.into()
        };

        let mut data = std::collections::HashMap::new();
        data.insert("users".to_string(), Val::list(Arc::from(vec![user1, user2])));
        data.insert("count".to_string(), Val::Int(2));

        let match_expr = Expr::Match {
            value: Box::new(Expr::Val(data.into())),
            arms: vec![MatchArm {
                pattern: Pattern::Map {
                    patterns: vec![(
                        "users".to_string(),
                        Pattern::List {
                            patterns: vec![Pattern::Map {
                                patterns: vec![("name".to_string(), Pattern::Variable("first_name".to_string()))],
                                rest: None,
                            }],
                            rest: Some("other_users".to_string()),
                        },
                    )],
                    rest: None,
                },
                body: Box::new(Expr::Var("first_name".to_string())),
            }],
        };

        let mut env = VmContext::new();
        let result = match_expr.eval_with_ctx(&mut env).unwrap();
        assert_eq!(result, Val::from_str("Alice"));
    }

    #[test]
    fn test_float_range_pattern() {
        let match_expr = Expr::Match {
            value: Box::new(Expr::Val(Val::Float(85.5))),
            arms: vec![
                MatchArm {
                    pattern: Pattern::Range {
                        start: Box::new(Expr::Val(Val::Float(0.0))),
                        end: Box::new(Expr::Val(Val::Float(60.0))),
                        inclusive: false,
                    },
                    body: Box::new(Expr::Val(Val::from_str("fail"))),
                },
                MatchArm {
                    pattern: Pattern::Range {
                        start: Box::new(Expr::Val(Val::Float(60.0))),
                        end: Box::new(Expr::Val(Val::Float(80.0))),
                        inclusive: false,
                    },
                    body: Box::new(Expr::Val(Val::from_str("pass"))),
                },
                MatchArm {
                    pattern: Pattern::Range {
                        start: Box::new(Expr::Val(Val::Float(80.0))),
                        end: Box::new(Expr::Val(Val::Float(100.0))),
                        inclusive: true,
                    },
                    body: Box::new(Expr::Val(Val::from_str("excellent"))),
                },
                MatchArm {
                    pattern: Pattern::Wildcard,
                    body: Box::new(Expr::Val(Val::from_str("invalid"))),
                },
            ],
        };

        let mut env = VmContext::new();
        let result = match_expr.eval_with_ctx(&mut env).unwrap();
        assert_eq!(result, Val::from_str("excellent"));

        // Test boundary cases
        let boundary_tests = vec![
            (59.9, "fail"),       // Just below 60
            (60.0, "pass"),       // Exactly 60 (exclusive range start)
            (79.9, "pass"),       // Just below 80
            (80.0, "excellent"),  // Exactly 80 (inclusive range start)
            (100.0, "excellent"), // Exactly 100 (inclusive range end)
            (100.1, "invalid"),   // Above 100
        ];

        for (value, expected) in boundary_tests {
            let match_expr = Expr::Match {
                value: Box::new(Expr::Val(Val::Float(value))),
                arms: vec![
                    MatchArm {
                        pattern: Pattern::Range {
                            start: Box::new(Expr::Val(Val::Float(0.0))),
                            end: Box::new(Expr::Val(Val::Float(60.0))),
                            inclusive: false,
                        },
                        body: Box::new(Expr::Val(Val::from_str("fail"))),
                    },
                    MatchArm {
                        pattern: Pattern::Range {
                            start: Box::new(Expr::Val(Val::Float(60.0))),
                            end: Box::new(Expr::Val(Val::Float(80.0))),
                            inclusive: false,
                        },
                        body: Box::new(Expr::Val(Val::from_str("pass"))),
                    },
                    MatchArm {
                        pattern: Pattern::Range {
                            start: Box::new(Expr::Val(Val::Float(80.0))),
                            end: Box::new(Expr::Val(Val::Float(100.0))),
                            inclusive: true,
                        },
                        body: Box::new(Expr::Val(Val::from_str("excellent"))),
                    },
                    MatchArm {
                        pattern: Pattern::Wildcard,
                        body: Box::new(Expr::Val(Val::from_str("invalid"))),
                    },
                ],
            };

            let mut env = VmContext::new();
            let result = match_expr.eval_with_ctx(&mut env).unwrap();
            assert_eq!(
                result,
                Val::from_str(expected),
                "Value {} should match {}",
                value,
                expected
            );
        }
    }
}
