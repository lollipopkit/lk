use crate::compat::path::PathBuf;
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::token::token_lexeme;
use crate::{
    ast::Parser as ExprParser,
    expr::Expr,
    macro_system::{
        AstMacroOrigin, MacroDefinitions, MacroExpandOptions, MacroTokenOrigin, MacroTrace, PackageMacroModuleResolver,
        ProcMacroDependency, ProcMacroDependencyRecorder, ProcMacroOptions, ProcMacroProviders,
        expand_ast_macros_with_metadata, expand_macros,
    },
    stmt::{Program, StmtParser},
    token::{ParseError, Token, Tokenizer},
    typ,
    val::LiteralVal,
};

#[derive(Debug, Clone)]
pub struct ParseOptions {
    pub expand_macros: bool,
    pub macro_trace: bool,
    pub recursion_limit: usize,
    pub base_dir: Option<PathBuf>,
    pub macro_features: Vec<String>,
    pub proc_macro_providers: ProcMacroProviders,
    /// How a package-named macro import finds its module — see
    /// [`PackageMacroModuleResolver`]. Defaulted to the package manager's own
    /// lookup, which is what makes this the *only* place the two meet.
    pub package_macro_resolver: Option<PackageMacroModuleResolver>,
    /// `macro_rules!` definitions from an earlier parse to keep in scope.
    ///
    /// Empty for a single-shot compile, where a source text carries its own
    /// definitions. A REPL parses each input separately and so must carry them
    /// itself, or a macro stops existing at the end of the line that defined it.
    pub carried_macro_definitions: MacroDefinitions,
}

#[derive(Debug, Clone)]
pub struct SourceExpansion {
    pub tokens: Vec<Token>,
    pub spans: Vec<crate::token::Span>,
    pub origins: Vec<MacroTokenOrigin>,
    pub trace: Vec<MacroTrace>,
    pub proc_macro_dependencies: Vec<ProcMacroDependency>,
    /// What this source defined, for a caller that parses again and wants the
    /// definitions still in scope — see [`ParseOptions::carried_macro_definitions`].
    pub macro_definitions: MacroDefinitions,
}

#[derive(Debug, Clone)]
pub struct ProgramExpansion {
    pub source: SourceExpansion,
    pub program: Program,
    pub ast_expanded: bool,
    pub ast_macro_origins: Vec<AstMacroOrigin>,
    pub proc_macro_dependencies: Vec<ProcMacroDependency>,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self {
            expand_macros: true,
            macro_trace: false,
            recursion_limit: 128,
            base_dir: None,
            macro_features: Vec::new(),
            proc_macro_providers: ProcMacroProviders::default(),
            #[cfg(feature = "std")]
            package_macro_resolver: Some(crate::package::macro_module_root),
            #[cfg(not(feature = "std"))]
            package_macro_resolver: None,
            carried_macro_definitions: MacroDefinitions::default(),
        }
    }
}

pub fn parse_program_source(source: &str, options: ParseOptions) -> Result<Program, ParseError> {
    Ok(expand_program_source(source, options)?.program)
}

pub fn expand_program_source(source: &str, options: ParseOptions) -> Result<ProgramExpansion, ParseError> {
    let expand_ast = options.expand_macros;
    let dependency_recorder = ProcMacroDependencyRecorder::default();
    let proc_macro_options = ProcMacroOptions {
        features: options.macro_features.clone(),
        providers: options.proc_macro_providers.clone(),
        dependency_recorder: dependency_recorder.clone(),
        ..ProcMacroOptions::default()
    };
    let source_expansion = expand_source_with_recorder(source, options, dependency_recorder.clone())?;
    let mut parser = StmtParser::new_with_spans(&source_expansion.tokens, &source_expansion.spans);
    let parsed_program = parser
        .parse_program_with_enhanced_errors(source)
        .map_err(|error| enrich_parse_error_with_macro_origins(error, &source_expansion))?;
    let (mut program, ast_macro_origins) = if expand_ast {
        let expanded = expand_ast_macros_with_metadata(parsed_program.clone(), proc_macro_options)?;
        (expanded.program, expanded.origins)
    } else {
        (parsed_program.clone(), Vec::new())
    };
    // `defer` is erased here, after macros and before everything else.
    //
    // After macros because a macro may expand to one; before everything else
    // because nothing downstream should know it existed. It is a rewrite of the
    // program's *shape*, not a runtime mechanism — see `stmt::defer`.
    crate::stmt::defer::desugar_defers(&mut program.statements).map_err(ParseError::new)?;
    // A trait's default method bodies are copied into the impls that left them
    // out — here, for the same reason `defer` is erased here: after macros
    // (which may write a trait or an impl) and before anything that dispatches.
    crate::stmt::trait_defaults::apply_trait_defaults(&mut program.statements);
    // A constructor beside every `struct`, so the module that owns a type is
    // the one that builds it — see `stmt::struct_ctors` for why that is the
    // whole trick.
    crate::stmt::struct_ctors::add_struct_constructors(&mut program.statements).map_err(ParseError::new)?;
    Ok(ProgramExpansion {
        ast_expanded: program != parsed_program,
        source: source_expansion,
        program,
        ast_macro_origins,
        proc_macro_dependencies: dependency_recorder.dependencies(),
    })
}

