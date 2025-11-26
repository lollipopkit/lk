#[cfg(test)]
mod tests {
    use anyhow::Result;
    use crate::{
        stmt::stmt_parser::StmtParser,
        token::Tokenizer,
        typ::TypeChecker,
        val::Type,
    };

    #[test]
    fn test_infer_function_return_simple_int() -> Result<()> {
        let src = "fn consts() { return 42; }";
        let tokens = Tokenizer::tokenize(src)?;
        let mut parser = StmtParser::new(&tokens);
        let stmt = parser.parse_statement()?;

        let mut tc = TypeChecker::new();
        stmt.type_check(&mut tc)?;

        let ty = tc.get_local_type("consts").cloned().expect("function type missing");
        match ty {
            Type::Function {
                params,
                named_params,
                return_type,
            } => {
                assert!(params.is_empty());
                assert!(named_params.is_empty());
                assert_eq!(*return_type, Type::Int);
            }
            other => panic!("expected Function type, got {:?}", other),
        }
        Ok(())
    }

    #[test]
    fn test_infer_function_return_union() -> Result<()> {
        let src = r#"
            fn foo(flag) {
                if (flag) { return 1; } else { return "x"; }
            }
        "#;
        let tokens = Tokenizer::tokenize(src)?;
        let mut parser = StmtParser::new(&tokens);
        let stmt = parser.parse_statement()?;

        let mut tc = TypeChecker::new();
        stmt.type_check(&mut tc)?;

        let ty = tc.get_local_type("foo").cloned().expect("function type missing");
        match ty {
            Type::Function { return_type, .. } => {
                match *return_type {
                    Type::Union(mut ts) => {
                        // Expect Int | String (order normalized by display)
                        ts.sort_by_key(|t| t.display());
                        let displays: Vec<String> = ts.iter().map(|t| t.display()).collect();
                        assert_eq!(displays, vec!["Int".to_string(), "String".to_string()]);
                    }
                    other => panic!("expected union return type, got {:?}", other),
                }
            }
            other => panic!("expected Function type, got {:?}", other),
        }
        Ok(())
    }

    #[test]
    fn test_infer_function_return_nil_when_no_return() -> Result<()> {
        let src = "fn noop() { let x = 1; }";
        let tokens = Tokenizer::tokenize(src)?;
        let mut parser = StmtParser::new(&tokens);
        let stmt = parser.parse_statement()?;

        let mut tc = TypeChecker::new();
        stmt.type_check(&mut tc)?;

        let ty = tc.get_local_type("noop").cloned().expect("function type missing");
        match ty {
            Type::Function { return_type, .. } => {
                assert_eq!(*return_type, Type::Nil);
            }
            other => panic!("expected Function type, got {:?}", other),
        }
        Ok(())
    }
}
