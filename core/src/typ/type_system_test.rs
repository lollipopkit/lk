#[cfg(test)]
mod tests {
    use crate::token::Tokenizer;
    use crate::typ::type_system::*;
    use crate::val::Type;
    use std::collections::HashMap;

    #[test]
    fn test_type_parsing_primitives() {
        assert_eq!(Type::parse("Int"), Some(Type::Int));
        assert_eq!(Type::parse("Float"), Some(Type::Float));
        assert_eq!(Type::parse("String"), Some(Type::String));
        assert_eq!(Type::parse("Bool"), Some(Type::Bool));
        assert_eq!(Type::parse("Nil"), Some(Type::Nil));
        assert_eq!(Type::parse("Any"), Some(Type::Any));
    }

    #[test]
    fn test_type_parsing_generics() {
        // List<Int>
        if let Some(Type::List(inner)) = Type::parse("List<Int>") {
            assert_eq!(*inner, Type::Int);
        } else {
            panic!("Expected List<Int>");
        }

        // Map<String, Int>
        if let Some(Type::Map(key, value)) = Type::parse("Map<String, Int>") {
            assert_eq!(*key, Type::String);
            assert_eq!(*value, Type::Int);
        } else {
            panic!("Expected Map<String, Int>");
        }

        if let Some(Type::Boxed(inner)) = Type::parse("Box<Float>") {
            assert_eq!(*inner, Type::Float);
        } else {
            panic!("Expected Box<Float>");
        }
    }

    #[test]
    fn test_type_parsing_optional() {
        // ?Int
        if let Some(Type::Optional(inner)) = Type::parse("?Int") {
            assert_eq!(*inner, Type::Int);
        } else {
            panic!("Expected ?Int");
        }

        // Int? (suffix form)
        if let Some(Type::Optional(inner)) = Type::parse("Int?") {
            assert_eq!(*inner, Type::Int);
        } else {
            panic!("Expected Int?");
        }

        // ?List<String>
        if let Some(Type::Optional(inner)) = Type::parse("?List<String>") {
            if let Type::List(list_inner) = inner.as_ref() {
                assert_eq!(**list_inner, Type::String);
            } else {
                panic!("Expected Optional<List<String>>");
            }
        } else {
            panic!("Expected ?List<String>");
        }

        // List<String>? (suffix form)
        if let Some(Type::Optional(inner)) = Type::parse("List<String>?") {
            if let Type::List(list_inner) = inner.as_ref() {
                assert_eq!(**list_inner, Type::String);
            } else {
                panic!("Expected Optional<List<String>> (suffix)");
            }
        } else {
            panic!("Expected List<String>?");
        }
    }

    #[test]
    fn test_type_parsing_union() {
        // Int | String
        if let Some(Type::Union(types)) = Type::parse("Int | String") {
            assert_eq!(types.len(), 2);
            assert_eq!(types[0], Type::Int);
            assert_eq!(types[1], Type::String);
        } else {
            panic!("Expected Int | String");
        }

        // Int | String | Bool
        if let Some(Type::Union(types)) = Type::parse("Int | String | Bool") {
            assert_eq!(types.len(), 3);
            assert_eq!(types[0], Type::Int);
            assert_eq!(types[1], Type::String);
            assert_eq!(types[2], Type::Bool);
        } else {
            panic!("Expected Int | String | Bool");
        }
    }

    #[test]
    fn test_type_parsing_function() {
        // () -> Int
        if let Some(Type::Function {
            params,
            named_params,
            return_type,
        }) = Type::parse("() -> Int")
        {
            assert_eq!(params.len(), 0);
            assert!(named_params.is_empty());
            assert_eq!(*return_type, Type::Int);
        } else {
            panic!("Expected () -> Int");
        }

        // (Int, String) -> Bool
        if let Some(Type::Function {
            params,
            named_params,
            return_type,
        }) = Type::parse("(Int, String) -> Bool")
        {
            assert_eq!(params.len(), 2);
            assert_eq!(params[0], Type::Int);
            assert_eq!(params[1], Type::String);
            assert!(named_params.is_empty());
            assert_eq!(*return_type, Type::Bool);
        } else {
            panic!("Expected (Int, String) -> Bool");
        }
    }

