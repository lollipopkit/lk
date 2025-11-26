use super::*;
use lkr_core::expr;
use std::collections::HashMap;
use tower_lsp::lsp_types::{DiagnosticSeverity, InlayHintKind, NumberOrString, Position, Range, SymbolKind};
use val::Val;

fn create_analyzer() -> LkrAnalyzer {
    LkrAnalyzer::new()
}

#[test]
fn test_analyze_simple_expression() {
    let mut analyzer = create_analyzer();
    let result = analyzer.analyze("req.user.role == 'admin'");

    // Should have identifier roots
    assert!(result.identifier_roots.contains("req"));

    // Should have expression symbol
    assert_eq!(result.symbols.len(), 1);
    assert_eq!(result.symbols[0].name, "expression");
    assert_eq!(result.symbols[0].kind, SymbolKind::CONSTANT);

    // Check that diagnostics include identifier roots info (this is now expected behavior)
    let has_context_info = result
        .diagnostics
        .iter()
        .any(|d| d.severity == Some(DiagnosticSeverity::INFORMATION) && d.message.contains("identifier roots"));
    assert!(has_context_info, "Expected identifier roots diagnostic");
}

#[test]
fn test_analyze_invalid_expression() {
    let mut analyzer = create_analyzer();
    let result = analyzer.analyze("req.user.role == 'unterminated string");

    // Should have diagnostic for invalid expression (tokenization error due to unterminated string)
    assert!(!result.diagnostics.is_empty());
    assert_eq!(result.diagnostics[0].severity, Some(DiagnosticSeverity::ERROR));
    assert!(result.diagnostics[0].message.contains("Tokenization error"));
}

#[test]
fn test_analyze_statement_program() {
    let mut analyzer = create_analyzer();
    let code = r#"
        import math;
        let user_level = req.user.level;
        fn calculate_score(base) {
            return math.sqrt(base * user_level);
        }
        let result = calculate_score(100);
    "#;
    let result = analyzer.analyze(code);

    // Type checking should surface strict Any diagnostics for missing annotations
    assert_eq!(result.diagnostics.len(), 1);
    let diag = &result.diagnostics[0];
    assert_eq!(diag.severity, Some(DiagnosticSeverity::ERROR));
    assert!(diag.message.contains("Function 'calculate_score' infers implicit Any"));
    assert_eq!(diag.code, Some(NumberOrString::String("lkr_type_error".to_string())));

    // Should have symbols for import, variable, and function
    assert!(result.symbols.len() >= 3);

    let symbol_names: Vec<&String> = result.symbols.iter().map(|s| &s.name).collect();
    assert!(symbol_names.contains(&&"import math".to_string()));
    assert!(symbol_names.contains(&&"user_level".to_string()));
    assert!(symbol_names.contains(&&"calculate_score".to_string()));
    assert!(symbol_names.contains(&&"result".to_string()));
}

#[test]
fn test_collect_named_param_decls_for_signature_help() {
    let mut analyzer = create_analyzer();
    let content = r#"
        fn draw_rect(x: Int, {width: Int, height: Int? = 100}) {
            return width * height;
        }
    "#;
    let decls = analyzer.collect_fn_named_param_decls(content);
    let params = decls.get("draw_rect").expect("expected named params");
    assert_eq!(params.len(), 2);
    assert_eq!(params[0].name, "width");
    assert!(matches!(params[0].type_annotation, Some(val::Type::Int)));
    assert!(params[0].default.is_none());
    assert_eq!(params[1].name, "height");
    assert!(matches!(
        params[1].type_annotation.as_ref(),
        Some(val::Type::Optional(inner)) if matches!(**inner, val::Type::Int)
    ));
    assert!(matches!(
        params[1].default.as_ref(),
        Some(expr::Expr::Val(Val::Int(100)))
    ));
}

#[test]
fn test_collect_named_call_diagnostics_for_missing_required() {
    let mut analyzer = create_analyzer();
    let content = r#"
        fn foo({x: Int, y: Int}) { return x + y; }
        fn main() { foo(x: 1); }
    "#;
    let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(content).unwrap();
    let diagnostics = analyzer.collect_named_call_diagnostics(content, &tokens, &spans);
    assert!(diagnostics
        .iter()
        .any(|diag| diag.message.contains("Missing required named argument: y")));
}

#[test]
fn test_collect_named_param_decls_supports_completions() {
    let mut analyzer = create_analyzer();
    let content = r#"
        fn configure({host: Str, port: Int, secure: Bool? = false}) { }
    "#;
    let decls = analyzer.collect_fn_named_param_decls(content);
    let params = decls.get("configure").expect("expected named params");
    let all_names: std::collections::HashSet<&str> = params.iter().map(|p| p.name.as_str()).collect();
    assert!(all_names.contains("host"));
    assert!(all_names.contains("port"));
    assert!(all_names.contains("secure"));
}

#[test]
fn test_get_var_completions() {
    let mut analyzer = create_analyzer();
    let completions = analyzer.get_var_completions("req");

    // Should return completions that start with "req"
    assert!(!completions.is_empty());

    let labels: Vec<&String> = completions.iter().map(|c| &c.label).collect();
    assert!(labels.contains(&&"req".to_string()));
    assert!(labels.contains(&&"req.user".to_string()));
    assert!(labels.contains(&&"req.user.id".to_string()));
    assert!(labels.contains(&&"req.user.role".to_string()));
    assert!(labels.contains(&&"req.user.name".to_string()));

    // Should not include completions that don't match the prefix
    assert!(!labels.contains(&&"record".to_string()));
}

