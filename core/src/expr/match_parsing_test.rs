#[cfg(test)]
mod tests {
    use crate::{
        ast::Parser,
        expr::{Expr, Pattern},
        token::Tokenizer,
        val::Val,
    };

    fn parse_expr(input: &str) -> Expr {
        let tokens = Tokenizer::tokenize(input).unwrap();
        Parser::new(&tokens).parse().unwrap()
    }

    #[test]
    fn test_parse_simple_match() {
        let expr = parse_expr("match x { 1 => \"one\", 2 => \"two\", _ => \"other\" }");

        if let Expr::Match { value, arms } = expr {
            // Check value is variable 'x'
            assert!(matches!(value.as_ref(), Expr::Var(name) if name == "x"));

            // Check we have 3 arms
            assert_eq!(arms.len(), 3);

            // Check first arm: 1 => "one"
            assert!(matches!(arms[0].pattern, Pattern::Literal(Val::Int(1))));
            if let Expr::Val(Val::Str(s)) = &*arms[0].body {
                assert_eq!(s.as_ref(), "one");
            } else {
                panic!("Expected string literal");
            }

            // Check second arm: 2 => "two"
            assert!(matches!(arms[1].pattern, Pattern::Literal(Val::Int(2))));

            // Check third arm: _ => "other"
            assert!(matches!(arms[2].pattern, Pattern::Wildcard));
        } else {
            panic!("Expected match expression");
        }
    }

    #[test]
    fn test_parse_match_with_variables() {
        let expr = parse_expr("match value { x => x, y => y + 1 }");

        if let Expr::Match { value: _, arms } = expr {
            assert_eq!(arms.len(), 2);

            // Check first arm: x => x
            if let Pattern::Variable(name) = &arms[0].pattern {
                assert_eq!(name, "x");
            } else {
                panic!("Expected variable pattern");
            }
        } else {
            panic!("Expected match expression");
        }
    }

    #[test]
    fn test_parse_match_with_list_pattern() {
        let expr = parse_expr("match list { [first, second] => first, [head, ..tail] => head }");

        if let Expr::Match { value: _, arms } = expr {
            assert_eq!(arms.len(), 2);

            // Check first arm: [first, second] => first
            if let Pattern::List { patterns, rest } = &arms[0].pattern {
                assert_eq!(patterns.len(), 2);
                assert!(rest.is_none());
                assert!(matches!(patterns[0], Pattern::Variable(ref name) if name == "first"));
                assert!(matches!(patterns[1], Pattern::Variable(ref name) if name == "second"));
            } else {
                panic!("Expected list pattern");
            }

            // Check second arm: [head, ..tail] => head
            if let Pattern::List { patterns, rest } = &arms[1].pattern {
                assert_eq!(patterns.len(), 1);
                assert_eq!(rest.as_ref().unwrap(), "tail");
                assert!(matches!(patterns[0], Pattern::Variable(ref name) if name == "head"));
            } else {
                panic!("Expected list pattern with rest");
            }
        } else {
            panic!("Expected match expression");
        }
    }

    #[test]
    fn test_parse_match_with_map_pattern() {
        let expr = parse_expr("match user { {\"name\": name, \"age\": age} => name }");

        if let Expr::Match { value: _, arms } = expr {
            assert_eq!(arms.len(), 1);

            // Check map pattern: {"name": name, "age": age}
            if let Pattern::Map { patterns, rest } = &arms[0].pattern {
                assert_eq!(patterns.len(), 2);
                assert!(rest.is_none());

                assert_eq!(patterns[0].0, "name");
                assert!(matches!(patterns[0].1, Pattern::Variable(ref name) if name == "name"));

                assert_eq!(patterns[1].0, "age");
                assert!(matches!(patterns[1].1, Pattern::Variable(ref name) if name == "age"));
            } else {
                panic!("Expected map pattern");
            }
        } else {
            panic!("Expected match expression");
        }
    }

    #[test]
    fn test_parse_match_with_or_pattern() {
        let expr = parse_expr("match x { 1 | 2 | 3 => \"small\", _ => \"large\" }");

        if let Expr::Match { value: _, arms } = expr {
            assert_eq!(arms.len(), 2);

            // Check or pattern: 1 | 2 | 3
            if let Pattern::Or(patterns) = &arms[0].pattern {
                assert_eq!(patterns.len(), 3);
                assert!(matches!(patterns[0], Pattern::Literal(Val::Int(1))));
                assert!(matches!(patterns[1], Pattern::Literal(Val::Int(2))));
                assert!(matches!(patterns[2], Pattern::Literal(Val::Int(3))));
            } else {
                panic!("Expected or pattern");
            }
        } else {
            panic!("Expected match expression");
        }
    }

