use lkr_lsp::analyzer::LkrAnalyzer;
use tower_lsp::lsp_types::DiagnosticSeverity;

#[test]
fn reports_numeric_operand_diagnostic() {
    let mut analyzer = LkrAnalyzer::new();
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