#[test]
fn test_validate_identifier_access_with_valid_vars() {
    let analyzer = create_analyzer();

    // Create a variables map with req.user.role
    let mut user_map = HashMap::new();
    user_map.insert("role".to_string(), Val::Str("admin".to_string().into()));
    user_map.insert("id".to_string(), Val::Int(123));

    let mut req_map = HashMap::new();
    req_map.insert("user".to_string(), Val::from(user_map));

    let mut context_map = HashMap::new();
    context_map.insert("req".to_string(), Val::from(req_map));
    let context = Val::from(context_map);

    // Parse expression that uses req.user.role
    let tokens = token::Tokenizer::tokenize("req.user.role == 'admin'").unwrap();
    let mut parser = ast::Parser::new(&tokens);
    let expr = parser
        .parse()
        .expect("parser should succeed for req.user.role expression");

    let diagnostics = analyzer.validate_identifier_access(&expr, Some(&context));

    // Should have no diagnostics since variables map is valid
    assert!(diagnostics.is_empty());
}

#[test]
fn test_identifier_map_has_key() {
    let analyzer = create_analyzer();

    // Create nested variables map structure
    let mut inner_map = HashMap::new();
    inner_map.insert("name".to_string(), Val::Str("test".to_string().into()));

    let mut middle_map = HashMap::new();
    middle_map.insert("user".to_string(), Val::from(inner_map));

    let mut context_map = HashMap::new();
    context_map.insert("req".to_string(), Val::from(middle_map));
    let context = Val::from(context_map);

    // Test existing nested key
    assert!(analyzer.vars_has_key(&context, "req.user.name"));

    // Test non-existing key
    assert!(!analyzer.vars_has_key(&context, "req.user.role"));
    assert!(!analyzer.vars_has_key(&context, "req.admin"));
    assert!(!analyzer.vars_has_key(&context, "nonexistent"));
}

#[test]
fn test_generate_semantic_tokens_simple_expression() {
    let analyzer = create_analyzer();
    let content = "req.user.role == 'admin'";
    let tokens = analyzer.generate_semantic_tokens(content);

    // Define the legend indices for testing
    const OPERATOR_IDX: u32 = 6;
    const STRING_IDX: u32 = 4;

    assert!(!tokens.is_empty());

    let mut found_operator = false;
    let mut found_string = false;

    for token in &tokens {
        if token.token_type == OPERATOR_IDX {
            found_operator = true;
        } else if token.token_type == STRING_IDX {
            found_string = true;
        }
    }

    assert!(found_operator, "Should find operator token");
    assert!(found_string, "Should find string token");
}

#[test]
fn test_generate_semantic_tokens_statement_program() {
    let analyzer = create_analyzer();
    let content = r#"
        let user_level = req.user.level;
        if user_level > 5 {
            return "admin";
        }
    "#;
    let tokens = analyzer.generate_semantic_tokens(content);

    const KEYWORD_IDX: u32 = 1;

    assert!(!tokens.is_empty());

    let mut found_keyword = false;
    for token in &tokens {
        if token.token_type == KEYWORD_IDX {
            found_keyword = true;
            break;
        }
    }

    assert!(found_keyword, "Should find keyword tokens");
}

#[test]
fn test_generate_semantic_tokens_with_comments() {
    let analyzer = create_analyzer();
    let content = r#"
        // This is a comment
        let x = 42;
    "#;
    let tokens = analyzer.generate_semantic_tokens(content);

    const COMMENT_IDX: u32 = 0;

    assert!(tokens.iter().any(|t| t.token_type == COMMENT_IDX));
}

#[test]
fn test_generate_semantic_tokens_with_numbers() {
    let analyzer = create_analyzer();
    let content = "let x = 42 + 3.14;";
    let tokens = analyzer.generate_semantic_tokens(content);

    const NUMBER_IDX: u32 = 5;

    assert!(tokens.iter().any(|t| t.token_type == NUMBER_IDX));
}

#[test]
fn test_generate_semantic_tokens_function_identifier() {
    let analyzer = create_analyzer();
    let content = "let y = foo(1) + bar (2);";
    let tokens = analyzer.generate_semantic_tokens(content);

    const FUNCTION_IDX: u32 = 3;

    assert!(tokens.iter().any(|t| t.token_type == FUNCTION_IDX));
}

#[test]
fn test_type_inlay_hints_let_and_define() {
    let analyzer = LkrAnalyzer::new();
    let src = r#"
        let x = 1;
        y := 1.0;
    "#;
    let mut hints = analyzer.compute_type_inlay_hints(src, full_range(src));
    hints.extend(analyzer.compute_define_type_hints(src, full_range(src)));
    assert!(!hints.is_empty(), "expected type hints for let/define, got none");
    assert!(hints.iter().all(|h| h.kind == Some(InlayHintKind::TYPE)));
}

fn full_range(s: &str) -> Range {
    let lines = s.lines().count();
    let end_line = (lines.saturating_sub(1)) as u32;
    let end_col = s.lines().last().map(|l| l.len() as u32).unwrap_or(0);
    Range::new(Position::new(0, 0), Position::new(end_line, end_col))
}
