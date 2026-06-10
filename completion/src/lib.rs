use lk_core::token::{Token, Tokenizer};
use lk_stdlib::{StdlibExportKind, StdlibExportSpec, stdlib_catalog};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::Path,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CompletionKind {
    Keyword,
    Operator,
    Type,
    Function,
    Module,
    Method,
    Field,
    Variable,
    Value,
    File,
    Folder,
    Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionMode {
    Lsp,
    Repl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionTrigger {
    Invoked,
    TriggerCharacter(char),
    Incomplete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionCandidate {
    pub label: String,
    pub replacement: String,
    pub detail: Option<String>,
    pub kind: CompletionKind,
    pub replace_start: usize,
    pub replace_end: usize,
}

impl CompletionCandidate {
    fn new(
        label: impl Into<String>,
        kind: CompletionKind,
        detail: impl Into<Option<String>>,
        replacement: impl Into<String>,
        replace_start: usize,
        replace_end: usize,
    ) -> Self {
        Self {
            label: label.into(),
            replacement: replacement.into(),
            detail: detail.into(),
            kind,
            replace_start,
            replace_end,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionResult {
    pub candidates: Vec<CompletionCandidate>,
    pub is_incomplete: bool,
}

impl CompletionResult {
    fn complete(candidates: Vec<CompletionCandidate>) -> Self {
        Self {
            candidates,
            is_incomplete: false,
        }
    }

    fn incomplete(candidates: Vec<CompletionCandidate>) -> Self {
        Self {
            candidates,
            is_incomplete: true,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CompletionRequest<'a> {
    pub source: &'a str,
    pub cursor: usize,
    pub mode: CompletionMode,
    pub trigger: CompletionTrigger,
    pub session_source: Option<&'a str>,
    pub base_dir: Option<&'a Path>,
}

#[derive(Debug)]
pub struct CompletionEngine;

impl CompletionEngine {
    pub fn new() -> anyhow::Result<Self> {
        let _ = stdlib_catalog();
        Ok(Self)
    }

    pub fn fallback() -> Self {
        Self
    }

    pub fn complete(&self, request: CompletionRequest<'_>) -> Vec<CompletionCandidate> {
        self.complete_with_metadata(request).candidates
    }

    pub fn complete_with_metadata(&self, request: CompletionRequest<'_>) -> CompletionResult {
        let cursor = request.cursor.min(request.source.len());
        let ctx = CompletionContext::new(request.source, cursor);
        let symbol_source = merged_symbol_source(request.source, request.session_source);
        let symbols = SymbolIndex::from_source(&symbol_source);

        let mut out = Vec::new();
        if request.mode == CompletionMode::Repl && ctx.line_prefix.trim_start().starts_with(':') {
            self.push_repl_commands(&mut out, &ctx);
            return CompletionResult::complete(dedup_sort(out));
        }
        if self.push_import_path(&mut out, &ctx, request.base_dir) {
            return CompletionResult::complete(dedup_sort(out));
        }
        if self.push_brace_import_exports(&mut out, &ctx) {
            return CompletionResult::complete(dedup_sort(out));
        }
        if self.push_module_name_context(&mut out, &ctx) {
            return CompletionResult::complete(dedup_sort(out));
        }
        if self.push_string_argument_values(&mut out, &ctx, &symbol_source) {
            return CompletionResult::incomplete(dedup_sort(out));
        }
        if self.push_member_context(&mut out, &ctx, &symbols) {
            return CompletionResult::complete(dedup_sort(out));
        }
        self.push_named_args(&mut out, &ctx, &symbols);
        if !out.is_empty() {
            return CompletionResult::complete(dedup_sort(out));
        }
        if should_suppress_general(&ctx, request.trigger) {
            return CompletionResult::incomplete(Vec::new());
        }
        self.push_general(&mut out, &ctx, &symbols);
        CompletionResult::complete(dedup_sort(out))
    }

    fn push_repl_commands(&self, out: &mut Vec<CompletionCandidate>, ctx: &CompletionContext<'_>) {
        let typed = ctx.line_prefix.trim_start();
        let start = ctx.cursor - typed.len();
        for command in [":help", ":quit", ":exit", ":q"] {
            if command.starts_with(typed) {
                out.push(CompletionCandidate::new(
                    command,
                    CompletionKind::Command,
                    Some("REPL command".to_string()),
                    command,
                    start,
                    ctx.cursor,
                ));
            }
        }
    }

    fn push_import_path(
        &self,
        out: &mut Vec<CompletionCandidate>,
        ctx: &CompletionContext<'_>,
        base_dir: Option<&Path>,
    ) -> bool {
        let Some(start_quote) = ctx.line_prefix.rfind("use \"") else {
            return false;
        };
        let typed_start = ctx.line_start + start_quote + "use \"".len();
        let typed = &ctx.source[typed_start..ctx.cursor];
        if typed.contains('"') {
            return false;
        }

        let mut base_dirs = Vec::new();
        if let Some(base) = base_dir {
            base_dirs.push(base.to_path_buf());
            base_dirs.push(base.join("lib"));
            base_dirs.push(base.join("modules"));
        }

        let (dir_part, file_prefix) = split_path_prefix(typed);
        for base in base_dirs {
            let root = if dir_part.is_empty() { base } else { base.join(dir_part) };
            let Ok(entries) = std::fs::read_dir(root) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(ft) = entry.file_type() else {
                    continue;
                };
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.starts_with(file_prefix) {
                    continue;
                }
                let rel = if dir_part.is_empty() {
                    name
                } else {
                    format!("{dir_part}/{name}")
                };
                let label = if ft.is_dir() { format!("{rel}/") } else { rel };
                out.push(CompletionCandidate::new(
                    label.clone(),
                    if ft.is_dir() {
                        CompletionKind::Folder
                    } else {
                        CompletionKind::File
                    },
                    Some("File path".to_string()),
                    label,
                    typed_start,
                    ctx.cursor,
                ));
            }
        }
        true
    }

    fn push_brace_import_exports(&self, out: &mut Vec<CompletionCandidate>, ctx: &CompletionContext<'_>) -> bool {
        let Some(brace_pos) = ctx.line_prefix.rfind("use {") else {
            return false;
        };
        let after_brace = &ctx.source[ctx.line_start + brace_pos + "use {".len()..ctx.cursor];
        if after_brace.contains('}') {
            return false;
        }
        let Some(module_name) = module_after_brace_import(ctx.line_suffix) else {
            return false;
        };
        let typed = after_brace.split(',').next_back().unwrap_or("").trim_start();
        let typed_start = ctx.cursor - typed.len();
        for (name, kind, detail) in self.export_names_at_path(&[module_name]).unwrap_or_default() {
            if name.starts_with(typed) {
                out.push(CompletionCandidate::new(
                    name.clone(),
                    kind,
                    Some(detail),
                    name,
                    typed_start,
                    ctx.cursor,
                ));
            }
        }
        true
    }

    fn push_module_name_context(&self, out: &mut Vec<CompletionCandidate>, ctx: &CompletionContext<'_>) -> bool {
        let trimmed = ctx.line_prefix.trim_start();
        let module_prefix = trimmed
            .strip_prefix("use ")
            .or_else(|| trimmed.strip_prefix("from "))
            .filter(|rest| rest.chars().all(|ch| is_ident_continue(ch)));
        let Some(typed) = module_prefix else {
            return false;
        };
        let typed_start = ctx.cursor - typed.len();
        for module in stdlib_catalog()
            .module_names()
            .into_iter()
            .filter(|module| module.starts_with(typed))
        {
            out.push(CompletionCandidate::new(
                module.clone(),
                CompletionKind::Module,
                Some("LK stdlib module".to_string()),
                module,
                typed_start,
                ctx.cursor,
            ));
        }
        true
    }

    fn push_member_context(
        &self,
        out: &mut Vec<CompletionCandidate>,
        ctx: &CompletionContext<'_>,
        symbols: &SymbolIndex,
    ) -> bool {
        let prefix = ctx.identifier_path_prefix();
        let Some(dot) = prefix.rfind('.') else {
            return false;
        };
        let qualifier = &prefix[..dot];
        let typed = &prefix[dot + 1..];
        let replace_start = ctx.cursor - typed.len();
        let path: Vec<&str> = qualifier.split('.').filter(|part| !part.is_empty()).collect();
        if path.is_empty() {
            return false;
        }
        if let Some(module_name) = symbols
            .import_aliases
            .get(path[0])
            .map(String::as_str)
            .or_else(|| stdlib_catalog().module(path[0]).map(|_| path[0]))
        {
            let mut module_path = Vec::with_capacity(path.len());
            module_path.push(module_name);
            module_path.extend_from_slice(&path[1..]);
            if let Some(exports) = self.export_names_at_path(&module_path) {
                for (name, kind, detail) in exports.into_iter().filter(|(name, _, _)| name.starts_with(typed)) {
                    out.push(CompletionCandidate::new(
                        name.clone(),
                        kind,
                        Some(detail),
                        name,
                        replace_start,
                        ctx.cursor,
                    ));
                }
                return true;
            }
        }

        let receiver_type = symbols.types.get(path[0]).copied();
        for (name, owner) in method_candidates(receiver_type) {
            if name.starts_with(typed) {
                out.push(CompletionCandidate::new(
                    name,
                    CompletionKind::Method,
                    Some(format!("{owner} method")),
                    name,
                    replace_start,
                    ctx.cursor,
                ));
            }
        }
        true
    }

    fn push_named_args(&self, out: &mut Vec<CompletionCandidate>, ctx: &CompletionContext<'_>, symbols: &SymbolIndex) {
        let Some((name, args_start)) = find_call_before_cursor(ctx.line_prefix) else {
            return;
        };
        let Some(params) = symbols.named_params.get(name) else {
            return;
        };
        let provided = collect_named_keys(&ctx.line_prefix[args_start..]);
        let typed = ctx.current_identifier_prefix();
        let replace_start = ctx.cursor - typed.len();
        for param in params {
            if provided.contains(param.as_str()) || !param.starts_with(typed) {
                continue;
            }
            out.push(CompletionCandidate::new(
                format!("{param}:"),
                CompletionKind::Field,
                Some("named argument".to_string()),
                format!("{param}: "),
                replace_start,
                ctx.cursor,
            ));
        }
    }

    fn push_string_argument_values(
        &self,
        out: &mut Vec<CompletionCandidate>,
        ctx: &CompletionContext<'_>,
        symbol_source: &str,
    ) -> bool {
        let Some(arg) = ctx.first_string_argument_context() else {
            return false;
        };
        for value in collect_first_string_argument_values(symbol_source, arg.function_name) {
            if !value.starts_with(arg.typed) {
                continue;
            }
            out.push(CompletionCandidate::new(
                value.clone(),
                CompletionKind::Value,
                Some(format!("string argument for {}", arg.function_name)),
                value,
                arg.typed_start,
                ctx.cursor,
            ));
        }
        true
    }

    fn push_general(&self, out: &mut Vec<CompletionCandidate>, ctx: &CompletionContext<'_>, symbols: &SymbolIndex) {
        let typed = ctx.current_identifier_prefix();
        let replace_start = ctx.cursor - typed.len();
        for &keyword in KEYWORDS {
            if keyword.starts_with(typed) {
                out.push(CompletionCandidate::new(
                    keyword,
                    CompletionKind::Keyword,
                    Some("LK keyword".to_string()),
                    keyword,
                    replace_start,
                    ctx.cursor,
                ));
            }
        }
        for &op in OPERATORS {
            if op.starts_with(typed) && !typed.is_empty() {
                out.push(CompletionCandidate::new(
                    op,
                    CompletionKind::Operator,
                    Some("LK operator".to_string()),
                    op,
                    replace_start,
                    ctx.cursor,
                ));
            }
        }
        for &ty in TYPES {
            if ty.starts_with(typed) {
                out.push(CompletionCandidate::new(
                    ty,
                    CompletionKind::Type,
                    Some("LK type".to_string()),
                    ty,
                    replace_start,
                    ctx.cursor,
                ));
            }
        }
        for global in &stdlib_catalog().globals {
            let name = &global.name;
            if name.starts_with(typed) && !name.contains("::") && !name.contains('$') {
                out.push(CompletionCandidate::new(
                    name.to_string(),
                    CompletionKind::Function,
                    Some(global.detail.clone()),
                    name.to_string(),
                    replace_start,
                    ctx.cursor,
                ));
            }
        }
        for module in stdlib_catalog().module_names() {
            if module.starts_with(typed) {
                out.push(CompletionCandidate::new(
                    module.clone(),
                    CompletionKind::Module,
                    Some("stdlib module".to_string()),
                    module,
                    replace_start,
                    ctx.cursor,
                ));
            }
        }
        for symbol in &symbols.symbols {
            if symbol.name.starts_with(typed) {
                out.push(CompletionCandidate::new(
                    symbol.name.clone(),
                    symbol.kind,
                    symbol.detail.clone(),
                    symbol.name.clone(),
                    replace_start,
                    ctx.cursor,
                ));
            }
        }
    }

    fn export_names_at_path(&self, path: &[&str]) -> Option<Vec<(String, CompletionKind, String)>> {
        let exports: Vec<&StdlibExportSpec> = if path.len() == 1 {
            stdlib_catalog()
                .module(path.first().copied()?)?
                .exports
                .iter()
                .collect()
        } else {
            stdlib_catalog().export_path(path)?.children.iter().collect()
        };
        let mut out: Vec<_> = exports
            .into_iter()
            .map(|export| {
                (
                    export.name.clone(),
                    completion_kind_from_stdlib(export),
                    export.detail.clone(),
                )
            })
            .collect();
        out.sort_by(|left, right| left.0.cmp(&right.0));
        Some(out)
    }
}

fn completion_kind_from_stdlib(export: &StdlibExportSpec) -> CompletionKind {
    match export.kind {
        StdlibExportKind::Function => CompletionKind::Function,
        StdlibExportKind::Module => CompletionKind::Module,
        StdlibExportKind::Value => CompletionKind::Value,
    }
}

impl Default for CompletionEngine {
    fn default() -> Self {
        match Self::new() {
            Ok(engine) => engine,
            Err(err) => {
                eprintln!("failed to initialize stdlib completion registry: {err}");
                Self::fallback()
            }
        }
    }
}

#[derive(Debug)]
struct CompletionContext<'a> {
    source: &'a str,
    cursor: usize,
    line_start: usize,
    line_prefix: &'a str,
    line_suffix: &'a str,
}

impl<'a> CompletionContext<'a> {
    fn new(source: &'a str, cursor: usize) -> Self {
        let line_start = source[..cursor].rfind('\n').map_or(0, |idx| idx + 1);
        let line_end = source[cursor..].find('\n').map_or(source.len(), |idx| cursor + idx);
        Self {
            source,
            cursor,
            line_start,
            line_prefix: &source[line_start..cursor],
            line_suffix: &source[cursor..line_end],
        }
    }

    fn current_identifier_prefix(&self) -> &str {
        let start = self
            .line_prefix
            .char_indices()
            .rev()
            .find_map(|(idx, ch)| (!is_ident_continue(ch)).then_some(idx + ch.len_utf8()))
            .unwrap_or(0);
        &self.line_prefix[start..]
    }

    fn identifier_path_prefix(&self) -> &str {
        let start = self
            .line_prefix
            .char_indices()
            .rev()
            .find_map(|(idx, ch)| (!(is_ident_continue(ch) || ch == '.')).then_some(idx + ch.len_utf8()))
            .unwrap_or(0);
        &self.line_prefix[start..]
    }

    fn first_string_argument_context(&self) -> Option<StringArgumentContext<'a>> {
        let (quote_start, _quote) = active_string_start(self.line_prefix)?;
        let before_quote = &self.line_prefix[..quote_start];
        let (function_name, args_start) = find_call_before_cursor(before_quote)?;
        if !before_quote[args_start..].trim().is_empty() {
            return None;
        }

        let typed_start = self.line_start + quote_start + 1;
        Some(StringArgumentContext {
            function_name,
            typed: &self.source[typed_start..self.cursor],
            typed_start,
        })
    }
}

#[derive(Debug)]
struct StringArgumentContext<'a> {
    function_name: &'a str,
    typed: &'a str,
    typed_start: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReceiverType {
    List,
    Map,
    Set,
    String,
}

#[derive(Debug, Clone)]
struct SymbolInfo {
    name: String,
    kind: CompletionKind,
    detail: Option<String>,
}

#[derive(Debug, Default)]
struct SymbolIndex {
    symbols: Vec<SymbolInfo>,
    import_aliases: HashMap<String, String>,
    named_params: HashMap<String, Vec<String>>,
    types: HashMap<String, ReceiverType>,
}

impl SymbolIndex {
    fn from_source(source: &str) -> Self {
        let Ok((tokens, _spans)) = Tokenizer::tokenize_enhanced_with_spans(source) else {
            return Self::scan_lines(source);
        };
        let mut index = Self::default();
        let mut seen = BTreeSet::new();
        let mut push = |index: &mut Self, name: String, kind: CompletionKind, detail: Option<String>| {
            if seen.insert((name.clone(), kind)) {
                index.symbols.push(SymbolInfo { name, kind, detail });
            }
        };

        let mut i = 0usize;
        while i < tokens.len() {
            match &tokens[i] {
                Token::Use => {
                    i = scan_import(&tokens, i, &mut index, &mut push);
                }
                Token::Fn => {
                    if let Some(Token::Id(name)) = tokens.get(i + 1) {
                        push(
                            &mut index,
                            name.clone(),
                            CompletionKind::Function,
                            Some("function".to_string()),
                        );
                        if let Some((params, next)) = scan_fn_params(&tokens, i + 2) {
                            for param in &params.positional {
                                push(
                                    &mut index,
                                    param.clone(),
                                    CompletionKind::Variable,
                                    Some("parameter".to_string()),
                                );
                            }
                            if !params.named.is_empty() {
                                index.named_params.insert(name.clone(), params.named.clone());
                            }
                            i = next;
                            continue;
                        }
                    }
                    i += 1;
                }
                Token::Let | Token::Const => {
                    i = scan_let(&tokens, i, &mut index, &mut push);
                }
                Token::Struct | Token::Trait | Token::Type => {
                    if let Some(Token::Id(name)) = tokens.get(i + 1) {
                        push(&mut index, name.clone(), CompletionKind::Type, Some("type".to_string()));
                    }
                    i += 1;
                }
                Token::Id(name) => {
                    if matches!(tokens.get(i + 1), Some(Token::Colon))
                        && matches!(tokens.get(i + 2), Some(Token::Assign))
                    {
                        push(
                            &mut index,
                            name.clone(),
                            CompletionKind::Variable,
                            Some("local".to_string()),
                        );
                        i += 3;
                    } else {
                        i += 1;
                    }
                }
                _ => i += 1,
            }
        }
        index.symbols.sort_by(|left, right| left.name.cmp(&right.name));
        index
    }

    fn scan_lines(source: &str) -> Self {
        let mut index = Self::default();
        let mut seen = BTreeSet::new();
        for line in source.lines() {
            let trimmed = line.trim_start();
            if let Some(rest) = trimmed.strip_prefix("fn ") {
                if let Some(name) = leading_identifier(rest) {
                    if seen.insert((name.to_string(), CompletionKind::Function)) {
                        index.symbols.push(SymbolInfo {
                            name: name.to_string(),
                            kind: CompletionKind::Function,
                            detail: Some("function".to_string()),
                        });
                    }
                }
            } else if let Some(rest) = trimmed.strip_prefix("let ").or_else(|| trimmed.strip_prefix("const ")) {
                if let Some(name) = leading_identifier(rest) {
                    if seen.insert((name.to_string(), CompletionKind::Variable)) {
                        index.symbols.push(SymbolInfo {
                            name: name.to_string(),
                            kind: CompletionKind::Variable,
                            detail: Some("local".to_string()),
                        });
                    }
                }
            } else if let Some(rest) = trimmed
                .strip_prefix("trait ")
                .or_else(|| trimmed.strip_prefix("struct "))
                .or_else(|| trimmed.strip_prefix("type "))
            {
                if let Some(name) = leading_identifier(rest) {
                    if seen.insert((name.to_string(), CompletionKind::Type)) {
                        index.symbols.push(SymbolInfo {
                            name: name.to_string(),
                            kind: CompletionKind::Type,
                            detail: Some("type".to_string()),
                        });
                    }
                }
            }
        }
        index
    }
}

#[derive(Default)]
struct FnParams {
    positional: Vec<String>,
    named: Vec<String>,
}

fn scan_fn_params(tokens: &[Token], start: usize) -> Option<(FnParams, usize)> {
    if !matches!(tokens.get(start), Some(Token::LParen)) {
        return None;
    }
    let mut params = FnParams::default();
    let mut i = start + 1;
    let mut paren = 1i32;
    let mut named_depth = 0i32;
    while i < tokens.len() && paren > 0 {
        match &tokens[i] {
            Token::LParen => paren += 1,
            Token::RParen => paren -= 1,
            Token::LBrace if paren == 1 => named_depth += 1,
            Token::RBrace if paren == 1 => named_depth -= 1,
            Token::Id(name) if paren == 1 && named_depth == 0 => {
                let previous_is_param_boundary =
                    matches!(tokens.get(i.wrapping_sub(1)), Some(Token::LParen | Token::Comma));
                if previous_is_param_boundary && matches!(tokens.get(i + 1), Some(Token::Colon)) {
                    params.positional.push(name.clone());
                } else if previous_is_param_boundary && matches!(tokens.get(i + 1), Some(Token::Comma | Token::RParen))
                {
                    params.positional.push(name.clone());
                }
            }
            Token::Id(name) if paren == 1 && named_depth == 1 && matches!(tokens.get(i + 1), Some(Token::Colon)) => {
                params.named.push(name.clone());
            }
            _ => {}
        }
        i += 1;
    }
    Some((params, i))
}

fn scan_import<F>(tokens: &[Token], start: usize, index: &mut SymbolIndex, push: &mut F) -> usize
where
    F: FnMut(&mut SymbolIndex, String, CompletionKind, Option<String>),
{
    match tokens.get(start + 1) {
        Some(Token::Id(module)) => {
            if matches!(tokens.get(start + 2), Some(Token::As)) {
                if let Some(Token::Id(alias)) = tokens.get(start + 3) {
                    index.import_aliases.insert(alias.clone(), module.clone());
                    push(
                        index,
                        alias.clone(),
                        CompletionKind::Module,
                        Some(format!("alias for {module}")),
                    );
                    return start + 4;
                }
            }
            index.import_aliases.insert(module.clone(), module.clone());
            push(
                index,
                module.clone(),
                CompletionKind::Module,
                Some("module".to_string()),
            );
            start + 2
        }
        Some(Token::Mul) if matches!(tokens.get(start + 2), Some(Token::As)) => {
            if let (Some(Token::Id(alias)), Some(Token::From), Some(Token::Id(module))) =
                (tokens.get(start + 3), tokens.get(start + 4), tokens.get(start + 5))
            {
                index.import_aliases.insert(alias.clone(), module.clone());
                push(
                    index,
                    alias.clone(),
                    CompletionKind::Module,
                    Some(format!("alias for {module}")),
                );
                start + 6
            } else {
                start + 1
            }
        }
        Some(Token::LBrace) => {
            let mut i = start + 2;
            while i < tokens.len() && !matches!(tokens[i], Token::RBrace) {
                if let Token::Id(name) = &tokens[i] {
                    let binding = if matches!(tokens.get(i + 1), Some(Token::As)) {
                        if let Some(Token::Id(alias)) = tokens.get(i + 2) {
                            i += 2;
                            alias.clone()
                        } else {
                            name.clone()
                        }
                    } else {
                        name.clone()
                    };
                    push(
                        index,
                        binding,
                        CompletionKind::Function,
                        Some("imported item".to_string()),
                    );
                }
                i += 1;
            }
            i + 1
        }
        _ => start + 1,
    }
}

fn scan_let<F>(tokens: &[Token], start: usize, index: &mut SymbolIndex, push: &mut F) -> usize
where
    F: FnMut(&mut SymbolIndex, String, CompletionKind, Option<String>),
{
    let mut i = start + 1;
    let mut names = Vec::new();
    while i < tokens.len() {
        match &tokens[i] {
            Token::Id(name) => names.push(name.clone()),
            Token::Colon => {
                if let Some(Token::Id(ty)) = tokens.get(i + 1) {
                    for name in &names {
                        if let Some(receiver_ty) = receiver_type_from_name(ty) {
                            index.types.insert(name.clone(), receiver_ty);
                        }
                    }
                }
                break;
            }
            Token::Assign => {
                let inferred = infer_receiver_type_from_tokens(tokens.get(i + 1));
                for name in &names {
                    if let Some(receiver_ty) = inferred {
                        index.types.insert(name.clone(), receiver_ty);
                    }
                }
                break;
            }
            Token::Semicolon => break,
            _ => {}
        }
        i += 1;
    }
    for name in names {
        push(index, name, CompletionKind::Variable, Some("local".to_string()));
    }
    i
}

fn infer_receiver_type_from_tokens(token: Option<&Token>) -> Option<ReceiverType> {
    match token {
        Some(Token::Str(_) | Token::TemplateString(_)) => Some(ReceiverType::String),
        Some(Token::LBracket) => Some(ReceiverType::List),
        Some(Token::LBrace) => Some(ReceiverType::Map),
        _ => None,
    }
}

fn receiver_type_from_name(name: &str) -> Option<ReceiverType> {
    match name {
        "String" | "Str" => Some(ReceiverType::String),
        "List" => Some(ReceiverType::List),
        "Map" => Some(ReceiverType::Map),
        "Set" => Some(ReceiverType::Set),
        _ => None,
    }
}

fn merged_symbol_source(source: &str, session_source: Option<&str>) -> String {
    match session_source {
        Some(session) if !session.trim().is_empty() => {
            let mut merged = String::with_capacity(session.len() + source.len() + 1);
            merged.push_str(session);
            merged.push('\n');
            merged.push_str(source);
            merged
        }
        _ => source.to_string(),
    }
}

fn dedup_sort(items: Vec<CompletionCandidate>) -> Vec<CompletionCandidate> {
    let mut by_key = BTreeMap::<(String, CompletionKind), CompletionCandidate>::new();
    for item in items {
        by_key.entry((item.label.clone(), item.kind)).or_insert(item);
    }
    by_key.into_values().collect()
}

fn split_path_prefix(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(idx) => (&path[..idx], &path[idx + 1..]),
        None => ("", path),
    }
}

fn module_after_brace_import(suffix: &str) -> Option<&str> {
    let suffix = suffix.trim_start();
    let suffix = suffix.strip_prefix('}').unwrap_or(suffix).trim_start();
    let suffix = suffix.strip_prefix("from ")?;
    leading_identifier(suffix)
}

fn leading_identifier(input: &str) -> Option<&str> {
    let mut chars = input.char_indices();
    let (_, first) = chars.next()?;
    if !is_ident_start(first) {
        return None;
    }
    let end = chars
        .find_map(|(idx, ch)| (!is_ident_continue(ch)).then_some(idx))
        .unwrap_or(input.len());
    Some(&input[..end])
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    is_ident_start(ch) || ch.is_ascii_digit() || ch == '-'
}

fn find_call_before_cursor(prefix: &str) -> Option<(&str, usize)> {
    let idx = prefix.rfind('(')?;
    let before = prefix[..idx].trim_end();
    let name_start = before
        .char_indices()
        .rev()
        .find_map(|(idx, ch)| (!is_ident_continue(ch)).then_some(idx + ch.len_utf8()))
        .unwrap_or(0);
    let name = &before[name_start..];
    (!name.is_empty()).then_some((name, idx + 1))
}

fn collect_named_keys(args: &str) -> BTreeSet<&str> {
    let mut out = BTreeSet::new();
    for part in args.split(',') {
        let trimmed = part.trim_start();
        if let Some(colon) = trimmed.find(':') {
            let key = trimmed[..colon].trim();
            if !key.is_empty() && key.chars().all(is_ident_continue) {
                out.insert(key);
            }
        }
    }
    out
}

fn should_suppress_general(ctx: &CompletionContext<'_>, trigger: CompletionTrigger) -> bool {
    matches!(
        trigger,
        CompletionTrigger::TriggerCharacter('{' | '"' | '\'' | ',' | ':')
    ) && ctx.current_identifier_prefix().is_empty()
}

fn active_string_start(line_prefix: &str) -> Option<(usize, char)> {
    let mut active = None;
    let mut escaped = false;
    for (idx, ch) in line_prefix.char_indices() {
        if let Some((_, quote)) = active {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                active = None;
            }
        } else if ch == '"' || ch == '\'' {
            active = Some((idx, ch));
        }
    }
    active
}

fn collect_first_string_argument_values(source: &str, function_name: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let Ok((tokens, _spans)) = Tokenizer::tokenize_enhanced_with_spans(source) {
        let mut i = 0usize;
        while i + 2 < tokens.len() {
            if matches!(&tokens[i], Token::Id(name) if name == function_name)
                && matches!(tokens[i + 1], Token::LParen)
                && let Token::Str(value) = &tokens[i + 2]
            {
                if !value.is_empty() {
                    out.insert(value.clone());
                }
                i += 3;
                continue;
            }
            i += 1;
        }
        return out;
    }

    collect_first_string_argument_values_text(source, function_name, &mut out);
    out
}

fn collect_first_string_argument_values_text(source: &str, function_name: &str, out: &mut BTreeSet<String>) {
    let mut offset = 0usize;
    while let Some(rel) = source[offset..].find(function_name) {
        let name_start = offset + rel;
        let name_end = name_start + function_name.len();
        if !identifier_boundary(source, name_start, name_end) {
            offset = name_end;
            continue;
        }
        let mut cursor = skip_ascii_ws(source, name_end);
        if source.as_bytes().get(cursor) != Some(&b'(') {
            offset = name_end;
            continue;
        }
        cursor = skip_ascii_ws(source, cursor + 1);
        let Some(quote) = source
            .as_bytes()
            .get(cursor)
            .copied()
            .filter(|byte| *byte == b'"' || *byte == b'\'')
        else {
            offset = cursor;
            continue;
        };
        if let Some((value, next)) = parse_quoted_value(source, cursor + 1, quote) {
            if !value.is_empty() {
                out.insert(value);
            }
            offset = next;
        } else {
            offset = cursor + 1;
        }
    }
}

fn identifier_boundary(source: &str, start: usize, end: usize) -> bool {
    let before = source[..start].chars().next_back();
    let after = source[end..].chars().next();
    before.is_none_or(|ch| !is_ident_continue(ch)) && after.is_none_or(|ch| !is_ident_continue(ch))
}

fn skip_ascii_ws(source: &str, mut cursor: usize) -> usize {
    while source.as_bytes().get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    cursor
}

fn parse_quoted_value(source: &str, mut cursor: usize, quote: u8) -> Option<(String, usize)> {
    let mut value = String::new();
    let bytes = source.as_bytes();
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if byte == quote {
            return Some((value, cursor + 1));
        }
        if byte == b'\\' {
            let next = *bytes.get(cursor + 1)?;
            value.push(match next {
                b'n' => '\n',
                b'r' => '\r',
                b't' => '\t',
                b'\\' => '\\',
                b'\'' => '\'',
                b'"' => '"',
                b'0' => '\0',
                other => other as char,
            });
            cursor += 2;
        } else {
            value.push(byte as char);
            cursor += 1;
        }
    }
    None
}

fn method_candidates(receiver_type: Option<ReceiverType>) -> Vec<(&'static str, &'static str)> {
    const LIST: &[&str] = &[
        "len",
        "push",
        "concat",
        "join",
        "get",
        "first",
        "last",
        "map",
        "filter",
        "reduce",
        "take",
        "skip",
        "chain",
        "flatten",
        "unique",
        "chunk",
        "enumerate",
        "zip",
        "contains",
    ];
    const MAP: &[&str] = &[
        "len", "keys", "values", "has", "contains", "get", "set", "delete", "clear",
    ];
    const SET: &[&str] = &["len", "has", "contains", "insert", "delete", "clear"];
    const STRING: &[&str] = &[
        "len",
        "lower",
        "upper",
        "trim",
        "starts_with",
        "ends_with",
        "contains",
        "replace",
        "substring",
        "split",
        "join",
        "to_int",
        "to_float",
    ];
    let mut out = Vec::new();
    let groups: &[(&[&str], &str)] = match receiver_type {
        Some(ReceiverType::List) => &[(LIST, "List")],
        Some(ReceiverType::Map) => &[(MAP, "Map")],
        Some(ReceiverType::Set) => &[(SET, "Set")],
        Some(ReceiverType::String) => &[(STRING, "String")],
        None => &[(LIST, "List"), (MAP, "Map"), (SET, "Set"), (STRING, "String")],
    };
    let mut seen = BTreeSet::new();
    for (items, owner) in groups {
        for item in *items {
            if seen.insert(*item) {
                out.push((*item, *owner));
            }
        }
    }
    out
}

const KEYWORDS: &[&str] = &[
    "if", "else", "while", "for", "let", "const", "fn", "return", "break", "continue", "use", "from", "as", "match",
    "case", "default", "true", "false", "nil", "select", "struct", "trait", "impl", "type",
];

const OPERATORS: &[&str] = &["==", "!=", "<=", ">=", "&&", "||", "in", "<-", "??", "..", "..="];

const TYPES: &[&str] = &[
    "Int",
    "Float",
    "Bool",
    "String",
    "Str",
    "Nil",
    "List",
    "Map",
    "Set",
    "Function",
    "Object",
    "Task",
    "Channel",
    "Stream",
    "StreamCursor",
];

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn labels(items: Vec<CompletionCandidate>) -> Vec<String> {
        items.into_iter().map(|item| item.label).collect()
    }

    #[test]
    fn completes_stdlib_globals_from_registry() {
        let engine = CompletionEngine::new().unwrap();
        let got = labels(engine.complete(CompletionRequest {
            source: "ass",
            cursor: 3,
            mode: CompletionMode::Lsp,
            trigger: CompletionTrigger::Invoked,
            session_source: None,
            base_dir: None,
        }));
        assert!(got.contains(&"assert".to_string()));
        assert!(got.contains(&"assert_eq".to_string()));
        assert!(got.contains(&"assert_ne".to_string()));
    }

    #[test]
    fn completes_nested_stdlib_exports() {
        let engine = CompletionEngine::new().unwrap();
        let got = labels(engine.complete(CompletionRequest {
            source: "io.file.read",
            cursor: "io.file.read".len(),
            mode: CompletionMode::Lsp,
            trigger: CompletionTrigger::Invoked,
            session_source: None,
            base_dir: None,
        }));
        assert!(got.contains(&"read_to_string".to_string()), "{got:?}");
    }

    #[test]
    fn completes_local_and_session_symbols() {
        let engine = CompletionEngine::new().unwrap();
        let got = labels(engine.complete(CompletionRequest {
            source: "use",
            cursor: 3,
            mode: CompletionMode::Repl,
            trigger: CompletionTrigger::Invoked,
            session_source: Some("let user_name = 1;\nfn user_score() { return 1; }"),
            base_dir: None,
        }));
        assert!(got.contains(&"user_name".to_string()));
        assert!(got.contains(&"user_score".to_string()));
    }

    #[test]
    fn completes_session_type_declarations() {
        let engine = CompletionEngine::new().unwrap();
        let session_source = "trait Drawable {}\nstruct Point {}\ntype UserId = Int;";

        let drawable = labels(engine.complete(CompletionRequest {
            source: "Dra",
            cursor: 3,
            mode: CompletionMode::Repl,
            trigger: CompletionTrigger::Invoked,
            session_source: Some(session_source),
            base_dir: None,
        }));
        assert!(drawable.contains(&"Drawable".to_string()));

        let point = labels(engine.complete(CompletionRequest {
            source: "Poi",
            cursor: 3,
            mode: CompletionMode::Repl,
            trigger: CompletionTrigger::Invoked,
            session_source: Some(session_source),
            base_dir: None,
        }));
        assert!(point.contains(&"Point".to_string()));

        let user_id = labels(engine.complete(CompletionRequest {
            source: "User",
            cursor: 4,
            mode: CompletionMode::Repl,
            trigger: CompletionTrigger::Invoked,
            session_source: Some(session_source),
            base_dir: None,
        }));
        assert!(user_id.contains(&"UserId".to_string()));
    }

    #[test]
    fn completes_named_arguments() {
        let engine = CompletionEngine::new().unwrap();
        let source = "fn draw({width: Int, height: Int}) { }\ndraw(w";
        let got = engine.complete(CompletionRequest {
            source,
            cursor: source.len(),
            mode: CompletionMode::Lsp,
            trigger: CompletionTrigger::Invoked,
            session_source: None,
            base_dir: None,
        });
        assert!(
            got.iter()
                .any(|item| item.label == "width:" && item.replacement == "width: ")
        );
    }

    #[test]
    fn typed_positional_params_do_not_complete_type_names_as_locals() {
        let engine = CompletionEngine::new().unwrap();
        let source = "fn f(a: Int, b: String) { I";
        let got = engine.complete(CompletionRequest {
            source,
            cursor: source.len(),
            mode: CompletionMode::Lsp,
            trigger: CompletionTrigger::Invoked,
            session_source: None,
            base_dir: None,
        });
        assert!(got.iter().any(|item| item.label == "Int"));
        assert!(
            !got.iter()
                .any(|item| item.label == "Int" && item.detail.as_deref() == Some("parameter"))
        );
    }

    #[test]
    fn completes_receiver_methods_with_light_type_filter() {
        let engine = CompletionEngine::new().unwrap();
        let source = "let s: String = \"x\";\ns.st";
        let got = labels(engine.complete(CompletionRequest {
            source,
            cursor: source.len(),
            mode: CompletionMode::Lsp,
            trigger: CompletionTrigger::Invoked,
            session_source: None,
            base_dir: None,
        }));
        assert!(got.contains(&"starts_with".to_string()));
        assert!(!got.contains(&"set".to_string()));
    }

    #[test]
    fn completes_import_paths() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("main.lk"), "").unwrap();
        let engine = CompletionEngine::new().unwrap();
        let got = labels(engine.complete(CompletionRequest {
            source: "use \"ma",
            cursor: "use \"ma".len(),
            mode: CompletionMode::Lsp,
            trigger: CompletionTrigger::Invoked,
            session_source: None,
            base_dir: Some(dir.path()),
        }));
        assert!(got.contains(&"main.lk".to_string()));
    }

    #[test]
    fn completes_first_string_argument_from_existing_calls() {
        let engine = CompletionEngine::new().unwrap();
        let source =
            "if should_run(\"gcd_batch\") {}\nif should_run(\"prime_trial_division\") {}\nif should_run(\"pri\") {}";
        let cursor = source.rfind("pri").unwrap() + "pri".len();
        let got = engine.complete(CompletionRequest {
            source,
            cursor,
            mode: CompletionMode::Lsp,
            trigger: CompletionTrigger::Invoked,
            session_source: None,
            base_dir: None,
        });
        assert!(got.iter().any(|item| item.label == "prime_trial_division"));
        assert!(!got.iter().any(|item| item.label == "gcd_batch"));
        assert!(!got.iter().any(|item| item.label == "if"));
        assert!(!got.iter().any(|item| item.label == "Int"));
        assert_eq!(got[0].replace_start, cursor - "pri".len());
        assert_eq!(got[0].replace_end, cursor);
    }

    #[test]
    fn completes_local_function_after_normal_identifier_typing() {
        let engine = CompletionEngine::new().unwrap();
        let source = "fn should_run(name) { return true; }\nsho";
        let got = labels(engine.complete(CompletionRequest {
            source,
            cursor: source.len(),
            mode: CompletionMode::Lsp,
            trigger: CompletionTrigger::Invoked,
            session_source: None,
            base_dir: None,
        }));
        assert!(got.contains(&"should_run".to_string()));
    }

    #[test]
    fn completes_local_function_after_empty_block_completion_session() {
        let engine = CompletionEngine::new().unwrap();
        let source = "fn should_run(name) { return true; }\nif should_run(\"\") {\nsho";
        let got = labels(engine.complete(CompletionRequest {
            source,
            cursor: source.len(),
            mode: CompletionMode::Lsp,
            trigger: CompletionTrigger::Incomplete,
            session_source: None,
            base_dir: None,
        }));
        assert!(got.contains(&"should_run".to_string()));
    }

    #[test]
    fn structural_trigger_empty_result_is_incomplete() {
        let engine = CompletionEngine::new().unwrap();
        let source = "let a0 = 1;\nif should_run(\"\") {";
        let got = engine.complete_with_metadata(CompletionRequest {
            source,
            cursor: source.len(),
            mode: CompletionMode::Lsp,
            trigger: CompletionTrigger::TriggerCharacter('{'),
            session_source: None,
            base_dir: None,
        });
        assert!(got.candidates.is_empty());
        assert!(got.is_incomplete);
    }

    #[test]
    fn completes_first_single_quoted_string_argument() {
        let engine = CompletionEngine::new().unwrap();
        let source = "if should_run('gcd_batch') {}\nif should_run('') {}";
        let cursor = source.rfind("''").unwrap() + 1;
        let got = labels(engine.complete(CompletionRequest {
            source,
            cursor,
            mode: CompletionMode::Lsp,
            trigger: CompletionTrigger::TriggerCharacter('\''),
            session_source: None,
            base_dir: None,
        }));
        assert_eq!(got, vec!["gcd_batch".to_string()]);
    }

    #[test]
    fn suppresses_general_empty_prefix_after_structural_trigger() {
        let engine = CompletionEngine::new().unwrap();
        let source = "let a0 = 1;\nif should_run(\"\") {";
        let got = engine.complete(CompletionRequest {
            source,
            cursor: source.len(),
            mode: CompletionMode::Lsp,
            trigger: CompletionTrigger::TriggerCharacter('{'),
            session_source: None,
            base_dir: None,
        });
        assert!(got.is_empty());
    }

    #[test]
    fn keeps_brace_import_exports_on_brace_trigger() {
        let engine = CompletionEngine::new().unwrap();
        let source = "use {} from io";
        let cursor = "use {".len();
        let got = labels(engine.complete(CompletionRequest {
            source,
            cursor,
            mode: CompletionMode::Lsp,
            trigger: CompletionTrigger::TriggerCharacter('{'),
            session_source: None,
            base_dir: None,
        }));
        assert!(!got.is_empty());
    }
}