    #[test]
    fn test_type_parsing_variables() {
        if let Some(Type::Variable(name)) = Type::parse("'T") {
            assert_eq!(name, "T");
        } else {
            panic!("Expected 'T");
        }

        if let Some(Type::Variable(name)) = Type::parse("'Key") {
            assert_eq!(name, "Key");
        } else {
            panic!("Expected 'Key");
        }
    }

    #[test]
    fn test_type_parsing_named() {
        if let Some(Type::Named(name)) = Type::parse("UserId") {
            assert_eq!(name, "UserId");
        } else {
            panic!("Expected UserId");
        }

        if let Some(Type::Named(name)) = Type::parse("MyCustomType") {
            assert_eq!(name, "MyCustomType");
        } else {
            panic!("Expected MyCustomType");
        }
    }

    #[test]
    fn test_type_display() {
        assert_eq!(Type::Int.display(), "Int");
        assert_eq!(Type::List(Box::new(Type::String)).display(), "List<String>");
        assert_eq!(
            Type::Map(Box::new(Type::String), Box::new(Type::Int)).display(),
            "Map<String, Int>"
        );
        assert_eq!(Type::Optional(Box::new(Type::Bool)).display(), "Bool?");
        assert_eq!(Type::Union(vec![Type::Int, Type::String]).display(), "Int | String");
        assert_eq!(
            Type::Function {
                params: vec![Type::Int, Type::String],
                named_params: Vec::new(),
                return_type: Box::new(Type::Bool),
            }
            .display(),
            "(Int, String) -> Bool"
        );
        assert_eq!(Type::Variable("T".to_string()).display(), "'T");
        assert_eq!(Type::Named("UserId".to_string()).display(), "UserId");
        assert_eq!(Type::Boxed(Box::new(Type::Int)).display(), "Box<Int>");
    }

    #[test]
    fn test_type_assignability() {
        // Same types
        assert!(Type::Int.is_assignable_to(&Type::Int));
        assert!(Type::String.is_assignable_to(&Type::String));

        // Any accepts everything
        assert!(Type::Int.is_assignable_to(&Type::Any));
        assert!(Type::String.is_assignable_to(&Type::Any));

        // Optional types
        assert!(Type::Int.is_assignable_to(&Type::Optional(Box::new(Type::Int))));
        assert!(!Type::String.is_assignable_to(&Type::Optional(Box::new(Type::Int))));

        // Union types
        let int_or_string = Type::Union(vec![Type::Int, Type::String]);
        assert!(Type::Int.is_assignable_to(&int_or_string));
        assert!(Type::String.is_assignable_to(&int_or_string));

        // Numeric promotion
        assert!(Type::Int.is_assignable_to(&Type::Float));
        assert!(!Type::Float.is_assignable_to(&Type::Int));

        // Boxed behaviour
        let boxed_any = Type::Boxed(Box::new(Type::Any));
        assert!(Type::Float.is_assignable_to(&boxed_any));
        assert!(Type::Int.is_assignable_to(&boxed_any));
        let boxed_float = Type::Boxed(Box::new(Type::Float));
        assert!(boxed_float.is_assignable_to(&Type::Float));
        assert!(!Type::Bool.is_assignable_to(&int_or_string));

        // Container types (covariant)
        let list_int = Type::List(Box::new(Type::Int));
        let list_any = Type::List(Box::new(Type::Any));
        assert!(list_int.is_assignable_to(&list_any));
    }