pub fn parse_expr_source(source: &str, options: ParseOptions) -> Result<Expr, ParseError> {
    let expanded = expand_source(source, options)?;
    let mut parser = ExprParser::new_with_spans(&expanded.tokens, &expanded.spans);
    parser
        .parse_with_enhanced_errors(source)
        .map_err(|error| enrich_parse_error_with_macro_origins(error, &expanded))
}

pub fn expand_source(source: &str, options: ParseOptions) -> Result<SourceExpansion, ParseError> {
    expand_source_with_recorder(source, options, ProcMacroDependencyRecorder::default())
}

fn expand_source_with_recorder(
    source: &str,
    options: ParseOptions,
    dependency_recorder: ProcMacroDependencyRecorder,
) -> Result<SourceExpansion, ParseError> {
    let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(source)?;
    if !options.expand_macros {
        return Ok(SourceExpansion {
            tokens,
            spans,
            origins: Vec::new(),
            trace: Vec::new(),
            proc_macro_dependencies: Vec::new(),
            macro_definitions: MacroDefinitions::default(),
        });
    }
    let expanded = expand_macros(
        tokens,
        spans,
        MacroExpandOptions {
            recursion_limit: options.recursion_limit,
            trace: options.macro_trace,
            base_dir: options.base_dir,
            package_macro_resolver: options.package_macro_resolver,
            proc_macro_providers: options.proc_macro_providers,
            proc_macro_features: options.macro_features,
            proc_macro_dependency_recorder: dependency_recorder.clone(),
            carried_definitions: options.carried_macro_definitions,
        },
    )?;
    Ok(SourceExpansion {
        tokens: expanded.tokens,
        spans: expanded.spans,
        origins: expanded.origins,
        trace: expanded.trace,
        proc_macro_dependencies: expanded.proc_macro_dependencies,
        macro_definitions: expanded.definitions,
    })
}

fn enrich_parse_error_with_macro_origins(error: ParseError, expansion: &SourceExpansion) -> ParseError {
    if error.message.contains("Macro origin stack:") {
        return error;
    }
    let Some(origin) = error
        .span
        .as_ref()
        .and_then(|span| origin_for_span(&expansion.origins, span))
    else {
        return error;
    };
    if origin.frames.is_empty() {
        return error;
    }

    let mut message = error.message;
    message.push('\n');
    message.push_str(&format_macro_origin_stack(origin));
    match error.span {
        Some(span) => ParseError::with_span(message, span),
        None => ParseError::new(message),
    }
}

pub fn macro_origin_note_for_span(origins: &[MacroTokenOrigin], span: &crate::token::Span) -> Option<String> {
    let origin = origin_for_span(origins, span)?;
    (!origin.frames.is_empty()).then(|| format_macro_origin_stack(origin))
}

pub fn type_error_span(
    err: &anyhow::Error,
    tokens: &[Token],
    spans: &[crate::token::Span],
) -> Option<crate::token::Span> {
    typed_error_span(err.downcast_ref::<typ::TypeError>()?, tokens, spans)
}

