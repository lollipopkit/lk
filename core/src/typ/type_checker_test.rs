#[cfg(test)]
mod tests {
    use crate::{
        ast::Parser as ExprParser,
        token::Tokenizer,
        val::Type,
        typ::TypeChecker,
    };

    fn infer(src: &str) -> Type {
        let tokens = Tokenizer::tokenize(src).expect("tokenize");
        let expr = ExprParser::new(&tokens).parse().expect("parse expr");
        let mut tc = TypeChecker::new();
        tc.infer_resolved_type(&expr).expect("infer")
    }

    #[test]
    fn test_string_add_concatenation_rules() {
        // String + String => String
        assert_eq!(infer("\"a\" + \"b\""), Type::String);
        // String + Int => String
        assert_eq!(infer("\"a\" + 1"), Type::String);
        // Int + String => String
        assert_eq!(infer("1 + \"b\""), Type::String);
        // Var + String => String (var constrained to String)
        assert_eq!(infer("x + \"!\""), Type::String);
    }

    #[test]
    fn test_list_union_element_type() {
        // Mixed numeric list should infer union element type
        let ty = infer("[1, 2.0, 3]");
        match ty {
            Type::List(inner) => match *inner {
                Type::Union(ts) => {
                    // Expect Int and Float members
                    assert!(ts.contains(&Type::Int));
                    assert!(ts.contains(&Type::Float));
                }
                _ => panic!("expected union element type, got {:?}", *inner),
            },
            _ => panic!("expected List<...>, got {:?}", ty),
        }
    }

    #[test]
    fn test_map_union_value_type() {
        // Map with mixed value types should infer union value type
        let ty = infer("{\"a\": 1, \"b\": 2.0}");
        match ty {
            Type::Map(k, v) => {
                assert_eq!(*k, Type::String);
                match *v {
                    Type::Union(ts) => {
                        assert!(ts.contains(&Type::Int));
                        assert!(ts.contains(&Type::Float));
                    }
                    _ => panic!("expected union value type, got {:?}", *v),
                }
            }
            _ => panic!("expected Map<...>, got {:?}", ty),
        }
    }

    #[test]
    fn test_indexing_preserves_union_element_type() {
        // Indexing a mixed-element list should yield the union element type
        let ty = infer("([1, 2.0])[0]");
        match ty {
            Type::Union(ts) => {
                assert!(ts.contains(&Type::Int));
                assert!(ts.contains(&Type::Float));
            }
            _ => panic!("expected union type, got {:?}", ty),
        }
    }
}