    #[test]
    fn test_tokenizer_new_tokens() {
        // Test type keywords
        let tokens = Tokenizer::tokenize("type trait impl").unwrap();
        assert_eq!(tokens.len(), 3);
        assert!(matches!(tokens[0], crate::token::Token::Type));
        assert!(matches!(tokens[1], crate::token::Token::Trait));
        assert!(matches!(tokens[2], crate::token::Token::Impl));

        // Test type operators
        let tokens = Tokenizer::tokenize("Int | String ? ->").unwrap();
        assert_eq!(tokens.len(), 5);
        assert!(matches!(tokens[1], crate::token::Token::Pipe));
        assert!(matches!(tokens[3], crate::token::Token::Question));
        assert!(matches!(tokens[4], crate::token::Token::FnArrow));

        // Test complex type annotation
        let tokens = Tokenizer::tokenize("let x: ?List<Int | String> = nil;").unwrap();
        // Should contain Question, List identifier, Lt, Int, Pipe, String, Gt, etc.
        assert!(tokens.iter().any(|t| matches!(t, crate::token::Token::Question)));
        assert!(tokens.iter().any(|t| matches!(t, crate::token::Token::Pipe)));
        assert!(tokens.iter().any(|t| matches!(t, crate::token::Token::Lt)));
        assert!(tokens.iter().any(|t| matches!(t, crate::token::Token::Gt)));
    }

    #[test]
    fn test_type_registry() {
        let mut registry = TypeRegistry::new();

        // Test type alias
        let alias = TypeAlias {
            name: "UserId".to_string(),
            target_type: Type::Int,
        };
        registry.register_type_alias(alias);
        assert_eq!(registry.resolve_type("UserId"), Some(Type::Int));

        // Test trait definition
        let mut methods = HashMap::new();
        methods.insert(
            "display".to_string(),
            Type::Function {
                params: vec![],
                named_params: Vec::new(),
                return_type: Box::new(Type::String),
            },
        );
        let trait_def = TraitDef {
            name: "Display".to_string(),
            methods,
        };
        registry.register_trait(trait_def);

        // Test fresh type variables
        let var1 = registry.fresh_type_var();
        let var2 = registry.fresh_type_var();
        assert!(matches!(var1, Type::Variable(_)));
        assert!(matches!(var2, Type::Variable(_)));
        // Should be different variables
        if let (Type::Variable(name1), Type::Variable(name2)) = (&var1, &var2) {
            assert_ne!(name1, name2);
        }
    }

    #[test]
    fn test_type_inference() {
        let registry = TypeRegistry::new();
        let mut engine = TypeInferenceEngine::new(registry);

        // Test basic unification
        let var1 = engine.fresh_type_var();
        engine.add_constraint(var1.clone(), Type::Int);

        let substitutions = engine.solve_constraints().unwrap();

        if let Type::Variable(name) = var1 {
            assert_eq!(substitutions.get(&name), Some(&Type::Int));
        }
    }

    #[test]
    fn test_type_substitution() {
        let mut substitutions = HashMap::new();
        substitutions.insert("T".to_string(), Type::Int);
        substitutions.insert("U".to_string(), Type::String);

        // Test variable substitution
        let var_t = Type::Variable("T".to_string());
        assert_eq!(var_t.substitute(&substitutions), Type::Int);

        // Test complex type substitution
        let list_t = Type::List(Box::new(Type::Variable("T".to_string())));
        let expected = Type::List(Box::new(Type::Int));
        assert_eq!(list_t.substitute(&substitutions), expected);

        // Test function type substitution
        let func_type = Type::Function {
            params: vec![Type::Variable("T".to_string()), Type::Variable("U".to_string())],
            named_params: Vec::new(),
            return_type: Box::new(Type::Variable("T".to_string())),
        };
        let expected_func = Type::Function {
            params: vec![Type::Int, Type::String],
            named_params: Vec::new(),
            return_type: Box::new(Type::Int),
        };
        assert_eq!(func_type.substitute(&substitutions), expected_func);
    }
}