    #[test]
    fn test_parse_match_with_guard() {
        let expr = parse_expr("match x { n if n > 10 => \"big\", _ => \"small\" }");

        if let Expr::Match { value: _, arms } = expr {
            assert_eq!(arms.len(), 2);

            // Check guard pattern: n if n > 10
            if let Pattern::Guard { pattern, guard } = &arms[0].pattern {
                assert!(matches!(pattern.as_ref(), Pattern::Variable(name) if name == "n"));

                // Check guard expression: n > 10
                if let Expr::Bin(left, op, right) = guard.as_ref() {
                    assert!(matches!(left.as_ref(), Expr::Var(name) if name == "n"));
                    assert!(matches!(op, crate::op::BinOp::Gt));
                    assert!(matches!(right.as_ref(), Expr::Val(Val::Int(10))));
                } else {
                    panic!("Expected binary expression in guard");
                }
            } else {
                panic!("Expected guard pattern");
            }
        } else {
            panic!("Expected match expression");
        }
    }

    #[test]
    fn test_parse_match_with_range_pattern() {
        let expr = parse_expr("match age { 0..18 => \"child\", 18..=64 => \"adult\", _ => \"senior\" }");

        if let Expr::Match { value: _, arms } = expr {
            assert_eq!(arms.len(), 3);

            // Check range pattern: 0..18
            if let Pattern::Range { start, end, inclusive } = &arms[0].pattern {
                assert!(!inclusive);
                assert!(matches!(start.as_ref(), Expr::Val(Val::Int(0))));
                assert!(matches!(end.as_ref(), Expr::Val(Val::Int(18))));
            } else {
                panic!("Expected range pattern");
            }

            // Check inclusive range pattern: 18..=64
            if let Pattern::Range { start, end, inclusive } = &arms[1].pattern {
                assert!(*inclusive);
                assert!(matches!(start.as_ref(), Expr::Val(Val::Int(18))));
                assert!(matches!(end.as_ref(), Expr::Val(Val::Int(64))));
            } else {
                panic!("Expected inclusive range pattern");
            }
        } else {
            panic!("Expected match expression");
        }
    }

    #[test]
    fn test_parse_nested_match() {
        let expr = parse_expr("match x { y => match y { 1 => \"one\", _ => \"other\" }, 99 => \"ninety_nine\" }");

        if let Expr::Match { value: _, arms } = expr {
            assert_eq!(arms.len(), 2);

            // Check that first arm body is another match expression
            if let Expr::Match {
                value: inner_value,
                arms: inner_arms,
            } = arms[0].body.as_ref()
            {
                assert!(matches!(inner_value.as_ref(), Expr::Var(name) if name == "y"));
                assert_eq!(inner_arms.len(), 2);
            } else {
                panic!("Expected nested match expression");
            }
        } else {
            panic!("Expected match expression");
        }
    }

    #[test]
    fn test_parse_complex_pattern() {
        let expr = parse_expr("match data { {\"users\": [first, ..rest], \"count\": n} => first }");

        if let Expr::Match { value: _, arms } = expr {
            assert_eq!(arms.len(), 1);

            // Check complex nested pattern
            if let Pattern::Map { patterns, rest } = &arms[0].pattern {
                assert_eq!(patterns.len(), 2);
                assert!(rest.is_none());

                // Check "users" pattern is a list pattern
                assert_eq!(patterns[0].0, "users");
                if let Pattern::List {
                    patterns: list_patterns,
                    rest: list_rest,
                } = &patterns[0].1
                {
                    assert_eq!(list_patterns.len(), 1);
                    assert_eq!(list_rest.as_ref().unwrap(), "rest");
                    assert!(matches!(list_patterns[0], Pattern::Variable(ref name) if name == "first"));
                } else {
                    panic!("Expected list pattern for users");
                }

                // Check "count" pattern is a variable
                assert_eq!(patterns[1].0, "count");
                assert!(matches!(patterns[1].1, Pattern::Variable(ref name) if name == "n"));
            } else {
                panic!("Expected map pattern");
            }
        } else {
            panic!("Expected match expression");
        }
    }
}
