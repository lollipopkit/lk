#[cfg(test)]
mod tests {
    use crate::{stmt::stmt_parser::StmtParser, token::Tokenizer};

    #[test]
    fn test_stmt_recovery_collects_multiple_errors() {
        // Program with multiple statement-level errors:
        // - if with incomplete condition
        // - let without initializer expression
        // - return with incomplete expression
        let code = r#"
let a = 1;
if (a > ) { return; }
let b = ;
return 1 + ;
"#;
        let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(code).expect("tokenize with spans");
        let mut parser = StmtParser::new_with_spans(&tokens, &spans);
        let (_stmts, errs) = parser.parse_program_recovering_with_enhanced_errors(code);

        // Expect multiple errors collected (at least 2 with current recovery strategy)
        assert!(errs.len() >= 2, "expected multiple errors, got {}", errs.len());
        // Every error should have a span
        assert!(errs.iter().all(|e| e.span.is_some()));
    }
}
