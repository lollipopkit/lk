#[cfg(test)]
mod tests {
    use crate::expr::Pattern;
    use crate::val::Type;
    use crate::typ::TypeChecker;

    #[test]
    fn test_or_pattern_common_bindings_union_types() {
        let mut tc = TypeChecker::new();

        // Pattern: [x] | {"name": x}
        let pat = Pattern::Or(vec![
            Pattern::List { patterns: vec![Pattern::Variable("x".to_string())], rest: None },
            Pattern::Map { patterns: vec![("name".to_string(), Pattern::Variable("x".to_string()))], rest: None },
        ]);

        // Provide unknown value type; binder will introduce type variables as needed
        let value_ty = Type::Any;
        tc.add_bindings_for_pattern(&pat, &value_ty).unwrap();

        // Expect `x` to be present with a Union type (two alternatives)
        let Some(x_ty) = tc.get_local_type("x") else { panic!("x not bound") };
        match x_ty {
            Type::Union(ts) => {
                // At least two different type variants should be present
                assert!(ts.len() >= 2);
            }
            other => panic!("expected union type for x, got {:?}", other),
        }
    }
}

