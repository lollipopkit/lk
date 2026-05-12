use lkr_lsp::analyzer::LkrAnalyzer;
use std::{fs, path::PathBuf};
use tower_lsp::lsp_types::DiagnosticSeverity;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("lsp crate has workspace parent")
        .to_path_buf()
}

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

    let _ = fs::remove_dir_all(base);
}

#[test]
fn test_package_import_is_not_reported_as_unknown_module() {
    let mut base = std::env::temp_dir();
    base.push(format!("lkr-lsp-package-test-{}", std::process::id()));
    let app_src = base.join("src");
    let dep_src = base.join("deps").join("util").join("src");
    fs::create_dir_all(&app_src).unwrap();
    fs::create_dir_all(&dep_src).unwrap();
    fs::write(
        base.join("Lkr.toml"),
        r#"
[package]
name = "app"

[dependencies]
util = { path = "deps/util" }
"#,
    )
    .unwrap();
    fs::write(
        base.join("deps").join("util").join("Lkr.toml"),
        r#"
[package]
name = "util"
"#,
    )
    .unwrap();
    fs::write(dep_src.join("mod.lkr"), "fn answer() { return 42; }\n").unwrap();

    let mut analyzer = LkrAnalyzer::new();
    analyzer.set_base_dir(app_src);
    let res = analyzer.analyze("import util;\nreturn util.answer();\n");
    let msgs: Vec<&str> = res.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert!(
        !msgs.iter().any(|m| m.contains("Unknown module: util")),
        "expected package import to resolve; diagnostics: {msgs:?}"
    );

    let _ = fs::remove_dir_all(base);
}

#[test]
fn test_example_workspace_imports_are_resolved() {
    let root = repo_root().join("examples/lkr-example-workspace");
    let app_src = root.join("apps/demo/src");
    let main_path = app_src.join("main.lkr");
    let code = fs::read_to_string(&main_path).expect("read example workspace main.lkr");

    let mut analyzer = LkrAnalyzer::new();
    analyzer.set_base_dir(app_src);
    let res = analyzer.analyze(&code);

    let msgs: Vec<&str> = res.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert!(
        !msgs.iter().any(|m| m.contains("Unknown module: mathlib")),
        "expected mathlib workspace import to resolve; diagnostics: {msgs:?}"
    );
    assert!(
        !msgs.iter().any(|m| m.contains("Unknown module: greetings")),
        "expected greetings workspace import to resolve; diagnostics: {msgs:?}"
    );
}
