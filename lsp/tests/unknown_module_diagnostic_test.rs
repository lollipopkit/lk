use lkr_lsp::analyzer::LkrAnalyzer;
use tower_lsp::lsp_types::DiagnosticSeverity;

#[test]
fn test_unknown_module_import_diagnostic() {
    let mut analyzer = LkrAnalyzer::new();
    let code = r#"
        import not_a_module;
        import * as ns from missing;
        import { foo, bar } from bogus;
    "#;
    let res = analyzer.analyze(code);
    // Should have at least one error diagnostic about unknown modules
    assert!(res
        .diagnostics
        .iter()
        .any(|d| d.severity == Some(DiagnosticSeverity::ERROR)));
    let msgs: Vec<&str> = res.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert!(msgs.iter().any(|m| m.contains("Unknown module: not_a_module")));
    assert!(msgs.iter().any(|m| m.contains("Unknown module: bogus")));
}
