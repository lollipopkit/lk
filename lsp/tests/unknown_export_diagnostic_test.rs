use lkr_lsp::analyzer::LkrAnalyzer;
use tower_lsp::lsp_types::DiagnosticSeverity;

#[test]
fn test_unknown_export_import_diagnostic() {
    let mut analyzer = LkrAnalyzer::new();
    let code = r#"
        import { sqrt, not_exist } from math;
        import { bogus } from string;
    "#;
    let res = analyzer.analyze(code);
    // Should have ERROR diagnostics for the unknown exports
    let errors: Vec<_> = res
        .diagnostics
        .iter()
        .filter(|d| d.severity == Some(DiagnosticSeverity::ERROR))
        .collect();
    assert!(!errors.is_empty());
    let msgs: Vec<&str> = res.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert!(msgs
        .iter()
        .any(|m| m.contains("Unknown export 'not_exist' from module 'math'")));
    assert!(msgs
        .iter()
        .any(|m| m.contains("Unknown export 'bogus' from module 'string'")));
}
