use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::token::{ParseError, Span, Token};

mod expansion;
mod follow;
mod imports;
mod origin;
mod proc_deps;
mod proc_function;
mod proc_output;
mod procedural;

pub use origin::{MacroOriginFrame, MacroOriginKind, MacroTokenOrigin};
pub use proc_deps::ProcMacroDependencyRecorder;
pub use procedural::{
    PROC_MACRO_PROTOCOL_VERSION, ProcMacroDependency, ProcMacroDiagnostic, ProcMacroDiagnosticLevel, ProcMacroKind,
    ProcMacroOptions, ProcMacroProcessConfig, ProcMacroProcessError, ProcMacroProviders, ProcMacroRequest,
    ProcMacroResponse, ProcMacroSpan, ProcMacroToken, expand_ast_macros, run_proc_macro_process,
};

const DEFAULT_RECURSION_LIMIT: usize = 128;

#[derive(Debug, Clone)]
pub struct MacroExpandOptions {
    pub recursion_limit: usize,
    pub trace: bool,
    pub base_dir: Option<PathBuf>,
    pub proc_macro_providers: ProcMacroProviders,
    pub proc_macro_features: Vec<String>,
    pub proc_macro_dependency_recorder: ProcMacroDependencyRecorder,
}

impl Default for MacroExpandOptions {
    fn default() -> Self {
        Self {
            recursion_limit: DEFAULT_RECURSION_LIMIT,
            trace: false,
            base_dir: None,
            proc_macro_providers: ProcMacroProviders::default(),
            proc_macro_features: Vec::new(),
            proc_macro_dependency_recorder: ProcMacroDependencyRecorder::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Delimiter {
    Paren,
    Brace,
    Bracket,
}

#[derive(Debug, Clone)]
pub struct SourceToken {
    pub token: Token,
    pub span: Span,
    pub lexeme: String,
    pub origins: Vec<MacroOriginFrame>,
}

#[derive(Debug, Clone)]
pub enum TokenTree {
    Token(SourceToken),
    Group {
        delimiter: Delimiter,
        span: Span,
        tokens: Vec<TokenTree>,
    },
}

#[derive(Debug, Clone)]
pub struct MacroTrace {
    pub macro_name: String,
    pub call_span: Span,
    pub output_len: usize,
}

#[derive(Debug, Clone)]
pub struct MacroExpandResult {
    pub tokens: Vec<Token>,
    pub spans: Vec<Span>,
    pub origins: Vec<MacroTokenOrigin>,
    pub trace: Vec<MacroTrace>,
    pub proc_macro_dependencies: Vec<ProcMacroDependency>,
}

#[derive(Debug, Clone)]
struct MacroCallFrame {
    macro_name: String,
    call_span: Span,
}

#[derive(Debug, Clone)]
struct MacroDef {
    name: String,
    crate_anchor: Option<String>,
    rules: Vec<MacroRule>,
}

#[derive(Debug, Clone)]
struct MacroRule {
    matcher: Vec<PatternElem>,
    template: Vec<TemplateElem>,
}

#[derive(Debug, Clone)]
pub(in crate::macro_system) struct MacroExportItem {
    name: String,
    alias: String,
    span_index: usize,
}

#[derive(Debug, Clone)]
enum PatternElem {
    Token(Token),
    MetaVar {
        name: String,
        kind: FragmentKind,
        span_index: usize,
    },
    Repeat {
        elems: Vec<PatternElem>,
        separator: Option<Token>,
        op: RepeatOp,
        span_index: usize,
    },
}

#[derive(Debug, Clone)]
enum TemplateElem {
    Token(SourceToken),
    MetaVar(String),
    CrateAnchor(SourceToken),
    Repeat {
        elems: Vec<TemplateElem>,
        separator: Option<SourceToken>,
        op: RepeatOp,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RepeatOp {
    ZeroOrMore,
    OneOrMore,
    Optional,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FragmentKind {
    Expr,
    Stmt,
    Block,
    Item,
    Ident,
    Literal,
    Tt,
    Pat,
    Ty,
    Path,
}

#[derive(Debug, Clone)]
struct Capture {
    alternatives: Vec<Vec<SourceToken>>,
}

impl Capture {
    fn single(tokens: Vec<SourceToken>) -> Self {
        Self {
            alternatives: vec![tokens],
        }
    }

    fn repeated(alternatives: Vec<Vec<SourceToken>>) -> Self {
        Self { alternatives }
    }
}

#[derive(Debug, Clone)]
struct ExpandedToken {
    token: SourceToken,
    from_capture: bool,
    origin_kind: MacroOriginKind,
}

#[derive(Default)]
struct MacroRegistry {
    macros: HashMap<String, MacroDef>,
}

impl MacroRegistry {
    fn contains_macro(&self, name: &str) -> bool {
        self.macros.contains_key(name)
    }

    fn contains_macro_namespace(&self, alias: &str) -> bool {
        let prefix = format!("{alias}::");
        self.macros.keys().any(|name| name.starts_with(&prefix))
    }

    fn insert_macro(
        &mut self,
        name: String,
        mut definition: MacroDef,
        tokens: &[SourceToken],
        index: usize,
    ) -> Result<(), ParseError> {
        if self.macros.contains_key(&name) {
            return Err(error_at(
                tokens,
                index,
                &format!("Macro `{name}` is already defined in this macro scope"),
            ));
        }
        definition.name = name.clone();
        self.macros.insert(name, definition);
        Ok(())
    }

    fn insert_macro_if_absent(&mut self, name: String, mut definition: MacroDef) {
        if self.macros.contains_key(&name) {
            return;
        }
        definition.name = name.clone();
        self.macros.insert(name, definition);
    }
}

pub fn expand_macros(
    tokens: Vec<Token>,
    spans: Vec<Span>,
    options: MacroExpandOptions,
) -> Result<MacroExpandResult, ParseError> {
    let source_tokens = tokens
        .into_iter()
        .zip(spans)
        .map(|(token, span)| {
            let lexeme = token_lexeme(&token);
            SourceToken {
                token,
                span,
                lexeme,
                origins: Vec::new(),
            }
        })
        .collect::<Vec<_>>();
    let (without_defs, registry) = collect_macro_defs(&source_tokens, options.base_dir.as_deref())?;
    let mut trace = Vec::new();
    let mut stack = Vec::new();
    let expanded = expand_stream(&without_defs, &registry, &options, 0, &mut trace, &mut stack)?;
    let (tokens, spans, origins) = split_source_tokens(expanded);
    Ok(MacroExpandResult {
        tokens,
        spans,
        origins,
        trace,
        proc_macro_dependencies: options.proc_macro_dependency_recorder.dependencies(),
    })
}

pub fn token_lexeme(token: &Token) -> String {
    match token {
        Token::LParen => "(".to_string(),
        Token::RParen => ")".to_string(),
        Token::LBrace => "{".to_string(),
        Token::RBrace => "}".to_string(),
        Token::LBracket => "[".to_string(),
        Token::RBracket => "]".to_string(),
        Token::Dot => ".".to_string(),
        Token::ColonColon => "::".to_string(),
        Token::OptionalDot => "?.".to_string(),
        Token::Colon => ":".to_string(),
        Token::Comma => ",".to_string(),
        Token::Semicolon => ";".to_string(),
        Token::Dollar => "$".to_string(),
        Token::Hash => "#".to_string(),
        Token::Assign => "=".to_string(),
        Token::AddAssign => "+=".to_string(),
        Token::SubAssign => "-=".to_string(),
        Token::MulAssign => "*=".to_string(),
        Token::DivAssign => "/=".to_string(),
        Token::ModAssign => "%=".to_string(),
        Token::Nil => "nil".to_string(),
        Token::Eq => "==".to_string(),
        Token::Ne => "!=".to_string(),
        Token::Gt => ">".to_string(),
        Token::Lt => "<".to_string(),
        Token::Ge => ">=".to_string(),
        Token::Le => "<=".to_string(),
        Token::In => "in".to_string(),
        Token::And => "&&".to_string(),
        Token::Or => "||".to_string(),
        Token::BitAnd => "&".to_string(),
        Token::BitNot => "~".to_string(),
        Token::Not => "!".to_string(),
        Token::Add => "+".to_string(),
        Token::Sub => "-".to_string(),
        Token::Mul => "*".to_string(),
        Token::Div => "/".to_string(),
        Token::Mod => "%".to_string(),
        Token::Arrow => "=>".to_string(),
        Token::LeftArrow => "<-".to_string(),
        Token::NullishCoalescing => "??".to_string(),
        Token::Range => "..".to_string(),
        Token::RangeInclusive => "..=".to_string(),
        Token::If => "if".to_string(),
        Token::Else => "else".to_string(),
        Token::While => "while".to_string(),
        Token::Let => "let".to_string(),
        Token::Const => "const".to_string(),
        Token::Break => "break".to_string(),
        Token::Continue => "continue".to_string(),
        Token::Return => "return".to_string(),
        Token::Fn => "fn".to_string(),
        Token::For => "for".to_string(),
        Token::Match => "match".to_string(),
        Token::Case => "case".to_string(),
        Token::Default => "default".to_string(),
        Token::Select => "select".to_string(),
        Token::Use => "use".to_string(),
        Token::From => "from".to_string(),
        Token::As => "as".to_string(),
        Token::Type => "type".to_string(),
        Token::Struct => "struct".to_string(),
        Token::Trait => "trait".to_string(),
        Token::Impl => "impl".to_string(),
        Token::Pipe => "|".to_string(),
        Token::Question => "?".to_string(),
        Token::FnArrow => "->".to_string(),
        Token::Str(value) => format!("\"{}\"", value.escape_default()),
        Token::TemplateString(value) => format!("\"{}\"", value.escape_default()),
        Token::Int(value) => value.to_string(),
        Token::Float(value) => value.to_string(),
        Token::Bool(value) => value.to_string(),
        Token::Id(value) => value.clone(),
    }
}

pub fn is_builtin_macro_module(name: &str) -> bool {
    imports::is_builtin_macro_module(name)
}

fn collect_macro_defs(
    tokens: &[SourceToken],
    base_dir: Option<&Path>,
) -> Result<(Vec<SourceToken>, MacroRegistry), ParseError> {
    let mut registry = MacroRegistry::default();
    let mut loading = Vec::new();
    imports::collect_imported_macro_defs(tokens, base_dir, &mut registry, &mut loading)?;
    let mut skipped_ranges = Vec::new();
    let mut export_items = Vec::new();
    let mut local_names = Vec::new();
    let mut index = 0usize;
    let crate_anchor = imports::local_macro_crate_anchor();
    while index < tokens.len() {
        if let Some((macro_start, _exported)) = macro_rules_start_at(tokens, index) {
            let (mut definition, next) = parse_macro_def(tokens, macro_start)?;
            definition.crate_anchor = Some(crate_anchor.clone());
            local_names.push(definition.name.clone());
            registry.insert_macro(definition.name.clone(), definition, tokens, index)?;
            skipped_ranges.push((index, next));
            index = next;
        } else if let Some((items, next)) = parse_macro_export_list_at(tokens, index)? {
            export_items.extend(items);
            skipped_ranges.push((index, next));
            index = next;
        } else {
            index += 1;
        }
    }
    validate_macro_export_items(&registry, &export_items, tokens)?;
    register_local_macro_crate_anchor(&mut registry, &local_names, &crate_anchor);

    let mut output = Vec::with_capacity(tokens.len());
    let mut skip_index = 0usize;
    index = 0;
    while index < tokens.len() {
        if skip_index < skipped_ranges.len() && index == skipped_ranges[skip_index].0 {
            index = skipped_ranges[skip_index].1;
            skip_index += 1;
        } else if let Some(next) = imports::compile_time_macro_import_end_at(tokens, index, &registry)? {
            index = next;
        } else {
            output.push(tokens[index].clone());
            index += 1;
        }
    }
    Ok((output, registry))
}

fn register_local_macro_crate_anchor(registry: &mut MacroRegistry, local_names: &[String], crate_anchor: &str) {
    let anchored = local_names
        .iter()
        .filter_map(|name| {
            registry
                .macros
                .get(name)
                .cloned()
                .map(|definition| (format!("{crate_anchor}::{name}"), definition))
        })
        .collect::<Vec<_>>();
    for (name, definition) in anchored {
        registry.insert_macro_if_absent(name, definition);
    }
}

pub(in crate::macro_system) fn macro_rules_start_at(tokens: &[SourceToken], index: usize) -> Option<(usize, bool)> {
    if is_macro_rules_at(tokens, index) {
        return Some((index, false));
    }
    if is_contextual_id(tokens, index, "export") && is_macro_rules_at(tokens, index + 1) {
        return Some((index + 1, true));
    }
    None
}

pub(in crate::macro_system) fn is_macro_rules_at(tokens: &[SourceToken], index: usize) -> bool {
    matches!(tokens.get(index).map(|t| &t.token), Some(Token::Id(name)) if name == "macro_rules")
        && matches!(tokens.get(index + 1).map(|t| &t.token), Some(Token::Not))
        && matches!(tokens.get(index + 2).map(|t| &t.token), Some(Token::Id(_)))
        && matches!(tokens.get(index + 3).map(|t| &t.token), Some(Token::LBrace))
}

pub(in crate::macro_system) fn parse_macro_export_list_at(
    tokens: &[SourceToken],
    index: usize,
) -> Result<Option<(Vec<MacroExportItem>, usize)>, ParseError> {
    if !is_contextual_id(tokens, index, "export")
        || !matches!(tokens.get(index + 1).map(|token| &token.token), Some(Token::LBrace))
    {
        return Ok(None);
    }
    let (_, end) = find_group(tokens, index + 1)?;
    let items = parse_macro_export_items(tokens, index + 2, end)?;
    if items.is_empty() {
        return Err(error_at(
            tokens,
            index + 1,
            "Expected at least one macro name in macro export list",
        ));
    }
    let next = if matches!(tokens.get(end + 1).map(|token| &token.token), Some(Token::Semicolon)) {
        end + 2
    } else {
        end + 1
    };
    Ok(Some((items, next)))
}

fn parse_macro_export_items(
    tokens: &[SourceToken],
    start: usize,
    end: usize,
) -> Result<Vec<MacroExportItem>, ParseError> {
    let mut items = Vec::new();
    let mut index = start;
    while index < end {
        if matches!(tokens[index].token, Token::Comma) {
            index += 1;
            continue;
        }
        let span_index = index;
        let name = expect_id(tokens, index, "Expected macro name in macro export list")?;
        let mut alias = name.clone();
        index += 1;
        if matches!(tokens.get(index).map(|token| &token.token), Some(Token::As)) {
            alias = expect_id(tokens, index + 1, "Expected macro export alias after `as`")?;
            index += 2;
        }
        items.push(MacroExportItem {
            name,
            alias,
            span_index,
        });
        if index < end {
            if matches!(tokens[index].token, Token::Comma) {
                index += 1;
            } else {
                return Err(error_at(tokens, index, "Expected `,` between macro export names"));
            }
        }
    }
    Ok(items)
}

fn validate_macro_export_items(
    registry: &MacroRegistry,
    items: &[MacroExportItem],
    tokens: &[SourceToken],
) -> Result<(), ParseError> {
    let mut names = HashMap::<String, usize>::new();
    for item in items {
        if !registry.contains_macro(&item.name) {
            return Err(error_at(
                tokens,
                item.span_index,
                &format!("Cannot export unknown macro `{}`", item.name),
            ));
        }
        if names.insert(item.alias.clone(), item.span_index).is_some() {
            return Err(error_at(
                tokens,
                item.span_index,
                &format!("Macro `{}` is already exported from this macro module", item.alias),
            ));
        }
    }
    Ok(())
}

fn is_contextual_id(tokens: &[SourceToken], index: usize, expected: &str) -> bool {
    matches!(tokens.get(index).map(|token| &token.token), Some(Token::Id(name)) if name == expected)
}

pub(in crate::macro_system) fn parse_macro_def(
    tokens: &[SourceToken],
    start: usize,
) -> Result<(MacroDef, usize), ParseError> {
    let name = match &tokens[start + 2].token {
        Token::Id(name) => name.clone(),
        _ => unreachable!(),
    };
    let (body_start, body_end) = find_group(tokens, start + 3)?;
    let mut rules = Vec::new();
    let mut index = body_start + 1;
    while index < body_end {
        while index < body_end && matches!(tokens[index].token, Token::Semicolon | Token::Comma) {
            index += 1;
        }
        if index >= body_end {
            break;
        }
        let (matcher_start, matcher_end) = find_group(tokens, index)?;
        if tokens.get(matcher_end + 1).map(|t| &t.token) != Some(&Token::Arrow) {
            return Err(error_at(tokens, matcher_end, "Expected `=>` in macro_rules rule"));
        }
        let template_group = matcher_end + 2;
        let (template_start, template_end) = find_group(tokens, template_group)?;
        let matcher = parse_pattern_elems(&tokens[matcher_start + 1..matcher_end])?;
        let template = parse_template_elems(&tokens[template_start + 1..template_end])?;
        rules.push(MacroRule { matcher, template });
        index = template_end + 1;
        if index < body_end && matches!(tokens[index].token, Token::Semicolon | Token::Comma) {
            index += 1;
        }
    }
    if rules.is_empty() {
        return Err(error_at(
            tokens,
            start,
            "macro_rules definition must contain at least one rule",
        ));
    }
    Ok((
        MacroDef {
            name,
            crate_anchor: None,
            rules,
        },
        body_end + 1,
    ))
}

fn parse_pattern_elems(tokens: &[SourceToken]) -> Result<Vec<PatternElem>, ParseError> {
    let mut elems = Vec::new();
    let mut index = 0usize;
    while index < tokens.len() {
        if matches!(tokens[index].token, Token::Dollar) {
            if matches!(tokens.get(index + 1).map(|t| &t.token), Some(Token::LParen)) {
                let (start, end) = find_group(tokens, index + 1)?;
                let inner = parse_pattern_elems(&tokens[start + 1..end])?;
                if pattern_matches_empty(&inner) {
                    return Err(error_at(
                        tokens,
                        index,
                        "Macro repetition pattern must consume at least one token",
                    ));
                }
                let (separator, op, next) = parse_repeat_tail(tokens, end + 1)?;
                elems.push(PatternElem::Repeat {
                    elems: inner,
                    separator: separator.map(|token| token.token),
                    op,
                    span_index: index,
                });
                index = next;
                continue;
            }
            let name = expect_id(tokens, index + 1, "Expected metavariable name after `$`")?;
            if tokens.get(index + 2).map(|t| &t.token) != Some(&Token::Colon) {
                return Err(error_at(
                    tokens,
                    index + 2,
                    "Expected `:` after macro metavariable name",
                ));
            }
            let kind_name = expect_id(tokens, index + 3, "Expected fragment kind after `:`")?;
            let kind = parse_fragment_kind(&kind_name).ok_or_else(|| {
                error_at(
                    tokens,
                    index + 3,
                    "Unsupported macro fragment kind; expected expr, stmt, block, item, ident, literal, tt, pat, ty, or path",
                )
            })?;
            elems.push(PatternElem::MetaVar {
                name,
                kind,
                span_index: index,
            });
            index += 4;
        } else {
            elems.push(PatternElem::Token(tokens[index].token.clone()));
            index += 1;
        }
    }
    follow::validate_pattern_follow_sets(&elems, tokens)?;
    Ok(elems)
}

fn parse_template_elems(tokens: &[SourceToken]) -> Result<Vec<TemplateElem>, ParseError> {
    let mut elems = Vec::new();
    let mut index = 0usize;
    while index < tokens.len() {
        if matches!(tokens[index].token, Token::Dollar) {
            if matches!(tokens.get(index + 1).map(|t| &t.token), Some(Token::Id(name)) if name == "crate") {
                elems.push(TemplateElem::CrateAnchor(tokens[index + 1].clone()));
                index += 2;
                continue;
            }
            if matches!(tokens.get(index + 1).map(|t| &t.token), Some(Token::LParen)) {
                let (start, end) = find_group(tokens, index + 1)?;
                let inner = parse_template_elems(&tokens[start + 1..end])?;
                let (separator, op, next) = parse_repeat_tail(tokens, end + 1)?;
                elems.push(TemplateElem::Repeat {
                    elems: inner,
                    separator,
                    op,
                });
                index = next;
                continue;
            }
            let name = expect_id(tokens, index + 1, "Expected metavariable name after `$`")?;
            elems.push(TemplateElem::MetaVar(name));
            index += 2;
        } else {
            elems.push(TemplateElem::Token(tokens[index].clone()));
            index += 1;
        }
    }
    Ok(elems)
}

fn parse_repeat_tail(
    tokens: &[SourceToken],
    mut index: usize,
) -> Result<(Option<SourceToken>, RepeatOp, usize), ParseError> {
    let mut separator = None;
    if let Some(token) = tokens.get(index)
        && !matches!(token.token, Token::Mul | Token::Add | Token::Question)
    {
        if !is_valid_repeat_separator(&token.token) {
            return Err(error_at(
                tokens,
                index,
                "Invalid macro repetition separator; delimiters cannot be used as separators",
            ));
        }
        separator = Some(token.clone());
        index += 1;
    }
    let op = match tokens.get(index).map(|t| &t.token) {
        Some(Token::Mul) => RepeatOp::ZeroOrMore,
        Some(Token::Add) => RepeatOp::OneOrMore,
        Some(Token::Question) => RepeatOp::Optional,
        _ => {
            return Err(error_at(
                tokens,
                index,
                "Expected macro repetition operator `*`, `+`, or `?`",
            ));
        }
    };
    if op == RepeatOp::Optional && separator.is_some() {
        return Err(error_at(
            tokens,
            index,
            "Optional macro repetition `?` cannot use a separator",
        ));
    }
    Ok((separator, op, index + 1))
}

fn pattern_matches_empty(pattern: &[PatternElem]) -> bool {
    pattern.iter().all(pattern_elem_matches_empty)
}

fn pattern_elem_matches_empty(elem: &PatternElem) -> bool {
    match elem {
        PatternElem::Token(_) | PatternElem::MetaVar { .. } => false,
        PatternElem::Repeat { elems, op, .. } => *op != RepeatOp::OneOrMore || pattern_matches_empty(elems),
    }
}

fn is_valid_repeat_separator(token: &Token) -> bool {
    !matches!(
        token,
        Token::LParen
            | Token::RParen
            | Token::LBrace
            | Token::RBrace
            | Token::LBracket
            | Token::RBracket
            | Token::Mul
            | Token::Add
            | Token::Question
    )
}

fn parse_fragment_kind(name: &str) -> Option<FragmentKind> {
    match name {
        "expr" => Some(FragmentKind::Expr),
        "stmt" => Some(FragmentKind::Stmt),
        "block" => Some(FragmentKind::Block),
        "item" => Some(FragmentKind::Item),
        "ident" => Some(FragmentKind::Ident),
        "literal" => Some(FragmentKind::Literal),
        "tt" => Some(FragmentKind::Tt),
        "pat" => Some(FragmentKind::Pat),
        "ty" => Some(FragmentKind::Ty),
        "path" => Some(FragmentKind::Path),
        _ => None,
    }
}

fn expand_stream(
    tokens: &[SourceToken],
    registry: &MacroRegistry,
    options: &MacroExpandOptions,
    depth: usize,
    trace: &mut Vec<MacroTrace>,
    stack: &mut Vec<MacroCallFrame>,
) -> Result<Vec<SourceToken>, ParseError> {
    if depth > options.recursion_limit {
        return Err(macro_error_with_stack(
            error_at(tokens, 0, "Macro expansion recursion limit exceeded"),
            stack,
        ));
    }

    let mut output = Vec::with_capacity(tokens.len());
    let mut index = 0usize;
    while index < tokens.len() {
        let Some((name, group_start)) = macro_invocation_at(tokens, index, registry, options) else {
            output.push(tokens[index].clone());
            index += 1;
            continue;
        };
        let (inner_start, inner_end) = find_group(tokens, group_start)?;
        let input = &tokens[inner_start + 1..inner_end];
        let call_span = tokens[index].span.clone();
        let call_origins = tokens[index].origins.clone();
        stack.push(MacroCallFrame {
            macro_name: name.clone(),
            call_span: call_span.clone(),
        });
        let expanded = if let Some(definition) = registry.macros.get(&name) {
            match expansion::expand_macro_invocation(definition, input, &call_span, &call_origins) {
                Ok(expanded) => expanded,
                Err(error) => {
                    let error = macro_error_with_stack(error, stack);
                    stack.pop();
                    return Err(error);
                }
            }
        } else {
            match proc_function::expand_function_like_proc_macro(&name, input, options, &call_span, &call_origins) {
                Ok(expanded) => expanded,
                Err(error) => {
                    let error = macro_error_with_stack(error, stack);
                    stack.pop();
                    return Err(error);
                }
            }
        };
        if options.trace {
            trace.push(MacroTrace {
                macro_name: name,
                call_span,
                output_len: expanded.len(),
            });
        }
        let expanded = match expand_stream(&expanded, registry, options, depth + 1, trace, stack) {
            Ok(expanded) => expanded,
            Err(error) => {
                let error = macro_error_with_stack(error, stack);
                stack.pop();
                return Err(error);
            }
        };
        stack.pop();
        output.extend(expanded);
        index = inner_end + 1;
    }
    Ok(output)
}

fn macro_error_with_stack(error: ParseError, stack: &[MacroCallFrame]) -> ParseError {
    const HEADER: &str = "Macro expansion stack:";
    if stack.is_empty() || error.message.contains(HEADER) {
        return error;
    }
    let mut message = error.message;
    message.push('\n');
    message.push_str(HEADER);
    for frame in stack.iter().rev() {
        message.push_str(&format!(
            "\n  while expanding `{}` at {}",
            frame.macro_name, frame.call_span
        ));
    }
    match error.span {
        Some(span) => ParseError::with_span(message, span),
        None => ParseError::new(message),
    }
}

fn macro_invocation_at(
    tokens: &[SourceToken],
    index: usize,
    registry: &MacroRegistry,
    options: &MacroExpandOptions,
) -> Option<(String, usize)> {
    let Token::Id(name) = &tokens.get(index)?.token else {
        return None;
    };
    let mut macro_name = name.clone();
    let mut cursor = index + 1;
    while tokens.get(cursor).map(|token| &token.token) == Some(&Token::ColonColon) {
        let Some(Token::Id(segment)) = tokens.get(cursor + 1).map(|token| &token.token) else {
            return None;
        };
        macro_name.push_str("::");
        macro_name.push_str(segment);
        cursor += 2;
    }
    let is_macro = registry.macros.contains_key(&macro_name)
        || options
            .proc_macro_providers
            .function_like_provider(&macro_name)
            .is_some();
    if !is_macro || tokens.get(cursor).map(|t| &t.token) != Some(&Token::Not) {
        return None;
    }
    if !is_open_delim(&tokens.get(cursor + 1)?.token) {
        return None;
    }
    Some((macro_name, cursor + 1))
}

fn find_group(tokens: &[SourceToken], open: usize) -> Result<(usize, usize), ParseError> {
    let open_token = tokens
        .get(open)
        .ok_or_else(|| error_at(tokens, open, "Expected macro delimiter"))?;
    let expected_close = match open_token.token {
        Token::LParen => Token::RParen,
        Token::LBrace => Token::RBrace,
        Token::LBracket => Token::RBracket,
        _ => return Err(error_at(tokens, open, "Expected macro delimiter")),
    };
    let mut depth = 0usize;
    for index in open..tokens.len() {
        let token = &tokens[index].token;
        if token_matches(&open_token.token, token) {
            depth += 1;
        } else if token_matches(&expected_close, token) {
            depth -= 1;
            if depth == 0 {
                return Ok((open, index));
            }
        }
    }
    Err(ParseError::with_span(
        "Unclosed macro delimiter".to_string(),
        open_token.span.clone(),
    ))
}

fn is_open_delim(token: &Token) -> bool {
    matches!(token, Token::LParen | Token::LBrace | Token::LBracket)
}

fn token_matches(expected: &Token, actual: &Token) -> bool {
    match (expected, actual) {
        (Token::Id(a), Token::Id(b)) => a == b,
        (Token::Str(a), Token::Str(b)) => a == b,
        (Token::TemplateString(a), Token::TemplateString(b)) => a == b,
        (Token::Int(a), Token::Int(b)) => a == b,
        (Token::Float(a), Token::Float(b)) => a == b,
        (Token::Bool(a), Token::Bool(b)) => a == b,
        _ => std::mem::discriminant(expected) == std::mem::discriminant(actual),
    }
}

fn split_source_tokens(tokens: Vec<SourceToken>) -> (Vec<Token>, Vec<Span>, Vec<MacroTokenOrigin>) {
    let mut out_tokens = Vec::with_capacity(tokens.len());
    let mut spans = Vec::with_capacity(tokens.len());
    let mut origins = Vec::with_capacity(tokens.len());
    for (token_index, token) in tokens.into_iter().enumerate() {
        origins.push(MacroTokenOrigin {
            token_index,
            lexeme: token.lexeme.clone(),
            span: token.span.clone(),
            frames: token.origins.clone(),
        });
        out_tokens.push(token.token);
        spans.push(token.span);
    }
    (out_tokens, spans, origins)
}

fn expect_id(tokens: &[SourceToken], index: usize, message: &str) -> Result<String, ParseError> {
    match tokens.get(index).map(|token| &token.token) {
        Some(Token::Id(name)) => Ok(name.clone()),
        _ => Err(error_at(tokens, index, message)),
    }
}

fn error_at(tokens: &[SourceToken], index: usize, message: &str) -> ParseError {
    let span = tokens
        .get(index)
        .or_else(|| tokens.last())
        .map(|token| token.span.clone());
    match span {
        Some(span) => ParseError::with_span(message.to_string(), span),
        None => ParseError::new(message.to_string()),
    }
}

#[cfg(test)]
mod origin_tests;

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::{
        syntax::{ParseOptions, expand_source, parse_program_source, render_tokens},
        vm::execute_source,
    };

    #[test]
    fn expands_vec_like_repetition() {
        let result = execute_source(
            r#"
            macro_rules! vec {
                ($($value:expr),*) => { [$($value),*] };
            }
            return vec![1, 2 + 3, 4].1;
            "#,
        )
        .expect("macro program should execute");
        assert_eq!(result.display_first_return(), "5");
    }

    #[test]
    fn expands_block_fragment() {
        let result = execute_source(
            r#"
            macro_rules! unless {
                ($cond:expr, $body:block) => { if (!($cond)) $body };
            }
            let value = 0;
            unless!(value == 1, { return 42; });
            return 0;
            "#,
        )
        .expect("macro program should execute");
        assert_eq!(result.display_first_return(), "42");
    }

    #[test]
    fn macro_definition_is_not_a_runtime_statement() {
        let program = parse_program_source(
            r#"
            macro_rules! id {
                ($value:expr) => { $value };
            }
            return id!(7);
            "#,
            Default::default(),
        )
        .expect("program parses");
        assert_eq!(program.statements.len(), 1);
    }

    #[test]
    fn exported_macro_definition_is_not_a_runtime_statement() {
        let program = parse_program_source(
            r#"
            export macro_rules! id {
                ($value:expr) => { $value };
            }
            return id!(7);
            "#,
            Default::default(),
        )
        .expect("program parses");
        assert_eq!(program.statements.len(), 1);
    }

    #[test]
    fn crate_anchor_resolves_same_file_private_helper() {
        let result = execute_source(
            r#"
            macro_rules! helper {
                () => { 40 };
            }
            macro_rules! answer {
                () => { $crate::helper!() + 2 };
            }
            return answer!();
            "#,
        )
        .expect("same-file crate anchor should expand");
        assert_eq!(result.display_first_return(), "42");
    }

    #[test]
    fn expansion_trace_records_macro_calls() {
        let expanded = expand_source(
            r#"
            macro_rules! id {
                ($value:expr) => { $value };
            }
            return id!(7);
            "#,
            ParseOptions {
                macro_trace: true,
                ..ParseOptions::default()
            },
        )
        .expect("macro expansion succeeds");
        assert_eq!(expanded.trace.len(), 1);
        assert_eq!(expanded.trace[0].macro_name, "id");
        assert!(render_tokens(&expanded.tokens).contains("return 7;"));
    }

    #[test]
    fn macro_errors_include_expansion_stack() {
        let err = parse_program_source(
            r#"
            macro_rules! outer {
                () => { inner!() };
            }
            macro_rules! inner {
                ($value:expr) => { $value };
            }
            return outer!();
            "#,
            Default::default(),
        )
        .expect_err("inner macro should fail to match");
        let message = err.to_string();
        assert!(message.contains("No matching rule for macro `inner`"));
        assert!(message.contains("Macro expansion stack:"));
        assert!(message.contains("while expanding `inner`"));
        assert!(message.contains("while expanding `outer`"));
    }

    #[test]
    fn unmatched_macro_reports_rule_mismatch_notes() {
        let err = parse_program_source(
            r#"
            macro_rules! pair {
                ($left:expr, $right:expr) => { $left + $right };
                (1 => $value:expr) => { $value };
            }
            return pair!(1);
            "#,
            Default::default(),
        )
        .expect_err("macro call should report why rules did not match");
        let message = err.to_string();
        assert!(message.contains("No matching rule for macro `pair`"));
        assert!(message.contains("Macro rule mismatch notes:"));
        assert!(message.contains("rule 1: expected `,` at end of input"));
        assert!(message.contains("rule 2: expected `=>` at end of input"));
    }

    #[test]
    fn rejects_zero_width_pattern_repetition() {
        let err = parse_program_source(
            r#"
            macro_rules! empty {
                ($()*) => { 1 };
            }
            return empty!();
            "#,
            Default::default(),
        )
        .expect_err("zero-width repetition should be rejected");
        assert!(err.to_string().contains("must consume at least one token"));
    }

    #[test]
    fn rejects_expr_fragment_with_invalid_follow_token() {
        let err = parse_program_source(
            r#"
            macro_rules! bad {
                ($value:expr +) => { 1 };
            }
            return 0;
            "#,
            Default::default(),
        )
        .expect_err("expr fragment should reject non-follow token");
        let message = err.to_string();
        assert!(message.contains("Macro fragment `$value:expr` cannot be followed by `+`"));
        assert!(message.contains("`=>`, `,`, `;`, or a `block` fragment"));
    }

    #[test]
    fn rejects_unseparated_expr_repetition_following_itself() {
        let err = parse_program_source(
            r#"
            macro_rules! bad {
                ($($value:expr)+) => { 1 };
            }
            return 0;
            "#,
            Default::default(),
        )
        .expect_err("expr repetition without separator should be ambiguous");
        assert!(
            err.to_string()
                .contains("Macro fragment `$value:expr` cannot be followed by `$_:expr`")
        );
    }

    #[test]
    fn rejects_optional_repetition_separator() {
        let err = parse_program_source(
            r#"
            macro_rules! bad {
                ($($value:expr),?) => { 1 };
            }
            return 0;
            "#,
            Default::default(),
        )
        .expect_err("optional repetition cannot use a separator");
        assert!(
            err.to_string()
                .contains("Optional macro repetition `?` cannot use a separator")
        );
    }

    #[test]
    fn rejects_template_repetition_without_metavariable() {
        let err = parse_program_source(
            r#"
            macro_rules! bad {
                ($value:expr) => { $(1),* };
            }
            return bad!(2);
            "#,
            Default::default(),
        )
        .expect_err("template repetition needs a metavariable");
        assert!(err.to_string().contains("requires at least one metavariable"));
    }

    #[test]
    fn rejects_repetition_arity_mismatch() {
        let err = parse_program_source(
            r#"
            macro_rules! zip {
                (($($a:expr),*); ($($b:expr),*)) => { [$($a + $b),*] };
            }
            return zip!((1, 2); (10));
            "#,
            Default::default(),
        )
        .expect_err("repeated metavariables must have matching arity");
        assert!(err.to_string().contains("matched 1 item(s), expected 2"));
    }

    #[test]
    fn expr_fragment_uses_parser_validation_before_tt_fallback() {
        let result = execute_source(
            r#"
            macro_rules! classify {
                ($value:expr) => { 1 };
                ($value:tt) => { 2 };
            }
            return classify!(let);
            "#,
        )
        .expect("tt fallback should handle a non-expression token");
        assert_eq!(result.display_first_return(), "2");
    }

    #[test]
    fn expr_fragment_boundary_is_parser_discovered_before_block() {
        let result = execute_source(
            r#"
            macro_rules! unless {
                ($cond:expr $body:block) => { if (!($cond)) $body };
            }
            let value = 0;
            unless!(value == 1 { return 42; });
            return 0;
            "#,
        )
        .expect("expression fragment should stop before the following block metavariable");
        assert_eq!(result.display_first_return(), "42");
    }

    #[test]
    fn stmt_fragment_uses_parser_validation_before_tt_fallback() {
        let result = execute_source(
            r#"
            macro_rules! classify {
                ($value:stmt) => { 1 };
                ($value:tt) => { 2 };
            }
            return classify!(=);
            "#,
        )
        .expect("tt fallback should handle a non-statement token");
        assert_eq!(result.display_first_return(), "2");
    }

    #[test]
    fn pattern_fragment_uses_parser_validation_before_tt_fallback() {
        let result = execute_source(
            r#"
            macro_rules! classify {
                ($value:pat) => { 1 };
                ($value:tt) => { 2 };
            }
            return classify!(=>);
            "#,
        )
        .expect("tt fallback should handle a non-pattern token");
        assert_eq!(result.display_first_return(), "2");
    }

    #[test]
    fn type_fragment_boundary_is_discovered_before_block() {
        let result = execute_source(
            r#"
            macro_rules! default_value {
                ($ty:ty $body:block) => { { let value: $ty = 42; return value; } };
            }
            default_value!(Int { return 0; });
            return 0;
            "#,
        )
        .expect("type fragment should stop before the following block metavariable");
        assert_eq!(result.display_first_return(), "42");
    }

    #[test]
    fn path_fragment_boundary_is_discovered_before_block() {
        let result = execute_source(
            r#"
            fn answer() {
                return 42;
            }
            macro_rules! call_path {
                ($target:path $body:block) => { $target() };
            }
            return call_path!(answer { return 0; });
            "#,
        )
        .expect("path fragment should stop before the following block metavariable");
        assert_eq!(result.display_first_return(), "42");
    }

    #[test]
    fn imports_named_macro_rules_from_used_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(
            dir.path().join("macros.lk"),
            r#"
            export macro_rules! answer {
                () => { 42 };
            }
            "#,
        )
        .expect("write macro module");
        let program = parse_program_source(
            r#"
            use { answer } from "macros";
            return answer!();
            "#,
            ParseOptions {
                base_dir: Some(dir.path().to_path_buf()),
                ..ParseOptions::default()
            },
        )
        .expect("program should import macro definitions from used file");
        assert_eq!(program.statements.len(), 1);
    }

    #[test]
    fn aliases_named_macro_imports() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(
            dir.path().join("macros.lk"),
            r#"
            export macro_rules! answer {
                () => { 42 };
            }
            "#,
        )
        .expect("write macro module");
        let expanded = expand_source(
            r#"
            use { answer as ans } from "macros";
            return ans!();
            "#,
            ParseOptions {
                base_dir: Some(dir.path().to_path_buf()),
                ..ParseOptions::default()
            },
        )
        .expect("aliased macro import should expand");
        assert!(render_tokens(&expanded.tokens).contains("return 42;"));
    }

    #[test]
    fn file_macro_use_imports_default_namespace() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(
            dir.path().join("macros.lk"),
            r#"
            export macro_rules! answer {
                () => { 42 };
            }
            "#,
        )
        .expect("write macro module");
        let expanded = expand_source(
            r#"
            use "macros";
            return macros::answer!();
            "#,
            ParseOptions {
                base_dir: Some(dir.path().to_path_buf()),
                ..ParseOptions::default()
            },
        )
        .expect("namespaced file macro import should expand");
        assert!(render_tokens(&expanded.tokens).contains("return 42;"));
    }

    #[test]
    fn namespace_macro_import_uses_explicit_alias() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(
            dir.path().join("macros.lk"),
            r#"
            export macro_rules! answer {
                () => { 42 };
            }
            "#,
        )
        .expect("write macro module");
        let expanded = expand_source(
            r#"
            use * as m from "macros";
            return m::answer!();
            "#,
            ParseOptions {
                base_dir: Some(dir.path().to_path_buf()),
                ..ParseOptions::default()
            },
        )
        .expect("aliased namespace macro import should expand");
        assert!(render_tokens(&expanded.tokens).contains("return 42;"));
    }

    #[test]
    fn named_macro_import_does_not_leak_unrequested_macros() {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(
            dir.path().join("macros.lk"),
            r#"
            export macro_rules! answer {
                () => { 42 };
            }
            macro_rules! hidden {
                () => { 99 };
            }
            "#,
        )
        .expect("write macro module");
        let expanded = expand_source(
            r#"
            use { answer } from "macros";
            return hidden!();
            "#,
            ParseOptions {
                base_dir: Some(dir.path().to_path_buf()),
                ..ParseOptions::default()
            },
        )
        .expect("macro expansion leaves unknown macros for the parser");
        assert!(render_tokens(&expanded.tokens).contains("hidden !"));
        assert!(
            parse_program_source(
                r#"
                use { answer } from "macros";
                return hidden!();
                "#,
                ParseOptions {
                    base_dir: Some(dir.path().to_path_buf()),
                    ..ParseOptions::default()
                },
            )
            .is_err()
        );
    }
}