/// The span of a type error that has already been unwrapped from `anyhow`.
///
/// A tool that caches type errors cannot keep the `anyhow::Error` — it is not
/// `Clone` — but `TypeError` is, and this is all the span lookup ever needed.
pub fn typed_error_span(
    type_error: &typ::TypeError,
    tokens: &[Token],
    spans: &[crate::token::Span],
) -> Option<crate::token::Span> {
    span_for_expr(type_error.expr.as_ref()?, tokens, spans)
}

fn format_macro_origin_stack(origin: &MacroTokenOrigin) -> String {
    let mut message = String::from("Macro origin stack:");
    for frame in origin.frames.iter().rev() {
        message.push_str(&format!(
            "\n  token `{}` from {} of `{}` at {}",
            origin.lexeme,
            frame.kind.as_str(),
            frame.macro_name,
            frame.call_span
        ));
    }
    message
}

fn span_for_expr(expr: &Expr, tokens: &[Token], spans: &[crate::token::Span]) -> Option<crate::token::Span> {
    match expr {
        Expr::Var(name) => find_token_span(tokens, spans, |token| matches!(token, Token::Id(id) if id == name)),
        Expr::Literal(value) => span_for_literal(value, tokens, spans),
        Expr::Paren(inner) => span_for_expr(inner, tokens, spans),
        Expr::Call(name, _) => find_token_span(tokens, spans, |token| matches!(token, Token::Id(id) if id == name)),
        Expr::CallExpr(callee, _) | Expr::CallNamed(callee, _, _) => span_for_expr(callee, tokens, spans),
        Expr::Bin(left, _, right) => span_for_expr(left, tokens, spans).or_else(|| span_for_expr(right, tokens, spans)),
        _ => None,
    }
}

fn span_for_literal(value: &LiteralVal, tokens: &[Token], spans: &[crate::token::Span]) -> Option<crate::token::Span> {
    match value {
        value if value.as_str().is_some() => find_token_span(
            tokens,
            spans,
            |token| matches!(token, Token::Str(lit) if Some(lit.as_str()) == value.as_str()),
        ),
        LiteralVal::Int(expected) => find_token_span(tokens, spans, |token| {
            matches!(token, Token::Int(actual) if actual == expected)
                    // The bit-pattern spelling of the same carrier: the AST kept
                    // the `i64`, so this is the token it came from.
                    || matches!(token, Token::UInt { value, .. } if *value as i64 == *expected)
        }),
        LiteralVal::Float(expected) => find_token_span(
            tokens,
            spans,
            |token| matches!(token, Token::Float(actual) if crate::compat::float::abs(*actual - *expected) < f64::EPSILON),
        ),
        LiteralVal::Bool(expected) => find_token_span(
            tokens,
            spans,
            |token| matches!(token, Token::Bool(actual) if actual == expected),
        ),
        LiteralVal::Nil => find_token_span(tokens, spans, |token| matches!(token, Token::Nil)),
        _ => None,
    }
}

fn find_token_span<F>(tokens: &[Token], spans: &[crate::token::Span], predicate: F) -> Option<crate::token::Span>
where
    F: Fn(&Token) -> bool,
{
    tokens.iter().enumerate().find_map(|(index, token)| {
        if predicate(token) {
            spans.get(index).cloned()
        } else {
            None
        }
    })
}

fn origin_for_span<'a>(origins: &'a [MacroTokenOrigin], span: &crate::token::Span) -> Option<&'a MacroTokenOrigin> {
    origins.iter().find(|origin| origin.span == *span).or_else(|| {
        origins
            .iter()
            .find(|origin| origin.span.start.offset == span.start.offset)
    })
}

