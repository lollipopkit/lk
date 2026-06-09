use super::*;
use lk_core::expr;
use lk_core::util::fast_map::FastHashMap;
use lk_core::val::{HeapStore, HeapValue, LiteralVal, RuntimeVal, ShortStr, TypedMap};
use tower_lsp::lsp_types::{
    DiagnosticSeverity, InlayHintKind, NumberOrString, Position, Range, SemanticToken, SymbolKind,
};
fn create_analyzer() -> LkAnalyzer {
    LkAnalyzer::new()
}

fn short(value: &str) -> RuntimeVal {
    RuntimeVal::ShortStr(ShortStr::new(value).expect("short test string"))
}

fn string_map(heap: &mut HeapStore, entries: impl IntoIterator<Item = (&'static str, RuntimeVal)>) -> RuntimeVal {
    let entries = entries
        .into_iter()
        .map(|(key, value)| (std::sync::Arc::<str>::from(key), value))
        .collect::<FastHashMap<_, _>>();
    RuntimeVal::Obj(heap.alloc(HeapValue::Map(TypedMap::StringMixed(entries))))
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
        use math;
        let user_level = req.user.level;
        fn calculate_score(base) {
            return math.sqrt(base * user_level);
        }
        let result = calculate_score(100);
    "#;
    let result = analyzer.analyze(code);

    // The later call constrains `base` to Int, so strict Any should not report a false positive.
    assert!(
        result.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        result.diagnostics
    );

    // Should have grouped use/variable symbols and function symbols.
    assert!(result.symbols.len() >= 3);

    let symbol_names: Vec<&String> = result.symbols.iter().map(|s| &s.name).collect();
    assert!(symbol_names.contains(&&"calculate_score".to_string()));
    let imports = result
        .symbols
        .iter()
        .find(|s| s.name == "Imports")
        .expect("Imports group");
    let import_names: Vec<&String> = imports
        .children
        .as_ref()
        .expect("use children")
        .iter()
        .map(|s| &s.name)
        .collect();
    assert!(import_names.contains(&&"use math".to_string()));
    let variables = result
        .symbols
        .iter()
        .find(|s| s.name == "Variables")
        .expect("Variables group");
    let variable_names: Vec<&String> = variables
        .children
        .as_ref()
        .expect("variable children")
        .iter()
        .map(|s| &s.name)
        .collect();
    assert!(variable_names.contains(&&"user_level".to_string()));
    assert!(variable_names.contains(&&"result".to_string()));
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
        Some(expr::Expr::Literal(LiteralVal::Int(100)))
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
fn test_validate_identifier_access_with_valid_vars() {
    let analyzer = create_analyzer();

    // Create a variables map with req.user.role
    let mut heap = HeapStore::new();
    let user = string_map(&mut heap, [("role", short("admin")), ("id", RuntimeVal::Int(123))]);
    let req = string_map(&mut heap, [("user", user)]);
    let context = string_map(&mut heap, [("req", req)]);

    // Parse expression that uses req.user.role
    let tokens = token::Tokenizer::tokenize("req.user.role == 'admin'").unwrap();
    let mut parser = ast::Parser::new(&tokens);
    let expr = parser
        .parse()
        .expect("parser should succeed for req.user.role expression");

    let diagnostics = analyzer.validate_identifier_access(&expr, Some((&context, &heap)));

    // Should have no diagnostics since variables map is valid
    assert!(diagnostics.is_empty());
}

#[test]
fn test_identifier_map_has_key() {
    let analyzer = create_analyzer();

    // Create nested variables map structure
    let mut heap = HeapStore::new();
    let inner = string_map(&mut heap, [("name", short("test"))]);
    let middle = string_map(&mut heap, [("user", inner)]);
    let context = string_map(&mut heap, [("req", middle)]);

    // Test existing nested key
    assert!(analyzer.vars_has_key(&context, &heap, "req.user.name"));

    // Test non-existing key
    assert!(!analyzer.vars_has_key(&context, &heap, "req.user.role"));
    assert!(!analyzer.vars_has_key(&context, &heap, "req.admin"));
    assert!(!analyzer.vars_has_key(&context, &heap, "nonexistent"));
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
fn test_generate_semantic_tokens_match_or_pattern() {
    let analyzer = create_analyzer();
    let content = r#"let day_type = match 6 {
  1 | 2 | 3 | 4 | 5 => "weekday",
  6 | 7 => "weekend",
  _ => "invalid",
};"#;
    let tokens = analyzer.generate_semantic_tokens(content);

    const KEYWORD_IDX: u32 = 1;
    const VARIABLE_IDX: u32 = 2;
    const STRING_IDX: u32 = 4;
    const OPERATOR_IDX: u32 = 6;

    let token_texts = semantic_token_texts(content, &tokens);

    assert!(token_texts
        .iter()
        .any(|(text, ty)| text == "match" && *ty == KEYWORD_IDX));
    assert!(token_texts.iter().any(|(text, ty)| text == "|" && *ty == OPERATOR_IDX));
    assert!(token_texts.iter().any(|(text, ty)| text == "=>" && *ty == OPERATOR_IDX));
    assert!(token_texts
        .iter()
        .any(|(text, ty)| text == "\"weekend\"" && *ty == STRING_IDX));
    assert!(!token_texts.iter().any(|(text, ty)| text == "_" && *ty == VARIABLE_IDX));
}

fn semantic_token_texts(content: &str, tokens: &[SemanticToken]) -> Vec<(String, u32)> {
    let lines: Vec<&str> = content.lines().collect();
    let mut out = Vec::new();
    let mut line = 0u32;
    let mut start = 0u32;

    for token in tokens {
        line += token.delta_line;
        if token.delta_line == 0 {
            start += token.delta_start;
        } else {
            start = token.delta_start;
        }

        let text: String = lines
            .get(line as usize)
            .map(|line_text| {
                line_text
                    .chars()
                    .skip(start as usize)
                    .take(token.length as usize)
                    .collect()
            })
            .unwrap_or_default();
        out.push((text, token.token_type));
    }

    out
}

#[test]
fn test_validate_semantic_tokens_accepts_generated_tokens() {
    let analyzer = create_analyzer();
    let content = "let y = foo(1)\nreturn y\n";
    let tokens = analyzer.generate_semantic_tokens(content);

    let summary = analyzer.validate_semantic_tokens(content, &tokens);

    assert!(summary.valid, "unexpected semantic token errors: {:?}", summary.errors);
    assert_eq!(summary.token_count, tokens.len());
}

#[test]
fn test_validate_semantic_tokens_rejects_bad_ranges_and_legend_indexes() {
    let analyzer = create_analyzer();
    let content = "let x = 1";
    let tokens = vec![SemanticToken {
        delta_line: 0,
        delta_start: 20,
        length: 1,
        token_type: 99,
        token_modifiers_bitset: 1 << 8,
    }];

    let summary = analyzer.validate_semantic_tokens(content, &tokens);

    assert!(!summary.valid);
    assert!(summary.errors.iter().any(|err| err.contains("token_type")));
    assert!(summary.errors.iter().any(|err| err.contains("modifier bitset")));
    assert!(summary.errors.iter().any(|err| err.contains("exceeds line")));
}

#[test]
fn test_type_inlay_hints_let_and_define() {
    let analyzer = LkAnalyzer::new();
    let src = r#"
        let x = 1;
        y := 1.0;
    "#;
    let mut hints = analyzer.compute_type_inlay_hints(src, full_range(src));
    hints.extend(analyzer.compute_define_type_hints(src, full_range(src)));
    assert!(!hints.is_empty(), "expected type hints for let/define, got none");
    assert!(hints.iter().all(|h| h.kind == Some(InlayHintKind::TYPE)));
}

#[test]
fn test_business_workload_should_run_infers_from_string_calls() {
    let mut analyzer = create_analyzer();
    let code = include_str!("../../../bench/workloads_business_algorithms.lk");
    let result = analyzer.analyze(code);

    assert!(
        !result
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("Function 'should_run' infers implicit Any")),
        "should_run(name) should infer name from later string call sites"
    );
}

#[test]
fn test_unconstrained_implicit_any_diagnostic_points_to_parameter() {
    let mut analyzer = create_analyzer();
    let code = r#"
        let workload_filter = os.env.get("LK_WORKLOAD_FILTER", "");
        fn should_run(name) {
            return workload_filter == "" || workload_filter == name;
        }
    "#;
    let result = analyzer.analyze(code);

    assert_eq!(result.diagnostics.len(), 1);
    let diag = &result.diagnostics[0];
    assert_eq!(diag.severity, Some(DiagnosticSeverity::ERROR));
    assert_eq!(diag.code, Some(NumberOrString::String("lk_type_error".to_string())));
    assert_eq!(
        diag.message,
        "Function 'should_run' infers implicit Any for parameter 'name'; add explicit annotations"
    );
    assert_eq!(
        diag.range,
        Range::new(Position::new(2, 22), Position::new(2, 26)),
        "implicit Any diagnostic should point at the unannotated parameter"
    );
}

fn full_range(s: &str) -> Range {
    let lines = s.lines().count();
    let end_line = (lines.saturating_sub(1)) as u32;
    let end_col = s.lines().last().map(|l| l.len() as u32).unwrap_or(0);
    Range::new(Position::new(0, 0), Position::new(end_line, end_col))
}
