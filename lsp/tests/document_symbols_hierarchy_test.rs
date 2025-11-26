use lkr_lsp::analyzer::LkrAnalyzer;
use tower_lsp::lsp_types::{DocumentSymbol, SymbolKind};

fn get_symbol<'a>(symbols: &'a [DocumentSymbol], name: &str) -> Option<&'a DocumentSymbol> {
    symbols.iter().find(|s| s.name == name)
}

fn get_child<'a>(parent: &'a DocumentSymbol, name: &str) -> Option<&'a DocumentSymbol> {
    parent
        .children
        .as_ref()
        .and_then(|kids| kids.iter().find(|s| s.name == name))
}

fn list_child_names(parent: &DocumentSymbol) -> Vec<String> {
    parent
        .children
        .as_ref()
        .map(|kids| kids.iter().map(|s| s.name.clone()).collect())
        .unwrap_or_default()
}

#[test]
fn test_function_symbol_hierarchy_with_groups_and_labels() {
    let mut analyzer = LkrAnalyzer::new();
    let code = r#"
        import math;

        let a: Int = 1;

        fn outer(p1: Int, p2: Int) -> Int {
            let x: Int = 1;
            return x;
        }
    "#;

    let res = analyzer.analyze(code);
    assert!(!res.symbols.is_empty());

    // Top-level Imports container with child import symbol(s)
    let imports = get_symbol(&res.symbols, "Imports").expect("Imports container present");
    assert_eq!(imports.kind, SymbolKind::NAMESPACE);
    let import_kids = list_child_names(imports);
    assert!(
        import_kids.iter().any(|n| n == "import math"),
        "imports children: {:?}",
        import_kids
    );

    // Top-level Variables container and individual variable
    let vars = get_symbol(&res.symbols, "Variables").expect("Variables container present");
    assert_eq!(vars.kind, SymbolKind::NAMESPACE);
    let var_kids = list_child_names(vars);
    assert!(var_kids.iter().any(|n| n == "a"), "variables children: {:?}", var_kids);
    // Backward compatibility: individual variable still present at top level
    let top_var = get_symbol(&res.symbols, "a").expect("top-level variable symbol present");
    assert_eq!(top_var.kind, SymbolKind::VARIABLE);

    // Function hierarchy: outer -> { Parameters, Locals, inner }
    let top_names: Vec<&String> = res.symbols.iter().map(|s| &s.name).collect();
    println!("top-level symbols: {:?}", top_names);
    let outer = res
        .symbols
        .iter()
        .find(|s| s.name == "outer" && s.kind == SymbolKind::FUNCTION)
        .expect("outer function present at top level");
    let params = get_child(outer, "Parameters").expect("Parameters group under outer");
    assert_eq!(params.kind, SymbolKind::NAMESPACE);
    let param_names = list_child_names(params);
    assert!(param_names.iter().any(|n| n == "p1"), "params: {:?}", param_names);
    assert!(param_names.iter().any(|n| n == "p2"), "params: {:?}", param_names);

    let locals = get_child(outer, "Locals").expect("Locals group under outer");
    assert_eq!(locals.kind, SymbolKind::NAMESPACE);
    let local_names = list_child_names(locals);
    assert!(local_names.iter().any(|n| n == "x"), "locals: {:?}", local_names);

    // No nested functions in this case; just verify groups are present
    let outer_child_names = list_child_names(outer);
    println!("outer children: {:?}", outer_child_names);
    assert!(outer_child_names.iter().any(|n| n == "Parameters"));
    assert!(outer_child_names.iter().any(|n| n == "Locals"));
}

#[test]
fn test_nested_function_appears_under_parent() {
    let mut analyzer = LkrAnalyzer::new();
    let code = r#"
        fn outer(a: Int) -> Int {
            let x: Int = 1;
            fn inner(q: Int) -> Int {
                let y: Int = q;
                return y;
            }
            return x;
        }
    "#;

    let res = analyzer.analyze(code);

    let outer = res
        .symbols
        .iter()
        .find(|s| s.name == "outer" && s.kind == SymbolKind::FUNCTION)
        .expect("outer function present");
    let child_names = list_child_names(outer);
    assert!(
        child_names.iter().any(|n| n == "inner"),
        "nested child not found, children: {:?}",
        child_names
    );
    let inner = get_child(outer, "inner").expect("inner function symbol under outer");
    // inner should have Parameters and Locals groups
    let inner_child_names = list_child_names(inner);
    assert!(
        inner_child_names.iter().any(|n| n == "Parameters"),
        "inner children: {:?}",
        inner_child_names
    );
    assert!(
        inner_child_names.iter().any(|n| n == "Locals"),
        "inner children: {:?}",
        inner_child_names
    );
    // Verify parameter q and local y exist within respective groups
    let inner_params = get_child(inner, "Parameters").expect("Parameters under inner");
    let inner_param_names = list_child_names(inner_params);
    assert!(
        inner_param_names.iter().any(|n| n == "q"),
        "inner params: {:?}",
        inner_param_names
    );
    let inner_locals = get_child(inner, "Locals").expect("Locals under inner");
    let inner_local_names = list_child_names(inner_locals);
    assert!(
        inner_local_names.iter().any(|n| n == "y"),
        "inner locals: {:?}",
        inner_local_names
    );
}

#[test]
fn test_toplevel_grouped_containers() {
    let mut analyzer = LkrAnalyzer::new();
    let code = r#"
        import math;
        import { sqrt, sin } from math;

        let x = 1;
        let y = 2;
    "#;

    let res = analyzer.analyze(code);
    assert!(
        res.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        res.diagnostics
    );

    // Imports container contains each import as a child; exact label per analyzer
    let imports = get_symbol(&res.symbols, "Imports").expect("Imports container present");
    let import_names = list_child_names(imports);
    assert!(
        import_names.iter().any(|n| n == "import math"),
        "imports: {:?}",
        import_names
    );
    assert!(
        import_names.iter().any(|n| n == "import {…} from math"),
        "imports: {:?}",
        import_names
    );

    // Variables container contains both x and y
    let vars = get_symbol(&res.symbols, "Variables").expect("Variables container present");
    let var_names = list_child_names(vars);
    assert!(var_names.iter().any(|n| n == "x"), "vars: {:?}", var_names);
    assert!(var_names.iter().any(|n| n == "y"), "vars: {:?}", var_names);
    // Individual variables still present at top-level
    assert!(get_symbol(&res.symbols, "x").is_some());
    assert!(get_symbol(&res.symbols, "y").is_some());

    // No top-level labels in this program
}
