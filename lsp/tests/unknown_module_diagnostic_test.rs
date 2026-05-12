use lkr_lsp::analyzer::LkrAnalyzer;
use std::{fs, path::PathBuf};
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

#[test]
fn test_file_import_diagnostic_resolves_relative_to_current_file_dir() {
    let mut base = std::env::temp_dir();
    base.push(format!("lkr-lsp-import-test-{}", std::process::id()));
    let current_file_dir = base.join("examples");
    let nested_import_dir = current_file_dir.join("examples");
    fs::create_dir_all(&nested_import_dir).unwrap();

    let fib_path = nested_import_dir.join("fib.lkr");
    fs::write(&fib_path, "export fn iterative(n) { return n; }\n").unwrap();

    let mut analyzer = LkrAnalyzer::new();
    analyzer.set_base_dir(current_file_dir);
    let res = analyzer.analyze(r#"import "examples/fib";"#);

    let msgs: Vec<&str> = res.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert!(
        !msgs.iter().any(|m| m.contains("File not found: examples/fib")),
        "expected import to resolve via current file directory; diagnostics: {msgs:?}"
    );

    let _ = fs::remove_dir_all(PathBuf::from(base));
}
