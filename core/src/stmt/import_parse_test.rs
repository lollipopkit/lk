#[cfg(test)]
mod tests {
    use crate::stmt::stmt_parser::StmtParser;
    use crate::token::Tokenizer;

    #[test]
    fn test_use_without_specifier_returns_error() {
        let src = "use";
        let tokens = Tokenizer::tokenize(src).expect("tokenize");
        let mut sp = StmtParser::new(&tokens);
        let result = sp.parse_program();
        assert!(result.is_err(), "expected parse error for bare 'use'");
    }

    #[test]
    fn test_use_from_without_source_returns_error() {
        let src = "use { a } from";
        let tokens = Tokenizer::tokenize(src).expect("tokenize");
        let mut sp = StmtParser::new(&tokens);
        let result = sp.parse_program();
        assert!(result.is_err(), "expected parse error for missing use source");
    }

    #[test]
    fn test_old_import_keyword_is_not_supported() {
        let src = "import math;";
        let tokens = Tokenizer::tokenize(src).expect("tokenize");
        let mut sp = StmtParser::new(&tokens);
        let result = sp.parse_program();
        assert!(result.is_err(), "old import keyword should not parse");
    }
}
