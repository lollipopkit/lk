#[cfg(test)]
mod tests {
    use crate::{ast::Parser, expr::Expr, token::Tokenizer};

    #[test]
    fn test_parse_select_with_guard() {
        let code = r#"
        select {
            case recv(ch) if true => 1;
            default => 0;
        }
        "#;
        let tokens = Tokenizer::tokenize(code).unwrap();
        let expr = Parser::new(&tokens).parse().unwrap();

        if let Expr::Select { cases, default_case } = expr {
            assert_eq!(cases.len(), 1);
            assert!(default_case.is_some());
            assert!(cases[0].guard.is_some(), "guard should be present");
        } else {
            panic!("Expected select expression");
        }
    }
}

