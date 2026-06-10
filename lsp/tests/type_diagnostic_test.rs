use lk_lsp::analyzer::LkAnalyzer;
use tower_lsp::lsp_types::DiagnosticSeverity;

#[test]
fn reports_numeric_operand_diagnostic() {
    let mut analyzer = LkAnalyzer::new();
    let code = r#"
        let result = "foo" - 1;
    "#;
    let analysis = analyzer.analyze(code);

    assert!(analysis
        .diagnostics
        .iter()
        .any(|d| d.severity == Some(DiagnosticSeverity::ERROR)));
    let messages: Vec<&str> = analysis.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert!(
        messages.iter().any(|m| m.contains("must by numeric types")),
        "expected numeric diagnostic in {:?}",
        messages
    );
}

#[test]
fn reports_macro_rule_mismatch_notes() {
    let mut analyzer = LkAnalyzer::new();
    let code = r#"
        macro_rules! pair {
            ($left:expr, $right:expr) => { $left + $right };
            (1 => $value:expr) => { $value };
        }
        return pair!(1);
    "#;
    let analysis = analyzer.analyze(code);
    let messages: Vec<&str> = analysis.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert!(
        messages
            .iter()
            .any(|m| { m.contains("No matching rule for macro `pair`") && m.contains("Macro rule mismatch notes:") }),
        "expected macro mismatch diagnostic in {:?}",
        messages
    );
}
