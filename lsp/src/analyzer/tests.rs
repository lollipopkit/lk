use super::*;
use lk_core::expr;
use lk_core::macro_system::{ProcMacroProcessConfig, ProcMacroProviders};
use lk_core::val::{HeapStore, HeapValue, LiteralVal, RuntimeVal, ShortStr, TypedMap};
use std::{fs, path::PathBuf, time::Duration};
use tower_lsp::lsp_types::{
    DiagnosticSeverity, InlayHintKind, NumberOrString, Position, Range, SemanticToken, SymbolKind,
};
fn create_analyzer() -> LkAnalyzer {
    LkAnalyzer::new()
}

fn test_shell() -> Option<PathBuf> {
    let shell = PathBuf::from("/bin/sh");
    shell.exists().then_some(shell)
}

fn unique_tmp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "lk_lsp_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos()
    ))
}

fn short(value: &str) -> RuntimeVal {
    RuntimeVal::ShortStr(ShortStr::new(value).expect("short test string"))
}

fn string_map(heap: &mut HeapStore, entries: impl IntoIterator<Item = (&'static str, RuntimeVal)>) -> RuntimeVal {
    let entries = entries
        .into_iter()
        .map(|(key, value)| (std::sync::Arc::<str>::from(key), value))
        .collect::<lk_core::util::value_map::ValueMap<_, _>>();
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
fn test_collect_named_param_decls_through_attributes() {
    let mut analyzer = create_analyzer();
    let content = r#"
        #[inline]
        fn draw_rect({width: Int}) {
            return width;
        }
    "#;
    let decls = analyzer.collect_fn_named_param_decls(content);
    let params = decls.get("draw_rect").expect("expected named params");
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].name, "width");
}

