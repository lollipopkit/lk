use lkr_lsp::analyzer::LkrAnalyzer;

#[test]
fn test_stdlib_modules_listed() {
    let analyzer = &mut LkrAnalyzer::new();
    let modules = analyzer.list_stdlib_modules();
    // Ensure key stdlib modules are registered
    for m in ["math", "string", "datetime", "os", "tcp", "io"] {
        assert!(modules.contains(&m.to_string()), "missing module: {}", m);
    }
}

#[test]
fn test_module_exports_math() {
    let analyzer = &mut LkrAnalyzer::new();
    let exports = analyzer.list_module_exports("math").expect("math module exports");
    for f in ["abs", "sqrt", "sin", "cos", "tan", "pi", "e"] {
        assert!(exports.contains(&f.to_string()), "missing export: {}", f);
    }
}

#[test]
fn test_module_exports_iter() {
    let analyzer = &mut LkrAnalyzer::new();
    let exports = analyzer.list_module_exports("iter").expect("iter module exports");
    for f in [
        "enumerate",
        "range",
        "zip",
        "take",
        "skip",
        "chain",
        "flatten",
        "unique",
        "chunk",
        // generic higher-order ops now exported by iter
        "map",
        "filter",
        "reduce",
    ] {
        assert!(exports.contains(&f.to_string()), "missing export: {}", f);
    }
}