/// Render a token stream back to source, one statement per line.
///
/// The line breaks are the point. `lk macro expand` exists to be *read* — it is
/// the debugging tool `docs/macros.md` points at — and it used to answer with
/// the whole program on a single line: a 78-line example came back as one
/// 832-character line. Nothing downstream could help either, because `lk fmt`
/// is a line re-indenter and there was one line.
///
/// Two breaks, both exact rather than guessed:
///
/// - after `;`, which ends a statement in this language and nothing else;
/// - after a `}` whose *next* token starts a declaration. A `}` alone is not a
///   break — `let m = {"a": 1};` would gain one before its own semicolon — so
///   the following token decides.
pub fn render_tokens(tokens: &[Token]) -> String {
    let mut output = String::new();
    let mut prev: Option<&Token> = None;
    for (index, token) in tokens.iter().enumerate() {
        let lexeme = token_lexeme(token);
        if output.ends_with('\n') {
            // A fresh line owns its indentation; `lk fmt` supplies the rest.
        } else if breaks_line_after(prev, token, tokens.get(index + 1)) {
            output.push('\n');
        } else if should_insert_space(prev, token) {
            output.push(' ');
        }
        output.push_str(&lexeme);
        prev = Some(token);
    }
    output
}

/// Does a line end *before* `token`?
fn breaks_line_after(prev: Option<&Token>, token: &Token, _next: Option<&Token>) -> bool {
    match prev {
        Some(Token::Semicolon) => true,
        Some(Token::RBrace) => starts_declaration(token),
        _ => false,
    }
}

/// Tokens that can only begin a new top-level item, so a `}` before one is the
/// end of the previous item rather than part of an expression.
fn starts_declaration(token: &Token) -> bool {
    matches!(
        token,
        Token::Fn | Token::Let | Token::Struct | Token::Impl | Token::Trait | Token::Use | Token::Hash
    )
}

pub fn render_program(program: &Program) -> String {
    let mut output = String::new();
    for stmt in &program.statements {
        output.push_str(&stmt.to_string());
        if !output.ends_with('\n') {
            output.push('\n');
        }
    }
    output.trim_end().to_string()
}

pub fn tokenize_and_expand(
    source: &str,
    options: ParseOptions,
) -> Result<(Vec<crate::token::Token>, Vec<crate::token::Span>), ParseError> {
    let expanded = expand_source(source, options)?;
    Ok((expanded.tokens, expanded.spans))
}

fn should_insert_space(prev: Option<&Token>, current: &Token) -> bool {
    let Some(prev) = prev else {
        return false;
    };
    if matches!(
        current,
        Token::RParen
            | Token::RBrace
            | Token::RBracket
            | Token::Comma
            | Token::Semicolon
            | Token::Dot
            | Token::ColonColon
    ) {
        return false;
    }
    if matches!(
        prev,
        Token::LParen | Token::LBrace | Token::LBracket | Token::Dot | Token::ColonColon
    ) {
        return false;
    }
    true
}

#[cfg(test)]
mod render_test {
    #[cfg(not(feature = "std"))]
    use crate::compat::prelude::*;

    use super::{ParseOptions, render_tokens, tokenize_and_expand};

    /// The expansion comes back one statement per line.
    ///
    /// `lk macro expand` is the tool `docs/macros.md` points at for reading
    /// what a macro produced, and it used to answer with the whole program on
    /// one line — a 78-line example came back as a single 832-character line.
    /// Nothing downstream could help either: `lk fmt` is a line re-indenter,
    /// and there was one line.
    #[test]
    fn rendered_tokens_are_one_statement_per_line() {
        let source = "fn a() -> Int { return 1; }\nfn b() -> Int { return a() + 1; }\nlet c = b();\n";
        let (tokens, _) = tokenize_and_expand(source, ParseOptions::default()).expect("expand");
        assert_eq!(
            render_tokens(&tokens).lines().collect::<Vec<_>>(),
            vec![
                "fn a () -> Int {return 1;",
                "}",
                "fn b () -> Int {return a () + 1;",
                "}",
                "let c = b ();"
            ]
        );
    }

    /// A `}` that closes a map literal is not the end of a statement.
    ///
    /// Breaking on every `}` would put the `;` of `let m = {"a": 1};` on a line
    /// of its own, which is why the *next* token decides.
    #[test]
    fn a_map_literals_brace_does_not_end_a_line() {
        let source = "let m = {\"a\": 1};\nlet n = 2;\n";
        let (tokens, _) = tokenize_and_expand(source, ParseOptions::default()).expect("expand");
        assert_eq!(
            render_tokens(&tokens).lines().collect::<Vec<_>>(),
            vec!["let m = {\"a\" : 1};", "let n = 2;"]
        );
    }
}
