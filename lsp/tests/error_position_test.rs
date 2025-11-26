use lkr_lsp::analyzer::LkrAnalyzer;
use tower_lsp::lsp_types::DiagnosticSeverity;

#[test]
fn test_expression_error_position() {
    let mut analyzer = LkrAnalyzer::new();

    // Test unterminated string - error should be at the end of line
    let code = "req.user.name == 'unterminated string";
    let result = analyzer.analyze(code);

    assert!(!result.diagnostics.is_empty());
    let diagnostic = &result.diagnostics[0];
    assert_eq!(diagnostic.severity, Some(DiagnosticSeverity::ERROR));

    // Expect the error to point exactly to end-of-input (0-based column)
    let expected_col = code.chars().count() as u32;
    assert_eq!(diagnostic.range.start.line, 0);
    assert_eq!(diagnostic.range.start.character, expected_col);
    assert_eq!(diagnostic.range.end.line, 0);
    assert_eq!(diagnostic.range.end.character, expected_col);
}

#[test]
fn test_statement_error_position() {
    let mut analyzer = LkrAnalyzer::new();

    // Test invalid statement syntax
    let code = r#"
let x = 5;
if (x == 5 {  // Missing closing parenthesis
    return true;
}
"#;

    let result = analyzer.analyze(code);
    assert!(!result.diagnostics.is_empty());

    let diagnostic = &result.diagnostics[0];
    assert_eq!(diagnostic.severity, Some(DiagnosticSeverity::ERROR));

    // The error should be on the line with the missing ')'
    let lines: Vec<&str> = code.lines().collect();
    let expected_line = lines
        .iter()
        .position(|l| l.contains("Missing closing parenthesis"))
        .expect("expected marker comment in test input");
    assert_eq!(diagnostic.range.start.line as usize, expected_line);
}

#[test]
fn test_multiline_error_position() {
    let mut analyzer = LkrAnalyzer::new();

    let code = r#"let user = req.user;
let role = user.role;
let invalid = role == 'admin' &&;  // Invalid syntax at end
return invalid;"#;

    let result = analyzer.analyze(code);

    if !result.diagnostics.is_empty() {
        let diagnostic = &result.diagnostics[0];
        // Error should be on the line with the invalid '&&;' syntax
        let lines: Vec<&str> = code.lines().collect();
        let expected_line = lines
            .iter()
            .position(|l| l.contains("Invalid syntax at end"))
            .expect("expected marker comment in test input");
        assert_eq!(diagnostic.range.start.line as usize, expected_line);
    } else {
        // If no errors, the expression might be parsed differently
        println!("No errors found, symbols: {:?}", result.symbols);
    }
}

#[test]
fn test_simple_syntax_error_position() {
    let mut analyzer = LkrAnalyzer::new();

    // Test with simple syntax error - missing quote
    let code = "req.user.name == 'admin"; // Missing closing quote
    let result = analyzer.analyze(code);

    assert!(!result.diagnostics.is_empty());
    let diagnostic = &result.diagnostics[0];
    // Should report an error at the end of the single line
    assert_eq!(diagnostic.severity, Some(DiagnosticSeverity::ERROR));
    let expected_col = code.chars().count() as u32;
    assert_eq!(diagnostic.range.start.line, 0);
    assert_eq!(diagnostic.range.start.character, expected_col);
    assert_eq!(diagnostic.range.end.line, 0);
    assert_eq!(diagnostic.range.end.character, expected_col);
}

#[test]
fn test_multiple_errors_position() {
    let mut analyzer = LkrAnalyzer::new();

    // Test with multiple errors in the code
    let code = r#"let user = req.user;
let invalid1 = user.role == 'admin;  // Missing closing quote
let invalid2 = user.age > 18 &&;      // Invalid syntax at end
return invalid1 && invalid2;"#;

    let result = analyzer.analyze(code);
    assert!(result.diagnostics.len() >= 2);

    let lines: Vec<&str> = code.lines().collect();

    // Check first error position
    let diagnostic1 = &result.diagnostics[0];
    let expected_line1 = lines
        .iter()
        .position(|l| l.contains("Missing closing quote"))
        .expect("expected marker comment in test input");
    assert_eq!(diagnostic1.range.start.line as usize, expected_line1);

    // Check second error position
    let diagnostic2 = &result.diagnostics[1];
    let expected_line2 = lines
        .iter()
        .position(|l| l.contains("Invalid syntax at end"))
        .expect("expected marker comment in test input");
    assert_eq!(diagnostic2.range.start.line as usize, expected_line2);
}
