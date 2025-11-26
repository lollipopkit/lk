use lkr_core::{
    ast::Parser as ExprParser,
    stmt::{self, stmt_parser::StmtParser, ImportStmt, Stmt},
    token::{self, Tokenizer},
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_lsp::lsp_types::*;

// Re-implement the analyzer for testing since we can't import from lkr_lsp
#[derive(Debug, Clone)]
pub struct AnalysisResult {
    pub diagnostics: Vec<Diagnostic>,
    pub symbols: Vec<DocumentSymbol>,
    pub identifier_roots: HashSet<String>,
}

#[derive(Default)]
pub struct LkrAnalyzer;

impl LkrAnalyzer {
    pub fn new() -> Self {
        Self
    }

    pub fn analyze(&self, content: &str) -> AnalysisResult {
        let mut result = AnalysisResult {
            diagnostics: Vec::new(),
            symbols: Vec::new(),
            identifier_roots: HashSet::new(),
        };

        // Try parsing as expression first
        let tokens = match Tokenizer::tokenize(content) {
            Ok(tokens) => tokens,
            Err(tokenize_err) => {
                result.diagnostics.push(Diagnostic::new(
                    Range::new(Position::new(0, 0), Position::new(0, content.len() as u32)),
                    Some(DiagnosticSeverity::ERROR),
                    None,
                    Some("lkr".to_string()),
                    format!("Tokenization error: {}", tokenize_err),
                    None,
                    None,
                ));
                return result;
            }
        };

        let mut expr_parser = ExprParser::new(&tokens);
        match expr_parser.parse() {
            Ok(expr) => {
                // Collect identifier roots
                result.identifier_roots = expr.requested_ctx();

                // Add expression symbol
                let symbol = DocumentSymbol {
                    name: "expression".to_string(),
                    detail: Some("LKR Expression".to_string()),
                    kind: SymbolKind::CONSTANT,
                    tags: None,
                    #[allow(deprecated)]
                    deprecated: None,
                    range: Range::new(Position::new(0, 0), Position::new(0, content.len() as u32)),
                    selection_range: Range::new(Position::new(0, 0), Position::new(0, content.len() as u32)),
                    children: None,
                };
                result.symbols.push(symbol);
            }
            Err(expr_err) => {
                // Try parsing as statement program
                let mut stmt_parser = StmtParser::new(&tokens);
                match stmt_parser.parse_program() {
                    Ok(program) => {
                        // Analyze statements for symbols and identifier roots
                        self.analyze_statements(&program.statements, &mut result);
                    }
                    Err(stmt_err) => {
                        // Both parsing attempts failed
                        result.diagnostics.push(Diagnostic::new(
                            Range::new(Position::new(0, 0), Position::new(0, content.len() as u32)),
                            Some(DiagnosticSeverity::ERROR),
                            None,
                            Some("lkr".to_string()),
                            format!("Parse error - Expression: {}, Statement: {}", expr_err, stmt_err),
                            None,
                            None,
                        ));
                    }
                }
            }
        }

        result
    }

    fn analyze_statements(&self, statements: &[Box<Stmt>], result: &mut AnalysisResult) {
        for (i, stmt) in statements.iter().enumerate() {
            match stmt.as_ref() {
                Stmt::Let { pattern, .. } => {
                    // Extract variable names from pattern and create symbols for each
                    if let Some(variables) = lkr_lsp::analyzer::extract_variables_from_pattern(pattern) {
                        for var_name in variables {
                            result.symbols.push(DocumentSymbol {
                                name: var_name.clone(),
                                detail: Some("Variable declaration".to_string()),
                                kind: SymbolKind::VARIABLE,
                                tags: None,
                                #[allow(deprecated)]
                                deprecated: None,
                                range: Range::new(Position::new(i as u32, 0), Position::new(i as u32, 100)),
                                selection_range: Range::new(Position::new(i as u32, 0), Position::new(i as u32, 100)),
                                children: None,
                            });
                        }
                    }
                }
                Stmt::Function { name, params, .. } => {
                    result.symbols.push(DocumentSymbol {
                        name: name.clone(),
                        detail: Some(format!("Function({})", params.join(", "))),
                        kind: SymbolKind::FUNCTION,
                        tags: None,
                        #[allow(deprecated)]
                        deprecated: None,
                        range: Range::new(Position::new(i as u32, 0), Position::new(i as u32, 100)),
                        selection_range: Range::new(Position::new(i as u32, 0), Position::new(i as u32, 100)),
                        children: None,
                    });
                }
                Stmt::Import(import_stmt) => {
                    let import_name = match import_stmt {
                        ImportStmt::Module { module } => module.clone(),
                        ImportStmt::File { path } => path.clone(),
                        ImportStmt::Items { source, .. } => match source {
                            stmt::ImportSource::Module(name) => name.clone(),
                            stmt::ImportSource::File(path) => path.clone(),
                        },
                        ImportStmt::Namespace { source, .. } => match source {
                            stmt::ImportSource::Module(name) => name.clone(),
                            stmt::ImportSource::File(path) => path.clone(),
                        },
                        ImportStmt::ModuleAlias { module, .. } => module.clone(),
                    };
                    result.symbols.push(DocumentSymbol {
                        name: format!("import {}", import_name),
                        detail: Some("Import statement".to_string()),
                        kind: SymbolKind::MODULE,
                        tags: None,
                        #[allow(deprecated)]
                        deprecated: None,
                        range: Range::new(Position::new(i as u32, 0), Position::new(i as u32, 100)),
                        selection_range: Range::new(Position::new(i as u32, 0), Position::new(i as u32, 100)),
                        children: None,
                    });
                }
                _ => {}
            }
        }
    }

    pub fn get_var_completions(&self, prefix: &str) -> Vec<CompletionItem> {
        let mut items = Vec::new();

        // Common variable patterns
        let common_contexts = [
            ("req", "Request object"),
            ("req.user", "User information"),
            ("req.user.id", "User ID"),
            ("req.user.role", "User role"),
            ("req.user.name", "User name"),
            ("record", "Record object"),
            ("record.id", "Record ID"),
            ("record.owner", "Record owner"),
            ("record.granted", "Granted users list"),
            ("env", "Environment variables"),
            ("time", "Current timestamp"),
        ];

        for (context, desc) in common_contexts {
            if context.starts_with(prefix) {
                items.push(CompletionItem {
                    label: context.to_string(),
                    kind: Some(CompletionItemKind::PROPERTY),
                    detail: Some(desc.to_string()),
                    ..Default::default()
                });
            }
        }

        items
    }
}

use url::Url;

// Test helper to create a mock language server
struct TestLanguageServer {
    documents: Arc<RwLock<HashMap<Url, TestDocument>>>,
    analyzer: LkrAnalyzer,
}

struct TestDocument {
    content: String,
    #[allow(dead_code)] // Keep for future version tracking
    version: i32,
}

impl TestLanguageServer {
    fn new() -> Self {
        Self {
            documents: Arc::new(RwLock::new(HashMap::new())),
            analyzer: LkrAnalyzer::new(),
        }
    }

    async fn open_document(&self, uri: Url, content: String, version: i32) {
        let document = TestDocument { content, version };
        self.documents.write().await.insert(uri, document);
    }

    async fn update_document(&self, uri: Url, content: String, version: i32) {
        let document = TestDocument { content, version };
        self.documents.write().await.insert(uri, document);
    }

    async fn validate_document(&self, uri: &Url) -> Vec<Diagnostic> {
        let documents = self.documents.read().await;
        let Some(document) = documents.get(uri) else {
            return Vec::new();
        };

        let analysis = self.analyzer.analyze(&document.content);
        analysis.diagnostics
    }

    async fn get_hover_info(&self, uri: &Url) -> Option<Hover> {
        // Tokenize and provide a position-aware hover over the first non-whitespace token

        let documents = self.documents.read().await;
        let document = documents.get(uri)?;
        let content = &document.content;

        // Tokenize with spans; pick first non-whitespace token to emulate a hover position
        let (tokens, spans) = token::Tokenizer::tokenize_enhanced_with_spans(content).ok()?;
        let hover_idx = Self::first_non_ws_token_index(content, &spans)?;
        let text = Self::describe_token_hover_test(&tokens, hover_idx);

        Some(Hover {
            contents: HoverContents::Scalar(MarkedString::String(text)),
            range: None,
        })
    }

    fn first_non_ws_token_index(content: &str, spans: &[token::Span]) -> Option<usize> {
        for (i, sp) in spans.iter().enumerate() {
            let start = sp.start.offset;
            let end = sp.end.offset.min(content.len());
            let slice = &content[start..end];
            if !slice.chars().all(|c| c.is_whitespace()) {
                return Some(i);
            }
        }
        None
    }

    fn describe_token_hover_test(tokens: &[token::Token], idx: usize) -> String {
        use token::Token as T;
        match &tokens[idx] {
            T::Id(name) => {
                let is_call = tokens.get(idx + 1).map(|t| matches!(t, T::LParen)).unwrap_or(false);
                if is_call {
                    format!("Function call: {}(…)", name)
                } else {
                    format!("Identifier: {}", name)
                }
            }
            T::Str(s) => format!("String literal: \"{}\"", s),
            T::Int(i) => format!("Integer: {}", i),
            T::Float(f) => format!("Float: {}", f),
            T::Bool(b) => format!("Boolean: {}", b),
            T::Nil => "Nil literal".to_string(),
            T::If => "Keyword: if".to_string(),
            T::Else => "Keyword: else".to_string(),
            T::While => "Keyword: while".to_string(),
            T::Let => "Keyword: let".to_string(),
            T::Break => "Keyword: break".to_string(),
            T::Continue => "Keyword: continue".to_string(),
            T::Return => "Keyword: return".to_string(),
            T::Struct => "Keyword: struct".to_string(),
            T::Fn => "Keyword: fn".to_string(),
            T::Import => "Keyword: import".to_string(),
            T::From => "Keyword: from".to_string(),
            T::Const => "Keyword: const".to_string(),
            T::As => "Keyword: as".to_string(),
            T::Eq => "Operator: ==".to_string(),
            T::Ne => "Operator: !=".to_string(),
            T::Ge => "Operator: >=".to_string(),
            T::Le => "Operator: <=".to_string(),
            T::Gt => "Operator: >".to_string(),
            T::Lt => "Operator: <".to_string(),
            T::And => "Operator: &&".to_string(),
            T::Or => "Operator: ||".to_string(),
            T::Not => "Operator: !".to_string(),
            T::In => "Operator: in".to_string(),
            T::Assign => "Operator: =".to_string(),
            T::Add => "Operator: +".to_string(),
            T::Sub => "Operator: -".to_string(),
            T::Mul => "Operator: *".to_string(),
            T::Div => "Operator: /".to_string(),
            T::Mod => "Operator: %".to_string(),
            T::Dot => "Accessor: .".to_string(),
            T::Colon => "Symbol: :".to_string(),
            T::Comma => "Symbol: ,".to_string(),
            T::Semicolon => "Symbol: ;".to_string(),
            // '@' token removed from lexer
            T::LParen => "Symbol: (".to_string(),
            T::RParen => "Symbol: )".to_string(),
            T::LBrace => "Symbol: {".to_string(),
            T::RBrace => "Symbol: }".to_string(),
            T::LBracket => "Symbol: [".to_string(),
            T::RBracket => "Symbol: ]".to_string(),
            T::For => "Keyword: for".to_string(),
            T::Range => "Operator: ..".to_string(),
            T::RangeInclusive => "Operator: ..=".to_string(),
            T::Select => "Keyword: select".to_string(),
            T::Case => "Keyword: case".to_string(),
            T::Default => "Keyword: default".to_string(),
            T::Arrow => "Operator: =>".to_string(),
            T::LeftArrow => "Operator: <-".to_string(),
            T::OptionalDot => "Operator: ?.".to_string(),
            T::NullishCoalescing => "Operator: ??".to_string(),
            T::TemplateString(_) => "Formatted string".to_string(),
            // Type system tokens
            T::Type => "Keyword: type".to_string(),
            T::Trait => "Keyword: trait".to_string(),
            T::Impl => "Keyword: impl".to_string(),
            T::Pipe => "Operator: |".to_string(),
            T::Question => "Operator: ?".to_string(),
            T::FnArrow => "Operator: ->".to_string(),
            T::AddAssign => "Operator: +=".to_string(),
            T::SubAssign => "Operator: -=".to_string(),
            T::MulAssign => "Operator: *=".to_string(),
            T::DivAssign => "Operator: /=".to_string(),
            T::ModAssign => "Operator: %=".to_string(),
            T::Match => "Keyword: match".to_string(),
        }
    }

    fn get_completions(&self) -> Vec<CompletionItem> {
        let mut items = Vec::new();

        // LKR keywords
        let keywords = [
            "if", "else", "while", "let", "fn", "return", "break", "continue", "import", "from", "as", "go", "select",
            "case", "default", "true", "false", "nil", "struct", "const", "for", "in", "spawn", "chan", "send", "recv",
            "type",
        ];

        for keyword in keywords {
            items.push(CompletionItem {
                label: keyword.to_string(),
                kind: Some(CompletionItemKind::KEYWORD),
                detail: Some("LKR keyword".to_string()),
                ..Default::default()
            });
        }

        // Operators
        let operators = [
            "==", "!=", "<=", ">=", "&&", "||", "in", "<-", "=>", "?.", "??", "+", "-", "*", "/", "%", "=", ">", "<",
            "!", "..", "..=", "+=", "-=", "*=", "/=", "%=", "->", "|", "?",
        ];
        for op in operators {
            items.push(CompletionItem {
                label: op.to_string(),
                kind: Some(CompletionItemKind::OPERATOR),
                detail: Some("LKR operator".to_string()),
                ..Default::default()
            });
        }

        // Context access (identifiers)
        items.push(CompletionItem {
            label: "req".to_string(),
            kind: Some(CompletionItemKind::VARIABLE),
            detail: Some("Identifier root example".to_string()),
            documentation: Some(Documentation::String(
                "Access common variables (e.g., req.user.role)".to_string(),
            )),
            ..Default::default()
        });

        items
    }

    async fn get_document_symbols(&self, uri: &Url) -> Option<Vec<DocumentSymbol>> {
        let documents = self.documents.read().await;

        if let Some(document) = documents.get(uri) {
            let analysis = self.analyzer.analyze(&document.content);
            if !analysis.symbols.is_empty() {
                return Some(analysis.symbols);
            }
        }

        None
    }
}

#[tokio::test]
async fn test_lsp_expression_validation() {
    let server = TestLanguageServer::new();
    let uri = Url::parse("file:///test.lkr").unwrap();

    // Test valid expression
    server
        .open_document(uri.clone(), "req.user.role == 'admin'".to_string(), 1)
        .await;
    let diagnostics = server.validate_document(&uri).await;
    assert!(diagnostics.is_empty());

    // Test invalid expression (tokenization error)
    server
        .update_document(uri.clone(), "req.user.role == 'unterminated".to_string(), 2)
        .await;
    let diagnostics = server.validate_document(&uri).await;
    assert!(!diagnostics.is_empty());
    assert_eq!(diagnostics[0].severity, Some(DiagnosticSeverity::ERROR));
}

#[tokio::test]
async fn test_lsp_validate_missing_document_returns_empty() {
    let server = TestLanguageServer::new();
    let uri = Url::parse("file:///not_opened.lkr").unwrap();

    let diagnostics = server.validate_document(&uri).await;
    assert!(diagnostics.is_empty(), "expected no diagnostics for unopened document");
}

#[tokio::test]
async fn test_lsp_statement_validation() {
    let server = TestLanguageServer::new();
    let uri = Url::parse("file:///program.lkr").unwrap();

    let program = r#"
        import math;
        let user_level = req.user.level;
        fn calculate_score(base) {
            return math.sqrt(base * user_level);
        }
    "#;

    server.open_document(uri.clone(), program.to_string(), 1).await;
    let diagnostics = server.validate_document(&uri).await;
    assert!(diagnostics.is_empty());

    // Test invalid statement
    server
        .update_document(uri.clone(), "let invalid_statement".to_string(), 2)
        .await;
    let diagnostics = server.validate_document(&uri).await;
    assert!(!diagnostics.is_empty());
    assert_eq!(diagnostics[0].severity, Some(DiagnosticSeverity::ERROR));
}

#[tokio::test]
async fn test_lsp_hover_for_whitespace_document_is_none() {
    let server = TestLanguageServer::new();
    let uri = Url::parse("file:///blank.lkr").unwrap();

    server.open_document(uri.clone(), "    \n\t\n".to_string(), 1).await;
    let hover = server.get_hover_info(&uri).await;
    assert!(hover.is_none(), "expected no hover info for whitespace-only document");
}

#[tokio::test]
async fn test_lsp_hover_functionality() {
    let server = TestLanguageServer::new();
    let uri = Url::parse("file:///test.lkr").unwrap();

    // Test hover with identifier roots
    server
        .open_document(
            uri.clone(),
            "req.user.role == 'admin' && req.user.id > 0".to_string(),
            1,
        )
        .await;
    let hover = server.get_hover_info(&uri).await;
    assert!(hover.is_some());

    let hover = hover.unwrap();
    if let HoverContents::Scalar(MarkedString::String(content)) = hover.contents {
        // Should describe a token, not necessarily a member path
        assert!(
            content.contains("Identifier:") || content.contains("Operator:") || content.contains("String literal:")
        );
    } else {
        panic!("Expected string hover content");
    }

    // Test hover with statements (symbols)
    let program = r#"
        import math;
        let result = math.sqrt(42);
        fn test() { return result; }
    "#;
    server.update_document(uri.clone(), program.to_string(), 2).await;
    let hover = server.get_hover_info(&uri).await;
    assert!(hover.is_some());

    let hover = hover.unwrap();
    if let HoverContents::Scalar(MarkedString::String(content)) = hover.contents {
        // The first non-whitespace token in this program should be the 'import' keyword
        assert!(content.contains("Keyword:"));
    } else {
        panic!("Expected string hover content");
    }
}

#[tokio::test]
async fn test_lsp_completion_functionality() {
    let server = TestLanguageServer::new();
    let completions = server.get_completions();

    assert!(!completions.is_empty());

    // Check for keywords
    let labels: Vec<&String> = completions.iter().map(|c| &c.label).collect();
    assert!(labels.contains(&&"if".to_string()));
    assert!(labels.contains(&&"let".to_string()));
    assert!(labels.contains(&&"fn".to_string()));
    assert!(labels.contains(&&"import".to_string()));

    // Check for operators
    assert!(labels.contains(&&"==".to_string()));
    assert!(labels.contains(&&"&&".to_string()));
    assert!(labels.contains(&&"||".to_string()));

    // Check for common access root identifier (no legacy '@')
    assert!(labels.contains(&&"req".to_string()));

    // Verify completion kinds
    let keyword_items: Vec<_> = completions
        .iter()
        .filter(|item| item.kind == Some(CompletionItemKind::KEYWORD))
        .collect();
    assert!(!keyword_items.is_empty());

    let operator_items: Vec<_> = completions
        .iter()
        .filter(|item| item.kind == Some(CompletionItemKind::OPERATOR))
        .collect();
    assert!(!operator_items.is_empty());

    let context_items: Vec<_> = completions
        .iter()
        .filter(|item| item.kind == Some(CompletionItemKind::VARIABLE))
        .collect();
    assert!(!context_items.is_empty());
}

#[tokio::test]
async fn test_lsp_document_symbols() {
    let server = TestLanguageServer::new();
    let uri = Url::parse("file:///program.lkr").unwrap();

    let program = r#"
        import math;
        import string;
        
        let global_var = 42;
        
        fn process_data(data) {
            let local_var = data * 2;
            return math.sqrt(local_var);
        }
        
        fn main() {
            let result = process_data(global_var);
            return result;
        }
        
        let final_result = main();
    "#;

    server.open_document(uri.clone(), program.to_string(), 1).await;
    let symbols = server.get_document_symbols(&uri).await;
    assert!(symbols.is_some());

    let symbols = symbols.unwrap();
    assert!(symbols.len() >= 6); // 2 imports, 3 lets, 2 functions

    let symbol_names: Vec<&String> = symbols.iter().map(|s| &s.name).collect();
    assert!(symbol_names.contains(&&"import math".to_string()));
    assert!(symbol_names.contains(&&"import string".to_string()));
    assert!(symbol_names.contains(&&"global_var".to_string()));
    assert!(symbol_names.contains(&&"process_data".to_string()));
    assert!(symbol_names.contains(&&"main".to_string()));

    // Check symbol kinds
    let import_symbols: Vec<_> = symbols.iter().filter(|s| s.kind == SymbolKind::MODULE).collect();
    assert_eq!(import_symbols.len(), 2);

    let function_symbols: Vec<_> = symbols.iter().filter(|s| s.kind == SymbolKind::FUNCTION).collect();
    assert_eq!(function_symbols.len(), 2);

    let variable_symbols: Vec<_> = symbols.iter().filter(|s| s.kind == SymbolKind::VARIABLE).collect();
    // Only top-level variables are detected in our simple analyzer
    assert!(variable_symbols.len() >= 2); // At least global_var and final_result
}

#[tokio::test]
async fn test_lsp_document_symbols_for_closed_document() {
    let server = TestLanguageServer::new();
    let uri = Url::parse("file:///missing_symbols.lkr").unwrap();

    let symbols = server.get_document_symbols(&uri).await;
    assert!(symbols.is_none(), "expected None for symbols on unopened document");
}

#[tokio::test]
async fn test_lsp_var_completions() {
    let analyzer = LkrAnalyzer::new();

    // Test context completions with "req" prefix
    let completions = analyzer.get_var_completions("req");
    assert!(!completions.is_empty());

    let labels: Vec<&String> = completions.iter().map(|c| &c.label).collect();
    assert!(labels.contains(&&"req".to_string()));
    assert!(labels.contains(&&"req.user".to_string()));
    assert!(labels.contains(&&"req.user.id".to_string()));
    assert!(labels.contains(&&"req.user.role".to_string()));

    // Should not include non-matching prefixes
    assert!(!labels.contains(&&"record".to_string()));
    assert!(!labels.contains(&&"env".to_string()));
}

#[tokio::test]
async fn test_lsp_complex_program_analysis() {
    let server = TestLanguageServer::new();
    let uri = Url::parse("file:///complex.lkr").unwrap();

    let complex_program = r#"
        import math;
        import string;
        import datetime;
        
        let user_level = req.user.level;
        let user_name = req.user.name;
        let record_id = record.id;
        
        fn validate_access(user_role) {
            if (user_role == "admin") {
                return true;
            }
            
            if (user_role == "moderator" && user_level > 5) {
                return true;
            }
            
            return false;
        }
        
        fn calculate_score(base_score) {
            let adjusted_score = base_score * math.sqrt(user_level);
            let name_bonus = string.len(user_name) * 2;
            return adjusted_score + name_bonus;
        }
        
        let access_granted = validate_access(req.user.role);
        
        if (access_granted) {
            let score = calculate_score(100);
            let timestamp = datetime.now();
            return score;
        } else {
            return 0;
        }
    "#;

    server.open_document(uri.clone(), complex_program.to_string(), 1).await;

    // Test diagnostics - should be clean
    let diagnostics = server.validate_document(&uri).await;
    assert!(diagnostics.is_empty());

    // Test document symbols
    let symbols = server.get_document_symbols(&uri).await;
    assert!(symbols.is_some());
    let symbols = symbols.unwrap();

    // Should have imports, variables, functions
    assert!(symbols.len() >= 6);

    let symbol_names: Vec<&String> = symbols.iter().map(|s| &s.name).collect();

    // Check imports
    assert!(symbol_names.contains(&&"import math".to_string()));
    assert!(symbol_names.contains(&&"import string".to_string()));
    assert!(symbol_names.contains(&&"import datetime".to_string()));

    // Check variables
    assert!(symbol_names.contains(&&"user_level".to_string()));
    assert!(symbol_names.contains(&&"user_name".to_string()));
    assert!(symbol_names.contains(&&"record_id".to_string()));

    // Check functions
    assert!(symbol_names.contains(&&"validate_access".to_string()));
    assert!(symbol_names.contains(&&"calculate_score".to_string()));

    // Test hover - should detect identifier roots or symbols
    let hover = server.get_hover_info(&uri).await;
    assert!(hover.is_some());

    let hover = hover.unwrap();
    if let HoverContents::Scalar(MarkedString::String(content)) = hover.contents {
        // Should describe a token at start of file; accept broad categories
        assert!(
            content.contains("Keyword:")
                || content.contains("Identifier:")
                || content.contains("Member path:")
                || content.contains("Operator:")
                || content.contains("String literal:")
        );
    } else {
        panic!("Expected string hover content");
    }
}

#[tokio::test]
async fn test_lsp_error_recovery() {
    let server = TestLanguageServer::new();
    let uri = Url::parse("file:///error_test.lkr").unwrap();

    // Test various error conditions
    let error_cases = [
        ("", vec![]),                                                        // Empty document should be fine
        ("req.user.role == 'unterminated", vec![DiagnosticSeverity::ERROR]), // Tokenization error
        ("let incomplete", vec![DiagnosticSeverity::ERROR]),                 // Incomplete statement
        ("req.user.role == 'admin'", vec![]),                                // Valid expression should work
    ];

    for (i, (code, expected_severities)) in error_cases.iter().enumerate() {
        server
            .update_document(uri.clone(), code.to_string(), i as i32 + 1)
            .await;
        let diagnostics = server.validate_document(&uri).await;

        assert_eq!(
            diagnostics.len(),
            expected_severities.len(),
            "Test case {}: Expected {} diagnostics for '{}', got {}",
            i,
            expected_severities.len(),
            code,
            diagnostics.len()
        );

        for (diagnostic, expected_severity) in diagnostics.iter().zip(expected_severities.iter()) {
            assert_eq!(
                diagnostic.severity,
                Some(*expected_severity),
                "Test case {}: Expected severity {:?} for '{}', got {:?}",
                i,
                expected_severity,
                code,
                diagnostic.severity
            );
        }
    }
}