#[test]
fn analyzer_token_cache_invalidates_program_expansion_when_proc_macro_dependency_changes() {
    let Some(shell) = test_shell() else {
        return;
    };
    let dir = unique_tmp_dir("analyzer_proc_macro_dep_cache");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    let schema = dir.join("schema.txt");
    let script = dir.join("provider.sh");
    fs::write(&schema, "1").expect("write schema");
    fs::write(
        &script,
        r#"cat >/dev/null
value=$(cat "$1")
printf '{"protocol_version":1,"output_tokens":[{"kind":"Int","lexeme":"%s","span":null}],"diagnostics":[],"dependencies":[{"path":"schema.txt","digest":null}]}' "$value"
"#,
    )
    .expect("write provider script");

    let mut providers = ProcMacroProviders::default();
    providers.register_function_like(
        "from_schema",
        ProcMacroProcessConfig {
            program: shell,
            args: vec![script.display().to_string(), schema.display().to_string()],
            timeout: Duration::from_secs(1),
            max_output_bytes: 4096,
            env: None,
        },
    );

    let mut analyzer = create_analyzer();
    analyzer.set_base_dir(dir.clone());
    analyzer.set_proc_macro_providers(providers);
    let content = "return from_schema!();";

    let first_entry = analyzer.tokenize_with_spans_cached(content).expect("first tokenize");
    let first = first_entry
        .parse_program_expansion_arc(content)
        .expect("first expansion");
    assert!(first
        .source
        .tokens
        .iter()
        .any(|token| matches!(token, token::Token::Int(1))));

    fs::write(&schema, "2").expect("rewrite schema");

    let second_entry = analyzer.tokenize_with_spans_cached(content).expect("second tokenize");
    let second = second_entry
        .parse_program_expansion_arc(content)
        .expect("second expansion");

    assert!(second
        .source
        .tokens
        .iter()
        .any(|token| matches!(token, token::Token::Int(2))));
    assert!(!second
        .source
        .tokens
        .iter()
        .any(|token| matches!(token, token::Token::Int(1))));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn analyzer_token_cache_invalidates_program_expansion_when_file_import_changes() {
    let dir = unique_tmp_dir("analyzer_file_import_dep_cache");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    let macros = dir.join("macros.lk");
    fs::write(
        &macros,
        r#"
        export macro_rules! answer {
            () => { return 1; };
        }
        "#,
    )
    .expect("write macro file");

    let mut analyzer = create_analyzer();
    analyzer.set_base_dir(dir.clone());
    let content = r#"
        use { answer } from "macros.lk";
        answer!();
    "#;

    let first_entry = analyzer.tokenize_with_spans_cached(content).expect("first tokenize");
    let first = first_entry
        .parse_program_expansion_arc(content)
        .expect("first expansion");
    assert!(first
        .source
        .tokens
        .iter()
        .any(|token| matches!(token, token::Token::Int(1))));

    fs::write(
        &macros,
        r#"
        export macro_rules! answer {
            () => { return 2; };
        }
        "#,
    )
    .expect("rewrite macro file");

    let second_entry = analyzer.tokenize_with_spans_cached(content).expect("second tokenize");
    let second = second_entry
        .parse_program_expansion_arc(content)
        .expect("second expansion");

    assert!(second
        .source
        .tokens
        .iter()
        .any(|token| matches!(token, token::Token::Int(2))));
    assert!(!second
        .source
        .tokens
        .iter()
        .any(|token| matches!(token, token::Token::Int(1))));
    let _ = fs::remove_dir_all(&dir);
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
    let mut analyzer = LkAnalyzer::new();
    let src = r#"
        let x = 1;
        y := 1.0;
    "#;
    let hints = analyzer.compute_type_inlay_hints(src, full_range(src));
    assert!(!hints.is_empty(), "expected type hints for let/define, got none");
    assert!(hints.iter().all(|h| h.kind == Some(InlayHintKind::TYPE)));
}

fn hint_labels(hints: &[InlayHint]) -> Vec<String> {
    hints
        .iter()
        .map(|hint| match &hint.label {
            tower_lsp::lsp_types::InlayHintLabel::String(label) => label.clone(),
            _ => String::new(),
        })
        .collect()
}

#[test]
fn test_type_hints_cover_bindings_that_name_other_things() {
    let mut analyzer = LkAnalyzer::new();
    let src = "fn greet(name: String) -> String {\n    return name;\n}\nlet who = greet(\"lk\");\n";

    let hints = analyzer.compute_type_inlay_hints(src, full_range(src));

    // A fresh per-expression checker has never heard of `greet`, so this hint
    // could not exist before the types came from one document-wide check.
    assert!(
        hint_labels(&hints).iter().any(|label| label == ": String"),
        "expected `who: String`, got {:?}",
        hint_labels(&hints)
    );
}

#[test]
fn test_type_hints_use_stdlib_signatures() {
    let mut analyzer = LkAnalyzer::new();
    let src = "use string;\nlet parts = string.split(\"a,b\", \",\");\n";

    let hints = analyzer.compute_type_inlay_hints(src, full_range(src));

    assert!(
        hint_labels(&hints).iter().any(|label| label == ": List<String>"),
        "expected the declared return type of string.split, got {:?}",
        hint_labels(&hints)
    );
}

#[test]
fn test_return_type_hints_come_from_the_checked_signature() {
    let mut analyzer = LkAnalyzer::new();
    let src = "fn greet(name: String) -> String {\n    return name;\n}\nfn call_it() {\n    return greet(\"lk\");\n}\n";

    let hints = analyzer.compute_function_return_type_hints(src, full_range(src));

    // `call_it` returns whatever `greet` returns. Deriving that from the token
    // stream in a fresh checker — what this used to do — could not know `greet`.
    assert!(
        hint_labels(&hints).iter().any(|label| label.trim() == "-> String"),
        "expected `-> String` for call_it, got {:?}",
        hint_labels(&hints)
    );
    // `greet` says its own return type, so it gets no hint.
    assert_eq!(
        hint_labels(&hints).len(),
        1,
        "an annotated function needs no hint: {:?}",
        hint_labels(&hints)
    );
}

#[test]
fn test_hints_never_show_solver_type_variables() {
    let mut analyzer = LkAnalyzer::new();
    // The shape from closure.lk: the parameter type is undetermined, the return
    // type is not. Rendering the whole thing gave `('T0) -> Int`.
    let src = "let double = |x| x * 2;\nlet plain = 7;\n";

    let hints = analyzer.compute_type_inlay_hints(src, full_range(src));
    let labels = hint_labels(&hints);

    assert!(
        !labels.iter().any(|label| label.contains('\'')),
        "a solver variable reached the editor: {labels:?}"
    );
    assert!(
        labels.iter().any(|label| label == ": Int"),
        "the known type should still be shown: {labels:?}"
    );
    assert!(
        labels.iter().any(|label| label.contains("(_)")),
        "an unknown parameter should read as `_`, not disappear: {labels:?}"
    );
}

#[test]
fn test_type_hints_use_imported_signatures() {
    let dir = unique_tmp_dir("imported_signature_hints");
    fs::create_dir_all(&dir).expect("create temp dir");
    fs::write(
        dir.join("lib.lk"),
        "fn greet(name: String) -> String {\n    return name;\n}\n",
    )
    .expect("write dependency");

    let mut analyzer = LkAnalyzer::new();
    analyzer.set_base_dir(dir.clone());
    let src = "use { greet } from \"lib\";\nlet who = greet(\"lk\");\n";

    let hints = analyzer.compute_type_inlay_hints(src, full_range(src));

    // `lk check` has always seeded these signatures; the editor used to know
    // strictly less about the file than the compiler did.
    assert!(
        hint_labels(&hints).iter().any(|label| label == ": String"),
        "expected the imported function's declared return type, got {:?}",
        hint_labels(&hints)
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The editor must not cry wolf about a type that lives in another file.
///
/// An unknown type name is a diagnostic now, and a type crosses a module
/// boundary by its bare name — `use * as L from "./lib"; fn f() -> Row` names
/// `Row`, not `L.Row`. Only *functions* used to be seeded from an imported
/// file, so the checker had never heard of `Row`; while unknown names were
/// silently accepted that cost nothing, and the moment they became an error it
/// would have put a red squiggle under correct code.
#[test]
fn test_imported_types_are_not_reported_as_unknown() {
    let dir = unique_tmp_dir("imported_type_diagnostics");
    fs::create_dir_all(&dir).expect("create temp dir");
    fs::write(
        dir.join("lib.lk"),
        "struct Row { id: Int }
type Id = Int;
trait Shown { fn show(self) -> Int; }
fn mk(v: Int) -> Row { return Row { id: v }; }
",
    )
    .expect("write dependency");

    let mut analyzer = LkAnalyzer::new();
    analyzer.set_base_dir(dir.clone());
    let src = "use * as L from \"lib\";\n\
               fn pass(v: Int) -> Row { return L.mk(v); }\n\
               fn ident(v: Id) -> Id { return v; }\n";

    let result = analyzer.analyze(src);
    let unknown: Vec<&String> = result
        .diagnostics
        .iter()
        .map(|diagnostic| &diagnostic.message)
        .filter(|message| message.contains("Unknown type"))
        .collect();
    assert!(
        unknown.is_empty(),
        "imported types should be known to the editor, got {unknown:?}"
    );

    // …and a name nothing declares is still reported, so this is not vacuous.
    let typo = analyzer.analyze("let x: Strng = \"a\";\n");
    assert!(
        typo.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("Unknown type 'Strng'")),
        "a typo should still be reported, got {:?}",
        typo.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_type_hints_use_namespace_import_members() {
    let dir = unique_tmp_dir("namespace_import_hints");
    fs::create_dir_all(&dir).expect("create temp dir");
    fs::write(
        dir.join("lib.lk"),
        "fn greet(name: String) -> String {\n    return name;\n}\n",
    )
    .expect("write dependency");

    let mut analyzer = LkAnalyzer::new();
    analyzer.set_base_dir(dir.clone());

    // Both spellings bind a namespace whose members are reached as `lib.greet`.
    for src in [
        "use \"lib\";\nlet who = lib.greet(\"lk\");\n",
        "use * as lib from \"lib\";\nlet who = lib.greet(\"lk\");\n",
    ] {
        let hints = analyzer.compute_type_inlay_hints(src, full_range(src));
        assert!(
            hint_labels(&hints).iter().any(|label| label == ": String"),
            "expected the namespace member's declared return type for {src:?}, got {:?}",
            hint_labels(&hints)
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_one_bad_statement_does_not_cost_the_rest_their_hints() {
    let mut analyzer = LkAnalyzer::new();
    let src = "let bad: Int = \"x\";\nlet good = 41 + 1;\n";

    let hints = analyzer.compute_type_inlay_hints(src, full_range(src));

    assert!(
        hint_labels(&hints).iter().any(|label| label == ": Int"),
        "the statement after a type error still has a type, got {:?}",
        hint_labels(&hints)
    );
}

#[test]
fn test_type_error_points_at_the_offending_statement() {
    let mut analyzer = create_analyzer();
    // Two statements whose expressions are token-identical. Locating the error
    // by hunting the token stream finds the first `1`; the error is in the
    // second statement.
    let result = analyzer.analyze("let a: Int = 1;\nlet b: Bool = 1;\n");

    let diagnostic = result
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("Type mismatch"))
        .unwrap_or_else(|| panic!("expected a type mismatch, got {:?}", result.diagnostics));
    assert_eq!(
        diagnostic.range.start.line, 1,
        "the error belongs to line 2, got {:?}",
        diagnostic.range
    );
}

#[test]
fn test_expression_document_is_type_checked_once() {
    let mut analyzer = create_analyzer();
    // A document that parses as a single expression never reaches the statement
    // path, so its type check lives in the expression branch of `analyze`.
    //
    // The sample used to be `1 ? 2 : 3`, which was an error only because the
    // ternary demanded a `Bool` condition where the `if` statement accepted any
    // truthy value. That divergence is gone — LK's rule is truthiness — so the
    // sample is now an expression that is ill-typed for a reason unrelated to
    // conditions.
    let result = analyzer.analyze("\"a\" - 1");

    assert_eq!(
        result.diagnostics.len(),
        1,
        "expected exactly one type diagnostic, got {:?}",
        result.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
    assert_eq!(result.diagnostics[0].severity, Some(DiagnosticSeverity::ERROR));
    assert!(
        result.diagnostics[0].message.contains("Type Error"),
        "unexpected message: {}",
        result.diagnostics[0].message
    );
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
        fn should_run(name) {
            return true;
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
        Range::new(Position::new(1, 22), Position::new(1, 26)),
        "implicit Any diagnostic should point at the unannotated parameter"
    );
}

#[test]
fn test_macro_generated_type_diagnostic_includes_origin_stack() {
    let mut analyzer = create_analyzer();
    let code = r#"
        macro_rules! bad_numeric {
            () => { "foo" - 1; };
        }
        bad_numeric!();
    "#;
    let result = analyzer.analyze(code);

    let diagnostic = result
        .diagnostics
        .iter()
        .find(|diag| diag.message.contains("must be numeric types"))
        .expect("expected numeric type diagnostic");
    assert!(diagnostic.message.contains("Macro origin stack:"));
    assert!(diagnostic.message.contains("bad_numeric"));
    assert_eq!(
        diagnostic.code,
        Some(NumberOrString::String("lk_type_error".to_string()))
    );
}

#[test]
fn test_ast_macro_origins_appear_in_document_symbols() {
    let mut analyzer = create_analyzer();
    let code = r#"
        #[derive(Debug)]
        struct User { id: Int }
    "#;
    let result = analyzer.analyze(code);

    let macros = result
        .symbols
        .iter()
        .find(|symbol| symbol.name == "Macro Expansions")
        .expect("macro expansions symbol container");
    assert_eq!(macros.kind, SymbolKind::NAMESPACE);
    let origins = macros.children.as_ref().expect("macro expansion origin children");
    let derive = origins
        .iter()
        .find(|symbol| symbol.name == "builtin_derive Debug")
        .expect("Debug derive origin symbol");
    assert_eq!(derive.detail.as_deref(), Some("generated 2 item(s)"));
    let generated = derive.children.as_ref().expect("generated item children");
    assert!(
        generated.iter().any(|symbol| symbol.name == "trait __LKShow"),
        "generated symbols: {generated:?}"
    );
    assert!(
        generated.iter().any(|symbol| symbol.name == "impl __LKShow for User"),
        "generated symbols: {generated:?}"
    );
    let generated_impl = generated
        .iter()
        .find(|symbol| symbol.name == "impl __LKShow for User")
        .expect("generated impl symbol");
    assert_eq!(
        generated_impl.detail.as_deref(),
        Some("generated by builtin_derive Debug")
    );
    assert!(
        generated_impl.range.end.character > generated_impl.range.start.character,
        "generated item symbol should expose a source-map range"
    );
    let generated_members = generated_impl
        .children
        .as_ref()
        .expect("generated impl should expose member origins");
    assert!(
        generated_members.iter().any(|symbol| symbol.name == "fn show"),
        "generated member symbols: {generated_members:?}"
    );
    assert!(
        generated_members.iter().any(|symbol| symbol.name == "expr self.id"),
        "generated member symbols: {generated_members:?}"
    );
}

#[test]
fn test_ast_macro_control_flow_origins_appear_as_typed_document_symbols() {
    let span = Span::new(token::Position::new(2, 9, 10), token::Position::new(2, 23, 24));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "control_flow".to_string(),
        kind: macro_system::AstMacroOriginKind::Attribute,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["fn generated".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "fn generated".to_string(),
            span: Some(span.clone()),
            generated_member_origins: vec![
                macro_system::AstGeneratedMemberOrigin {
                    label: "stmt if".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "stmt const".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "stmt expr".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "expr match".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "expr var".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "expr call_expr".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "pattern variable".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "for_pattern variable".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "match_arm".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "match guard".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "template_part literal".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "template_part expr".to_string(),
                    span: Some(span.clone()),
                },
            ],
        }],
    }];
    let mut symbols = Vec::new();
    LkAnalyzer::append_ast_macro_origin_symbols(&mut symbols, &origins);

    let macros = symbols
        .iter()
        .find(|symbol| symbol.name == "Macro Expansions")
        .expect("macro expansions symbol container");
    let origin = macros
        .children
        .as_ref()
        .and_then(|children| children.iter().find(|symbol| symbol.name == "attribute control_flow"))
        .expect("attribute origin symbol");
    let generated_fn = origin
        .children
        .as_ref()
        .and_then(|children| children.iter().find(|symbol| symbol.name == "fn generated"))
        .expect("generated function symbol");
    let members = generated_fn.children.as_ref().expect("generated member symbols");
    let stmt_if = members
        .iter()
        .find(|symbol| symbol.name == "stmt if")
        .expect("generated if statement origin symbol");
    let stmt_expr = members
        .iter()
        .find(|symbol| symbol.name == "stmt expr")
        .expect("generated expression statement origin symbol");
    let stmt_const = members
        .iter()
        .find(|symbol| symbol.name == "stmt const")
        .expect("generated const statement origin symbol");
    let expr_match = members
        .iter()
        .find(|symbol| symbol.name == "expr match")
        .expect("generated match expression origin symbol");
    let expr_var = members
        .iter()
        .find(|symbol| symbol.name == "expr var")
        .expect("generated variable expression origin symbol");
    let expr_call_expr = members
        .iter()
        .find(|symbol| symbol.name == "expr call_expr")
        .expect("generated expression-callee call origin symbol");
    let pattern_variable = members
        .iter()
        .find(|symbol| symbol.name == "pattern variable")
        .expect("generated pattern variable origin symbol");
    let for_pattern_variable = members
        .iter()
        .find(|symbol| symbol.name == "for_pattern variable")
        .expect("generated for-pattern variable origin symbol");
    let match_arm = members
        .iter()
        .find(|symbol| symbol.name == "match_arm")
        .expect("generated match arm origin symbol");
    let match_guard = members
        .iter()
        .find(|symbol| symbol.name == "match guard")
        .expect("generated match guard origin symbol");
    let template_part_literal = members
        .iter()
        .find(|symbol| symbol.name == "template_part literal")
        .expect("generated template literal part origin symbol");
    let template_part_expr = members
        .iter()
        .find(|symbol| symbol.name == "template_part expr")
        .expect("generated template expression part origin symbol");

    assert_eq!(stmt_if.kind, SymbolKind::EVENT);
    assert_eq!(stmt_const.kind, SymbolKind::EVENT);
    assert_eq!(stmt_expr.kind, SymbolKind::EVENT);
    assert_eq!(expr_match.kind, SymbolKind::OPERATOR);
    assert_eq!(expr_var.kind, SymbolKind::OPERATOR);
    assert_eq!(expr_call_expr.kind, SymbolKind::OPERATOR);
    assert_eq!(pattern_variable.kind, SymbolKind::EVENT);
    assert_eq!(for_pattern_variable.kind, SymbolKind::EVENT);
    assert_eq!(match_arm.kind, SymbolKind::EVENT);
    assert_eq!(match_guard.kind, SymbolKind::EVENT);
    assert_eq!(template_part_literal.kind, SymbolKind::EVENT);
    assert_eq!(template_part_expr.kind, SymbolKind::EVENT);
    assert!(
        stmt_if.range.end.character > stmt_if.range.start.character,
        "generated control-flow symbol should expose a source-map range"
    );
}

#[test]
fn test_ast_macro_import_and_attribute_origins_appear_as_typed_document_symbols() {
    let span = Span::new(token::Position::new(3, 5, 30), token::Position::new(3, 29, 54));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "generated_metadata".to_string(),
        kind: macro_system::AstMacroOriginKind::Attribute,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["statement".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "statement".to_string(),
            span: Some(span.clone()),
            generated_member_origins: vec![
                macro_system::AstGeneratedMemberOrigin {
                    label: "import_file lib.lk".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "import_module math".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "import_item sqrt".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "import_alias root".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "import_namespace m".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "attr derive".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "attr_arg all".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "attr_key feature".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "attr_value debug".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "derive Debug".to_string(),
                    span: Some(span),
                },
            ],
        }],
    }];
    let mut symbols = Vec::new();
    LkAnalyzer::append_ast_macro_origin_symbols(&mut symbols, &origins);

    let members = symbols
        .iter()
        .find(|symbol| symbol.name == "Macro Expansions")
        .and_then(|macros| macros.children.as_ref())
        .and_then(|origins| {
            origins
                .iter()
                .find(|symbol| symbol.name == "attribute generated_metadata")
        })
        .and_then(|origin| origin.children.as_ref())
        .and_then(|items| items.iter().find(|symbol| symbol.name == "statement"))
        .and_then(|item| item.children.as_ref())
        .expect("generated member symbols");
    let kind_for = |name: &str| {
        members
            .iter()
            .find(|symbol| symbol.name == name)
            .map(|symbol| symbol.kind)
            .unwrap_or_else(|| panic!("missing generated member symbol `{name}`: {members:?}"))
    };

    assert_eq!(kind_for("import_file lib.lk"), SymbolKind::FILE);
    assert_eq!(kind_for("import_module math"), SymbolKind::MODULE);
    assert_eq!(kind_for("import_item sqrt"), SymbolKind::VARIABLE);
    assert_eq!(kind_for("import_alias root"), SymbolKind::VARIABLE);
    assert_eq!(kind_for("import_namespace m"), SymbolKind::MODULE);
    assert_eq!(kind_for("attr derive"), SymbolKind::PROPERTY);
    assert_eq!(kind_for("attr_arg all"), SymbolKind::PROPERTY);
    assert_eq!(kind_for("attr_key feature"), SymbolKind::PROPERTY);
    assert_eq!(kind_for("attr_value debug"), SymbolKind::PROPERTY);
    assert_eq!(kind_for("derive Debug"), SymbolKind::ENUM_MEMBER);
}

#[test]
fn test_ast_macro_reference_origins_appear_as_typed_document_symbols() {
    let span = Span::new(token::Position::new(4, 7, 60), token::Position::new(4, 31, 84));
    let origins = vec![macro_system::AstMacroOrigin {
        macro_name: "generated_refs".to_string(),
        kind: macro_system::AstMacroOriginKind::Attribute,
        input_span: Some(span.clone()),
        generated_items: 1,
        generated_item_labels: vec!["fn generated".to_string()],
        generated_item_origins: vec![macro_system::AstGeneratedItemOrigin {
            label: "fn generated".to_string(),
            span: Some(span.clone()),
            generated_member_origins: vec![
                macro_system::AstGeneratedMemberOrigin {
                    label: "binding current".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "ref seed".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "assign_ref current".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "compound_assign_ref current".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "call helper".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "literal int".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "binary add".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "unary not".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "range step".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "pattern element".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "for_pattern rest".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "expr call_arg".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "struct_field id".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "map_key kind".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "named_arg value".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "named_param_type current".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "type_ref User".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "type_var T".to_string(),
                    span: Some(span.clone()),
                },
                macro_system::AstGeneratedMemberOrigin {
                    label: "type_expr function".to_string(),
                    span: Some(span),
                },
            ],
        }],
    }];
    let mut symbols = Vec::new();
    LkAnalyzer::append_ast_macro_origin_symbols(&mut symbols, &origins);

    let members = symbols
        .iter()
        .find(|symbol| symbol.name == "Macro Expansions")
        .and_then(|macros| macros.children.as_ref())
        .and_then(|origins| origins.iter().find(|symbol| symbol.name == "attribute generated_refs"))
        .and_then(|origin| origin.children.as_ref())
        .and_then(|items| items.iter().find(|symbol| symbol.name == "fn generated"))
        .and_then(|item| item.children.as_ref())
        .expect("generated member symbols");
    let kind_for = |name: &str| {
        members
            .iter()
            .find(|symbol| symbol.name == name)
            .map(|symbol| symbol.kind)
            .unwrap_or_else(|| panic!("missing generated member symbol `{name}`: {members:?}"))
    };

    assert_eq!(kind_for("binding current"), SymbolKind::VARIABLE);
    assert_eq!(kind_for("ref seed"), SymbolKind::VARIABLE);
    assert_eq!(kind_for("assign_ref current"), SymbolKind::VARIABLE);
    assert_eq!(kind_for("compound_assign_ref current"), SymbolKind::VARIABLE);
    assert_eq!(kind_for("call helper"), SymbolKind::FUNCTION);
    assert_eq!(kind_for("literal int"), SymbolKind::CONSTANT);
    assert_eq!(kind_for("binary add"), SymbolKind::OPERATOR);
    assert_eq!(kind_for("unary not"), SymbolKind::OPERATOR);
    assert_eq!(kind_for("range step"), SymbolKind::EVENT);
    assert_eq!(kind_for("pattern element"), SymbolKind::EVENT);
    assert_eq!(kind_for("for_pattern rest"), SymbolKind::EVENT);
    assert_eq!(kind_for("expr call_arg"), SymbolKind::OPERATOR);
    assert_eq!(kind_for("struct_field id"), SymbolKind::FIELD);
    assert_eq!(kind_for("map_key kind"), SymbolKind::PROPERTY);
    assert_eq!(kind_for("named_arg value"), SymbolKind::PROPERTY);
    assert_eq!(kind_for("named_param_type current"), SymbolKind::PROPERTY);
    assert_eq!(kind_for("type_ref User"), SymbolKind::TYPE_PARAMETER);
    assert_eq!(kind_for("type_var T"), SymbolKind::TYPE_PARAMETER);
    assert_eq!(kind_for("type_expr function"), SymbolKind::TYPE_PARAMETER);
}

fn full_range(s: &str) -> Range {
    let lines = s.lines().count();
    let end_line = (lines.saturating_sub(1)) as u32;
    let end_col = s.lines().last().map(|l| l.len() as u32).unwrap_or(0);
    Range::new(Position::new(0, 0), Position::new(end_line, end_col))
}

/// The editor's outline shows what the writer wrote.
///
/// A parse-time desugar binds a temporary — `a?.m()` and `expr!` both do — and
/// those temporaries were listed in the document outline beside the real
/// variables, as `__optcall0` and `__unwrap1`. They could not be filtered by
/// name either: `__optcall0` is a name a program may legitimately spell. They
/// are minted with a `$` now, which no source identifier can contain, and the
/// resolver keeps them out of the list tools read.
#[test]
fn the_outline_lists_only_the_writers_variables() {
    fn flatten(symbols: &[tower_lsp::lsp_types::DocumentSymbol], out: &mut Vec<String>) {
        for symbol in symbols {
            out.push(symbol.name.clone());
            if let Some(children) = &symbol.children {
                flatten(children, out);
            }
        }
    }

    let mut analyzer = LkAnalyzer::new();
    let src = "let m = {\"a\": \"xy\"};\nlet n = m.get(\"a\")?.len();\nlet v = n!;\n";
    let result = analyzer.analyze(src);
    let mut names = Vec::new();
    flatten(&result.symbols, &mut names);

    for written in ["m", "n", "v"] {
        assert!(
            names.iter().any(|name| name == written),
            "{written} should be listed: {names:?}"
        );
    }
    assert!(
        !names.iter().any(|name| name.contains('$')),
        "a desugar's temporary is not the writer's variable: {names:?}"
    );
}
