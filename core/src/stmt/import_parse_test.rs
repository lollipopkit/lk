#[cfg(test)]
mod tests {
    use crate::stmt::stmt_parser::StmtParser;
    use crate::token::Tokenizer;

    #[test]
    fn test_import_without_specifier_returns_error() {
        let src = "import";
        let tokens = Tokenizer::tokenize(src).expect("tokenize");
        let mut sp = StmtParser::new(&tokens);
        let result = sp.parse_program();
        assert!(result.is_err(), "expected parse error for bare 'import'");
    }

    #[test]
    fn test_import_from_without_source_returns_error() {
        let src = "import { a } from";
        let tokens = Tokenizer::tokenize(src).expect("tokenize");
        let mut sp = StmtParser::new(&tokens);
        let result = sp.parse_program();
        assert!(result.is_err(), "expected parse error for missing import source");
    }
}

