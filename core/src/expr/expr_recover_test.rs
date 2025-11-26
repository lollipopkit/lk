#[cfg(test)]
mod tests {
    use crate::{ast::Parser, token::Tokenizer};

    #[test]
    fn test_expr_recovery_multiple_errors() {
        // This input has several expression issues but tokenizes fine:
        // - '1 + * 2' invalid use of '*'
        // - '(3 + )' missing rhs before ')'
        // - '4 && * 5' invalid rhs after '&&'
        let input = "1 + * 2, (3 + ), 4 && * 5";
        let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(input).expect("tokenize with spans");

        let errors = Parser::recover_expression_errors(&tokens, &spans, input);
        assert!(
            errors.len() >= 2,
            "expected multiple expression errors, got {}",
            errors.len()
        );

        // All errors should carry precise spans
        assert!(errors.iter().all(|e| e.span.is_some()));
    }

    #[test]
    fn test_expr_recovery_operator_boundaries() {
        // Ensure logical/comparison operators act as soft boundaries so that
        // multiple local issues can be surfaced
        let input = "(1 + ) && (2 * ) || 3 =="; // missing operands in both groups
        let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(input).expect("tokenize with spans");

        let errors = Parser::recover_expression_errors(&tokens, &spans, input);
        assert!(
            errors.len() >= 2,
            "expected multiple expression errors, got {}",
            errors.len()
        );
        assert!(errors.iter().all(|e| e.span.is_some()));
    }

    #[test]
    fn test_expr_recovery_numeric_path_segments_spans_aligned() {
        // Ensure token spans remain aligned when parsing numeric segments in identifier paths
        let input = "user.emails.0.company, data.1 + 2";
        let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(input).expect("tokenize with spans");

        // Regression: tokens and spans must be same length (previously mismatched on ints)
        assert_eq!(tokens.len(), spans.len(), "tokens and spans length mismatch");

        // Should not produce expression errors for a valid expression
        let errors = Parser::recover_expression_errors(&tokens, &spans, input);
        assert!(errors.is_empty(), "unexpected expression errors: {:?}", errors);
    }
}
