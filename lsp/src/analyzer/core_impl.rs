use super::*;

impl LkrAnalyzer {
    /// Collect user-defined function named-parameter declarations from the current document.
    /// Returns a map: function name -> list of NamedParamDecl (from core AST).
    pub fn collect_fn_named_param_decls(&mut self, content: &str) -> Arc<HashMap<String, Vec<NamedParamDecl>>> {
        let entry = match self.tokenize_with_spans_cached(content) {
            Ok(entry) => entry,
            Err(_) => return Arc::new(HashMap::new()),
        };
        entry
            .named_param_decls
            .get_or_init(|| match entry.parse_program_arc(content) {
                Ok(program) => {
                    let mut out: HashMap<String, Vec<NamedParamDecl>> =
                        HashMap::with_capacity(program.statements.len());
                    for st in &program.statements {
                        if let Stmt::Function { name, named_params, .. } = st.as_ref() {
                            out.insert(name.clone(), named_params.clone());
                        }
                    }
                    Arc::new(out)
                }
                Err(_) => Arc::new(HashMap::new()),
            })
            .clone()
    }

    /// Scan calls in the token stream and produce diagnostics for duplicate/unknown/missing required named args.
    /// This is a lightweight text scan that uses parsed function declarations to know expected names.
    pub fn collect_named_call_diagnostics(
        &mut self,
        content: &str,
        tokens: &[token::Token],
        spans: &[Span],
    ) -> Vec<Diagnostic> {
        use token::Token as T;
        let mut diags: Vec<Diagnostic> = Vec::new();
        let sigs = self.collect_fn_named_param_decls(content);

        let mut i = 0usize;
        while i + 1 < tokens.len() {
            // Look for: Id '(' ... ')'
            let (fname, fspan) = match (&tokens[i], tokens.get(i + 1)) {
                (T::Id(n), Some(T::LParen)) => {
                    let sp = spans.get(i).cloned().unwrap_or_else(|| {
                        Span::single(token::Position {
                            line: 0,
                            column: 0,
                            offset: 0,
                        })
                    });
                    (n.clone(), sp)
                }
                _ => {
                    i += 1;
                    continue;
                }
            };
            // If we don't know the signature, skip
            let Some(named_decls) = sigs.get(&fname) else {
                i += 1;
                continue;
            };
            // Scan arguments until matching ')'
            let mut j = i + 2; // after '('
            let mut paren = 1i32;
            let mut bracket = 0i32;
            let mut brace = 0i32;
            // Collect provided named arg names with their id token index
            let mut provided: Vec<(String, usize)> = Vec::new();
            while j < tokens.len() && paren > 0 {
                match &tokens[j] {
                    T::LParen => paren += 1,
                    T::RParen => paren -= 1,
                    T::LBracket => bracket += 1,
                    T::RBracket => bracket -= 1,
                    T::LBrace => brace += 1,
                    T::RBrace => brace -= 1,
                    // A named arg appears as Id ':' at the top call level (paren==1),
                    // and not inside nested brackets/braces within the argument list.
                    T::Id(n) if paren == 1 && bracket == 0 && brace == 0 => {
                        if j + 1 < tokens.len() && matches!(tokens[j + 1], T::Colon) {
                            provided.push((n.clone(), j));
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            // Build sets for duplicate and unknown
            use std::collections::{HashMap as Map, HashSet as Set};
            let mut counts: Map<&str, usize> = Map::new();
            for (n, _) in &provided {
                *counts.entry(n.as_str()).or_insert(0) += 1;
            }
            let decl_names: Set<&str> = named_decls.iter().map(|d| d.name.as_str()).collect();
            // Duplicate diagnostics
            for (name, idx) in &provided {
                if counts.get(name.as_str()).copied().unwrap_or(0) > 1 {
                    if let Some(sp) = spans.get(*idx) {
                        let range = Range::new(
                            Position::new(sp.start.line - 1, sp.start.column - 1),
                            Position::new(sp.end.line - 1, sp.end.column - 1),
                        );
                        diags.push(Diagnostic::new(
                            range,
                            Some(DiagnosticSeverity::ERROR),
                            None,
                            Some("lkr".to_string()),
                            format!("Duplicate named argument: {}", name),
                            None,
                            None,
                        ));
                    }
                }
            }
            // Unknown diagnostics
            for (name, idx) in &provided {
                if !decl_names.contains(name.as_str()) {
                    if let Some(sp) = spans.get(*idx) {
                        let range = Range::new(
                            Position::new(sp.start.line - 1, sp.start.column - 1),
                            Position::new(sp.end.line - 1, sp.end.column - 1),
                        );
                        diags.push(Diagnostic::new(
                            range,
                            Some(DiagnosticSeverity::ERROR),
                            None,
                            Some("lkr".to_string()),
                            format!("Unknown named argument: {}", name),
                            None,
                            None,
                        ));
                    }
                }
            }
            // Missing required diagnostics (attach to function name span)
            let provided_set: Set<&str> = provided.iter().map(|(n, _)| n.as_str()).collect();
            for decl in named_decls {
                let is_optional = matches!(decl.type_annotation, Some(val::Type::Optional(_)));
                let has_default = decl.default.is_some();
                if !is_optional && !has_default && !provided_set.contains(decl.name.as_str()) {
                    let range = Range::new(
                        Position::new(fspan.start.line - 1, fspan.start.column - 1),
                        Position::new(fspan.end.line - 1, fspan.end.column - 1),
                    );
                    diags.push(Diagnostic::new(
                        range,
                        Some(DiagnosticSeverity::ERROR),
                        None,
                        Some("lkr".to_string()),
                        format!("Missing required named argument: {}", decl.name),
                        None,
                        None,
                    ));
                }
            }

            // Continue scanning after this call
            i = j.max(i + 1);
        }
        diags
    }
    /// Construct a lightweight analyzer that skips stdlib registration.
    /// Use this when only pure text processing is needed (e.g., semantic tokens),
    /// which does not require the stdlib registry.
    pub fn new_light() -> Self {
        Self {
            token_cache: FastHashMap::default(),
            completion_cache: None,
            // Empty registry; populated only in `new()` when needed for stdlib-aware features
            registry: ModuleRegistry::new(),
            base_dir: None,
        }
    }
    /// Create a new LKR analyzer
    pub fn new() -> Self {
        // Initialize a registry preloaded with stdlib modules and globals
        let mut registry = ModuleRegistry::new();
        // Register stdlib globals and modules so LSP can recognize them
        lkr_stdlib::register_stdlib_globals(&mut registry);
        if let Err(err) = lkr_stdlib::register_stdlib_modules(&mut registry) {
            tracing::error!("failed to register stdlib modules: {:#}", err);
        }

        Self {
            token_cache: FastHashMap::default(),
            completion_cache: None,
            registry,
            base_dir: None,
        }
    }

    /// Compute type inlay hints for simple `let name = expr;` without explicit annotations.
    /// Places a TYPE hint like `: Int` right after the pattern (before '=').
    #[cfg(test)]
    pub fn compute_type_inlay_hints(&self, content: &str, range: Range) -> Vec<InlayHint> {
        let (tokens, spans) = match Tokenizer::tokenize_enhanced_with_spans(content) {
            Ok(pair) => pair,
            Err(_) => return Vec::new(),
        };
        self.compute_type_inlay_hints_from_tokens(&tokens, &spans, range)
    }

    /// Variant that reuses a pre-tokenized buffer for performance.
    pub fn compute_type_inlay_hints_from_tokens(
        &self,
        tokens: &[token::Token],
        spans: &[Span],
        range: Range,
    ) -> Vec<InlayHint> {
        let mut hints: Vec<InlayHint> = Vec::new();
        use token::Token as T;
        let mut i = 0usize;
        while i < tokens.len() {
            if !matches!(tokens[i], T::Let) {
                i += 1;
                continue;
            }
            let let_idx = i;
            i += 1;

            // Capture pattern region until top-level ':' (annotation) or '=' (assignment)
            let start_pat = i;
            let mut end_pat = i;
            let mut paren = 0i32;
            let mut bracket = 0i32;
            let mut brace = 0i32;
            let mut saw_colon = false;
            let mut found_assign = false;
            while i < tokens.len() {
                match &tokens[i] {
                    T::LParen => paren += 1,
                    T::RParen => {
                        if paren > 0 {
                            paren -= 1;
                        }
                    }
                    T::LBracket => bracket += 1,
                    T::RBracket => {
                        if bracket > 0 {
                            bracket -= 1;
                        }
                    }
                    T::LBrace => brace += 1,
                    T::RBrace => {
                        if brace > 0 {
                            brace -= 1;
                        }
                    }
                    T::Assign if paren == 0 && bracket == 0 && brace == 0 => {
                        found_assign = true;
                        break;
                    }
                    T::Colon if paren == 0 && bracket == 0 && brace == 0 => {
                        saw_colon = true;
                        break;
                    }
                    _ => {}
                }
                end_pat = i;
                i += 1;
            }
            if !found_assign || saw_colon {
                // Skip cases without '=' or with explicit annotation
                continue;
            }

            // Determine RHS expression token range: after '=' until next top-level ';'
            let mut j = i + 1; // i at '='
            let mut depth = 0i32;
            let mut end_expr = j;
            while j < tokens.len() {
                match &tokens[j] {
                    T::LParen | T::LBracket | T::LBrace => depth += 1,
                    T::RParen | T::RBracket | T::RBrace => depth -= 1,
                    T::Semicolon if depth == 0 => break,
                    _ => {}
                }
                end_expr = j;
                j += 1;
            }
            if end_expr > i {
                // Parse expression and infer type
                let expr_tokens = &tokens[i + 1..=end_expr];
                if !expr_tokens.is_empty() {
                    if let Ok(expr) = ExprParser::new(expr_tokens).parse() {
                        let mut checker = TypeChecker::new_strict();
                        if let Ok(typ) = checker.infer_resolved_type(&expr) {
                            // Place hint at end of pattern
                            let pat_tok_idx = if end_pat >= start_pat { end_pat } else { start_pat };
                            if pat_tok_idx < spans.len() {
                                let sp = &spans[pat_tok_idx];
                                let pos = Position::new(sp.end.line - 1, sp.end.column.saturating_sub(1));
                                if pos.line >= range.start.line && pos.line <= range.end.line {
                                    let label = format!(": {}", typ.display());
                                    hints.push(InlayHint {
                                        position: pos,
                                        label: InlayHintLabel::from(label),
                                        kind: Some(InlayHintKind::TYPE),
                                        text_edits: None,
                                        tooltip: None,
                                        padding_left: Some(true),
                                        padding_right: Some(false),
                                        data: None,
                                    });
                                }
                            }
                        }
                    }
                }
            }

            // Advance to end of statement
            i = j;
            while i < tokens.len() && !matches!(tokens[i], T::Semicolon) {
                i += 1;
            }
            if i < tokens.len() {
                i += 1;
            }
            // Prevent infinite loop on invalid sequences
            if i <= let_idx {
                i = let_idx + 1;
            }
        }
        hints
    }

    /// Compute type hints for short declarations: `name := expr;`
    #[cfg(test)]
    pub fn compute_define_type_hints(&self, content: &str, range: Range) -> Vec<InlayHint> {
        let (tokens, spans) = match Tokenizer::tokenize_enhanced_with_spans(content) {
            Ok(pair) => pair,
            Err(_) => return Vec::new(),
        };
        self.compute_define_type_hints_from_tokens(&tokens, &spans, range)
    }

    /// Variant that reuses a pre-tokenized buffer for performance.
    pub fn compute_define_type_hints_from_tokens(
        &self,
        tokens: &[token::Token],
        spans: &[Span],
        range: Range,
    ) -> Vec<InlayHint> {
        let mut hints: Vec<InlayHint> = Vec::new();
        use token::Token as T;
        let mut i = 0usize;
        while i + 2 < tokens.len() {
            match (&tokens[i], &tokens[i + 1], &tokens[i + 2]) {
                (T::Id(_), T::Colon, T::Assign) => {
                    // Parse expression from i+3 to next top-level ';'
                    let mut j = i + 3;
                    let mut depth = 0i32;
                    let mut end_expr = j;
                    while j < tokens.len() {
                        match &tokens[j] {
                            T::LParen | T::LBracket | T::LBrace => depth += 1,
                            T::RParen | T::RBracket | T::RBrace => depth -= 1,
                            T::Semicolon if depth == 0 => break,
                            _ => {}
                        }
                        end_expr = j;
                        j += 1;
                    }
                    if end_expr >= i + 3 {
                        let expr_tokens = &tokens[i + 3..=end_expr];
                        if let Ok(expr) = ExprParser::new(expr_tokens).parse() {
                            let mut checker = TypeChecker::new_strict();
                            if let Ok(typ) = checker.infer_resolved_type(&expr) {
                                if i < spans.len() {
                                    let sp = &spans[i];
                                    let pos = Position::new(sp.end.line - 1, sp.end.column.saturating_sub(1));
                                    if pos.line >= range.start.line && pos.line <= range.end.line {
                                        let label = format!(": {}", typ.display());
                                        hints.push(InlayHint {
                                            position: pos,
                                            label: InlayHintLabel::from(label),
                                            kind: Some(InlayHintKind::TYPE),
                                            text_edits: None,
                                            tooltip: None,
                                            padding_left: Some(true),
                                            padding_right: Some(false),
                                            data: None,
                                        });
                                    }
                                }
                            }
                        }
                    }
                    // Advance to next ';'
                    i = j;
                    while i < tokens.len() && !matches!(tokens[i], T::Semicolon) {
                        i += 1;
                    }
                    if i < tokens.len() {
                        i += 1;
                    }
                }
                _ => i += 1,
            }
        }
        hints
    }

    /// Compute type inlay hints for function return types: place a TYPE hint like `-> Int`
    /// after the parameter list. If multiple return statements exist (e.g., branches),
    /// the displayed type is a union of all discovered return expression types.
    #[cfg(test)]
    pub fn compute_function_return_type_hints(&self, content: &str, range: Range) -> Vec<InlayHint> {
        let (tokens, spans) = match Tokenizer::tokenize_enhanced_with_spans(content) {
            Ok(pair) => pair,
            Err(_) => return Vec::new(),
        };
        self.compute_function_return_type_hints_from_tokens(&tokens, &spans, range)
    }

    /// Variant that reuses a pre-tokenized buffer for performance.
    pub fn compute_function_return_type_hints_from_tokens(
        &self,
        tokens: &[token::Token],
        spans: &[Span],
        range: Range,
    ) -> Vec<InlayHint> {
        let mut hints: Vec<InlayHint> = Vec::new();
        use token::Token as T;
        let mut i = 0usize;
        while i < tokens.len() {
            if !matches!(tokens[i], T::Fn) {
                i += 1;
                continue;
            }
            // fn name ( params ) { body }
            let mut j = i + 1;
            // Skip function name if present
            if matches!(tokens.get(j), Some(T::Id(_))) {
                j += 1;
            } else {
                i += 1;
                continue;
            }
            // Expect parameter list
            if !matches!(tokens.get(j), Some(T::LParen)) {
                i += 1;
                continue;
            }
            let mut depth = 0i32;
            // find matching ')'
            while j < tokens.len() {
                match &tokens[j] {
                    T::LParen => depth += 1,
                    T::RParen => {
                        depth -= 1;
                        if depth == 0 {
                            j += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            let rparen_idx = j.saturating_sub(1);
            // Expect function body starting '{'
            if !matches!(tokens.get(j), Some(T::LBrace)) {
                i = j;
                continue;
            }
            // Find matching '}' for the body
            let mut body_depth = 0i32;
            let body_start = j + 1; // after '{'
            j += 1;
            let mut body_end = body_start;
            while j < tokens.len() {
                match &tokens[j] {
                    T::LBrace => body_depth += 1,
                    T::RBrace => {
                        if body_depth == 0 {
                            body_end = j;
                            break;
                        }
                        body_depth -= 1;
                    }
                    _ => {}
                }
                j += 1;
            }
            if body_end <= body_start {
                i = j + 1;
                continue;
            }
            // Within body, scan for all `return <expr>;` occurrences (including inside branches)
            let mut k = body_start;
            let mut return_types: Vec<val::Type> = Vec::new();
            while k < body_end {
                if matches!(tokens[k], T::Return) {
                    // capture expression until next top-level `;` relative to paren/brace depth of this expression
                    let mut e = k + 1;
                    let mut expr_depth = 0i32;
                    let mut last = e;
                    while e < body_end {
                        match &tokens[e] {
                            T::LParen | T::LBracket | T::LBrace => expr_depth += 1,
                            T::RParen | T::RBracket | T::RBrace => expr_depth -= 1,
                            T::Semicolon if expr_depth == 0 => break,
                            _ => {}
                        }
                        last = e;
                        e += 1;
                    }
                    if last > k {
                        let expr_tokens = &tokens[k + 1..=last];
                        if !expr_tokens.is_empty() {
                            if let Ok(expr) = ast::Parser::new(expr_tokens).parse() {
                                let mut checker = typ::TypeChecker::new_strict();
                                if let Ok(ret_ty) = checker.infer_resolved_type(&expr) {
                                    return_types.push(ret_ty);
                                }
                            }
                        }
                    }
                    // Advance past this statement terminator if present
                    k = e + 1;
                    continue;
                }
                k += 1;
            }

            if !return_types.is_empty() {
                // Deduplicate by display string for stable union label
                use std::collections::BTreeMap;
                let mut by_key: BTreeMap<String, val::Type> = BTreeMap::new();
                for t in return_types {
                    by_key.entry(t.display()).or_insert(t);
                }
                let parts: Vec<String> = by_key.into_keys().collect();
                let label = if parts.len() == 1 {
                    format!(" -> {}", parts[0])
                } else {
                    format!(" -> {}", parts.join(" | "))
                };

                // Place hint right after the parameter list, at the end of ')'
                if rparen_idx < spans.len() {
                    let sp = &spans[rparen_idx];
                    let pos = Position::new(sp.end.line - 1, sp.end.column.saturating_sub(1));
                    if pos.line >= range.start.line && pos.line <= range.end.line {
                        hints.push(InlayHint {
                            position: pos,
                            label: InlayHintLabel::from(label),
                            kind: Some(InlayHintKind::TYPE),
                            text_edits: None,
                            tooltip: None,
                            padding_left: Some(true),
                            padding_right: Some(false),
                            data: None,
                        });
                    }
                }
            }
            i = j + 1;
        }
        hints
    }

    /// Clear caches - useful when memory usage becomes high
    pub fn clear_caches(&mut self) {
        self.token_cache.clear();
        self.completion_cache = None;
    }

    /// Set the base directory used for resolving file imports
    pub fn set_base_dir(&mut self, base: PathBuf) {
        self.base_dir = Some(base);
    }

    /// Scan tokens to add diagnostics for unknown stdlib modules and unknown exports with precise spans
    fn add_import_diagnostics(&self, tokens: &[token::Token], spans: &[Span], result: &mut AnalysisResult) {
        use token::Token as T;

        let mut i = 0usize;
        while i < tokens.len() {
            match &tokens[i] {
                T::Import => {
                    let mut j = i + 1;
                    match tokens.get(j) {
                        Some(T::Str(path)) => {
                            // import "file"; -> check existence
                            let exists = self.file_exists(path);
                            if !exists {
                                // Diagnostic on the string span (includes quotes)
                                if j < spans.len() {
                                    let sp = &spans[j];
                                    let range = Range::new(
                                        Position::new(sp.start.line - 1, sp.start.column.saturating_sub(1)),
                                        Position::new(sp.end.line - 1, sp.end.column.saturating_sub(1)),
                                    );
                                    let mut d = Diagnostic::new(
                                        range,
                                        Some(DiagnosticSeverity::ERROR),
                                        None,
                                        Some("lkr".to_string()),
                                        format!("File not found: {}", path),
                                        None,
                                        None,
                                    );
                                    d.code = Some(NumberOrString::String("lkr_file_not_found".to_string()));
                                    result.diagnostics.push(d);
                                }
                            }
                            // advance to ';'
                            while j < tokens.len() && !matches!(tokens[j], T::Semicolon) {
                                j += 1;
                            }
                            i = j + 1;
                            continue;
                        }
                        Some(T::LBrace) => {
                            // import { a, b as c } from module;
                            j += 1; // after '{'
                            let mut item_indices: Vec<usize> = Vec::new();
                            while j < tokens.len() {
                                match &tokens[j] {
                                    T::RBrace => {
                                        j += 1;
                                        break;
                                    }
                                    T::Id(_) => {
                                        // record the exported name id position (before any 'as')
                                        let id_idx = j;
                                        item_indices.push(id_idx);
                                        j += 1;
                                        // Skip optional 'as alias'
                                        if matches!(tokens.get(j), Some(T::As)) {
                                            j += 1;
                                            if matches!(tokens.get(j), Some(T::Id(_))) {
                                                j += 1;
                                            }
                                        }
                                    }
                                    T::Comma => j += 1,
                                    _ => j += 1,
                                }
                            }
                            // Expect 'from' then module id
                            while j < tokens.len() && !matches!(tokens[j], T::From) {
                                j += 1;
                            }
                            if j + 1 < tokens.len() {
                                j += 1; // move to module id
                                if let T::Id(mod_name) = &tokens[j] {
                                    if self.registry.get_module(mod_name).is_ok() {
                                        // Validate each item against module exports
                                        if let Ok(m) = self.registry.get_module(mod_name) {
                                            let exports = m.exports();
                                            for idx in item_indices {
                                                if let T::Id(item_name) = &tokens[idx] {
                                                    if !exports.contains_key(item_name) && idx < spans.len() {
                                                        let sp = &spans[idx];
                                                        let range = Range::new(
                                                            Position::new(
                                                                sp.start.line - 1,
                                                                sp.start.column.saturating_sub(1),
                                                            ),
                                                            Position::new(
                                                                sp.end.line - 1,
                                                                sp.end.column.saturating_sub(1),
                                                            ),
                                                        );
                                                        result.diagnostics.push(Diagnostic::new(
                                                            range,
                                                            Some(DiagnosticSeverity::ERROR),
                                                            None,
                                                            Some("lkr".to_string()),
                                                            format!(
                                                                "Unknown export '{}' from module '{}'",
                                                                item_name, mod_name
                                                            ),
                                                            None,
                                                            None,
                                                        ));
                                                    }
                                                }
                                            }
                                        }
                                    } else if j < spans.len() {
                                        let sp = &spans[j];
                                        let range = Range::new(
                                            Position::new(sp.start.line - 1, sp.start.column.saturating_sub(1)),
                                            Position::new(sp.end.line - 1, sp.end.column.saturating_sub(1)),
                                        );
                                        result.diagnostics.push(Diagnostic::new(
                                            range,
                                            Some(DiagnosticSeverity::ERROR),
                                            None,
                                            Some("lkr".to_string()),
                                            format!("Unknown module: {}", mod_name),
                                            None,
                                            None,
                                        ));
                                    }
                                }
                            }
                            // advance to semicolon
                            while j < tokens.len() && !matches!(tokens[j], T::Semicolon) {
                                j += 1;
                            }
                            i = j + 1;
                            continue;
                        }
                        Some(T::Mul) => {
                            // import * as alias from module;
                            // seek 'from' then module id
                            while j < tokens.len() && !matches!(tokens[j], T::From) {
                                j += 1;
                            }
                            if j + 1 < tokens.len() {
                                j += 1;
                                if let T::Id(mod_name) = &tokens[j] {
                                    if self.registry.get_module(mod_name).is_err() && j < spans.len() {
                                        let sp = &spans[j];
                                        let range = Range::new(
                                            Position::new(sp.start.line - 1, sp.start.column.saturating_sub(1)),
                                            Position::new(sp.end.line - 1, sp.end.column.saturating_sub(1)),
                                        );
                                        result.diagnostics.push(Diagnostic::new(
                                            range,
                                            Some(DiagnosticSeverity::ERROR),
                                            None,
                                            Some("lkr".to_string()),
                                            format!("Unknown module: {}", mod_name),
                                            None,
                                            None,
                                        ));
                                    }
                                }
                            }
                            // advance to semicolon
                            while j < tokens.len() && !matches!(tokens[j], T::Semicolon) {
                                j += 1;
                            }
                            i = j + 1;
                            continue;
                        }
                        Some(T::Id(mod_name)) => {
                            // import module [as alias]?;
                            let mod_idx = j;
                            if self.registry.get_module(mod_name).is_err() && mod_idx < spans.len() {
                                let sp = &spans[mod_idx];
                                let range = Range::new(
                                    Position::new(sp.start.line - 1, sp.start.column.saturating_sub(1)),
                                    Position::new(sp.end.line - 1, sp.end.column.saturating_sub(1)),
                                );
                                result.diagnostics.push(Diagnostic::new(
                                    range,
                                    Some(DiagnosticSeverity::ERROR),
                                    None,
                                    Some("lkr".to_string()),
                                    format!("Unknown module: {}", mod_name),
                                    None,
                                    None,
                                ));
                            }
                            // move to ';'
                            while j < tokens.len() && !matches!(tokens[j], T::Semicolon) {
                                j += 1;
                            }
                            i = j + 1;
                            continue;
                        }
                        _ => {}
                    }

                    i = j + 1;
                }
                _ => {
                    i += 1;
                }
            }
        }
    }

    fn file_exists(&self, rel: &str) -> bool {
        // Absolute path: use as-is
        let path = Path::new(rel);
        if path.is_absolute() {
            return path.exists();
        }
        let base = self.base_dir.as_ref().cloned().unwrap_or_else(|| PathBuf::from("."));
        let candidates = [base.clone(), base.join("lib"), base.join("modules")];
        for dir in candidates.iter() {
            let p = dir.join(rel);
            if p.exists() {
                return true;
            }
            // Try with .lkr appended if missing extension
            if p.extension().is_none() {
                let with_ext = p.with_extension("lkr");
                if with_ext.exists() {
                    return true;
                }
            }
        }
        false
    }

    /// Tokenize with spans, using an internal cache keyed by full content string.
    pub(crate) fn tokenize_with_spans_cached(
        &mut self,
        content: &str,
    ) -> std::result::Result<Arc<TokenCacheEntry>, token::ParseError> {
        if let Some(cached) = self.token_cache.get(content) {
            return Ok(cached.clone());
        }
        let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(content)?;
        let entry = Arc::new(TokenCacheEntry::new(tokens, spans));
        if content.len() < 10_000 {
            if self.token_cache.len() >= 100 {
                self.token_cache.clear();
            }
            self.token_cache.insert(content.to_string(), entry.clone());
        }
        Ok(entry)
    }

    /// Analyze LKR code and return diagnostics, symbols, and identifier roots
    pub fn analyze(&mut self, content: &str) -> AnalysisResult {
        let mut result = AnalysisResult {
            diagnostics: Vec::new(),
            symbols: Vec::new(),
            identifier_roots: HashSet::new(),
        };

        // Try parsing as expression first - use cached tokenization if available
        let token_entry = match self.tokenize_with_spans_cached(content) {
            Ok(entry) => entry,
            Err(parse_err) => {
                if content.lines().count() > 1 {
                    let diags = self.scan_lines_for_diagnostics(content);
                    if !diags.is_empty() {
                        result.diagnostics = diags;
                        return result;
                    }
                }

                let range = if let Some(span) = &parse_err.span {
                    let start_pos = Position::new(span.start.line - 1, span.start.column - 1);
                    let end_pos = Position::new(span.end.line - 1, span.end.column - 1);
                    Range::new(start_pos, end_pos)
                } else {
                    Range::new(Position::new(0, 0), Position::new(0, content.len() as u32))
                };

                result.diagnostics.push(Diagnostic::new(
                    range,
                    Some(DiagnosticSeverity::ERROR),
                    None,
                    Some("lkr".to_string()),
                    format!("Tokenization error: {}", parse_err.message),
                    None,
                    None,
                ));
                return result;
            }
        };
        let tokens = token_entry.tokens.as_ref();
        let spans = token_entry.spans.as_ref();

        match token_entry.parse_expression_arc(content) {
            Ok(expr_arc) => {
                let expr = expr_arc.as_ref();
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

                // Add identifier validation diagnostics if we can parse the expression again for validation
                let id_diagnostics = self.validate_identifier_access(expr, None);
                result.diagnostics.extend(id_diagnostics);

                // Even for expressions, scan for import diagnostics (typically none)
                self.add_import_diagnostics(tokens, spans, &mut result);
                // Named-args diagnostics on expressions that contain calls
                let nad = self.collect_named_call_diagnostics(content, tokens, spans);
                if !nad.is_empty() {
                    result.diagnostics.extend(nad);
                }
            }
            Err(expr_err) => {
                // Attempt expression-level recovery to surface multiple errors for pure expressions
                let expr_recover_errors = ExprParser::recover_expression_errors(tokens, spans, content);
                // Try parsing as statement program
                match token_entry.parse_program_arc(content) {
                    Ok(program_arc) => {
                        let program = program_arc.as_ref();
                        // Analyze statements for symbols and identifier roots
                        self.analyze_statements(&program.statements, &mut result);
                        // Integrate slot-based symbols (parameters/locals) for richer outline
                        let mut resolver = SlotResolver::new();
                        let resolution = resolver.resolve_program_slots(program);
                        // Enrich slot layout with scanned source spans for precise symbol ranges
                        let enriched = self.enrich_layout_spans(&resolution.root, tokens, spans);
                        // Top-level variable declarations (outside functions), grouped
                        let top_level_vars = Self::collect_decl_symbols(&enriched);
                        if !top_level_vars.is_empty() {
                            // Keep individual variables at top-level for backward compatibility
                            result.symbols.extend(top_level_vars.clone());
                            let (range_start, range_end) = (
                                top_level_vars
                                    .first()
                                    .map(|s| s.range.start)
                                    .unwrap_or(Position::new(0, 0)),
                                top_level_vars
                                    .last()
                                    .map(|s| s.range.end)
                                    .unwrap_or(Position::new(0, 0)),
                            );
                            let vars_container = DocumentSymbol {
                                name: "Variables".to_string(),
                                detail: None,
                                kind: SymbolKind::NAMESPACE,
                                tags: None,
                                #[allow(deprecated)]
                                deprecated: None,
                                range: Range::new(range_start, range_end),
                                selection_range: Range::new(range_start, range_start),
                                children: Some(top_level_vars),
                            };
                            result.symbols.push(vars_container);
                        }

                        // Top-level imports grouped
                        let import_syms = Self::collect_import_symbols_via_tokens(tokens, spans);
                        if !import_syms.is_empty() {
                            // Keep individual imports at top-level for backward compatibility
                            result.symbols.extend(import_syms.clone());
                            let (range_start, range_end) = (
                                import_syms
                                    .first()
                                    .map(|s| s.range.start)
                                    .unwrap_or(Position::new(0, 0)),
                                import_syms.last().map(|s| s.range.end).unwrap_or(Position::new(0, 0)),
                            );
                            let imports_container = DocumentSymbol {
                                name: "Imports".to_string(),
                                detail: None,
                                kind: SymbolKind::NAMESPACE,
                                tags: None,
                                #[allow(deprecated)]
                                deprecated: None,
                                range: Range::new(range_start, range_end),
                                selection_range: Range::new(range_start, range_start),
                                children: Some(import_syms),
                            };
                            result.symbols.push(imports_container);
                        }
                        // Add function symbols (nested hierarchy) using scanned blocks + enriched layouts
                        let fblocks = Self::scan_function_blocks(tokens, spans);
                        let (parents, children) = Self::compute_fn_block_hierarchy(&fblocks);
                        // Top-level functions in source order
                        let mut top_indices: Vec<usize> =
                            (0..fblocks.len()).filter(|&i| parents[i].is_none()).collect();
                        // Preserve source order as in fblocks
                        top_indices.sort();
                        for (top_ord, i) in top_indices.iter().enumerate() {
                            let layout_opt = enriched.children.get(top_ord);
                            let sym =
                                Self::build_function_symbol_tree(&fblocks, &children, *i, layout_opt, tokens, spans);
                            result.symbols.push(sym);
                        }

                        // Labels syntax is not supported; no label symbols at top-level
                        // Add precise import diagnostics using tokens/spans
                        self.add_import_diagnostics(tokens, spans, &mut result);

                        // Run type checking to surface semantic diagnostics (e.g., numeric operand errors)
                        let type_diags = Self::collect_type_diagnostics(program, tokens, spans, content);
                        if !type_diags.is_empty() {
                            result.diagnostics.extend(type_diags);
                        }
                    }
                    Err(stmt_err) => {
                        // If we found expression-level errors and the content doesn't look like statements,
                        // prefer reporting these expression diagnostics.
                        let mut collected: Vec<Diagnostic> = Vec::new();
                        let has_statement_keywords = content.contains("let ")
                            || content.contains("if ")
                            || content.contains("while ")
                            || content.contains("return ")
                            || content.contains("break")
                            || content.contains("continue");
                        if !expr_recover_errors.is_empty() && !has_statement_keywords {
                            for e in expr_recover_errors {
                                let range = if let Some(span) = &e.span {
                                    let start_pos = Position::new(span.start.line - 1, span.start.column - 1);
                                    let end_pos = Position::new(span.end.line - 1, span.end.column - 1);
                                    Range::new(start_pos, end_pos)
                                } else {
                                    Range::new(Position::new(0, 0), Position::new(0, content.len() as u32))
                                };
                                collected.push(Diagnostic::new(
                                    range,
                                    Some(DiagnosticSeverity::ERROR),
                                    None,
                                    Some("lkr".to_string()),
                                    e.message,
                                    None,
                                    None,
                                ));
                            }
                        }

                        // First, attempt recovering parse to collect multiple errors with precise spans
                        let mut recover_parser = StmtParser::new_with_spans(tokens, spans);
                        let (stmts, errs) = recover_parser.parse_program_recovering_with_enhanced_errors(content);
                        if !errs.is_empty() {
                            for e in errs {
                                let range = if let Some(span) = &e.span {
                                    let start_pos = Position::new(span.start.line - 1, span.start.column - 1);
                                    let end_pos = Position::new(span.end.line - 1, span.end.column - 1);
                                    Range::new(start_pos, end_pos)
                                } else {
                                    Range::new(Position::new(0, 0), Position::new(0, content.len() as u32))
                                };
                                collected.push(Diagnostic::new(
                                    range,
                                    Some(DiagnosticSeverity::ERROR),
                                    None,
                                    Some("lkr".to_string()),
                                    e.message,
                                    None,
                                    None,
                                ));
                            }
                            // Even with errors, analyze statements to surface symbols and identifier roots
                            self.analyze_statements(&stmts, &mut result);
                            // And add precise import diagnostics using tokens/spans
                            self.add_import_diagnostics(tokens, spans, &mut result);
                            // Named-args diagnostics (best-effort on partially parsed code)
                            let nad = self.collect_named_call_diagnostics(content, tokens, spans);
                            if !nad.is_empty() {
                                result.diagnostics.extend(nad);
                            }
                        }

                        // If recovery yielded nothing (e.g., single token), try chunk-based scan then line-wise
                        if collected.is_empty() {
                            collected = self.scan_chunks_for_diagnostics(content);
                            if collected.is_empty() {
                                collected = self.scan_lines_for_diagnostics(content);
                            }
                        }

                        // If line scanning found nothing (e.g., single-line expression-like input),
                        // fall back to reporting the most relevant single error.
                        if collected.is_empty() {
                            // Both parsing attempts failed - prefer statement error for code containing statement keywords
                            let has_statement_keywords = content.contains("let ")
                                || content.contains("if ")
                                || content.contains("while ")
                                || content.contains("return ")
                                || content.contains("break")
                                || content.contains("continue");
                            let parse_err = if has_statement_keywords { &stmt_err } else { &expr_err };

                            let range = if let Some(span) = &parse_err.span {
                                let start_pos = Position::new(span.start.line - 1, span.start.column - 1);
                                let end_pos = Position::new(span.end.line - 1, span.end.column - 1);
                                Range::new(start_pos, end_pos)
                            } else {
                                Range::new(Position::new(0, 0), Position::new(0, content.len() as u32))
                            };

                            collected.push(Diagnostic::new(
                                range,
                                Some(DiagnosticSeverity::ERROR),
                                None,
                                Some("lkr".to_string()),
                                parse_err.message.clone(),
                                None,
                                None,
                            ));
                        }

                        result.diagnostics.extend(collected);
                        // Also attempt import diagnostics if tokens parsed
                        self.add_import_diagnostics(tokens, spans, &mut result);
                    }
                }
            }
        }

        // Run strict type checking when parsing succeeded to surface semantic diagnostics
        if result.diagnostics.is_empty() {
            if let Ok((tokens, spans)) = Tokenizer::tokenize_enhanced_with_spans(content) {
                let mut parser = StmtParser::new_with_spans(&tokens, &spans);
                if let Ok(program) = parser.parse_program_with_enhanced_errors(content) {
                    let has_complex_items = program
                        .statements
                        .iter()
                        .any(|stmt| matches!(stmt.as_ref(), Stmt::Import(_) | Stmt::Function { .. }));
                    if has_complex_items {
                        // Skip type checking when imports/functions are present since additional context is required.
                        // TODO: enrich analyzer with module resolution to support complex programs.
                        self.dedup_diagnostics(&mut result.diagnostics);
                        return result;
                    }
                    let mut checker = TypeChecker::new_strict();
                    if let Err(err) = program.type_check(&mut checker) {
                        let mut diag = Diagnostic::new(
                            Range::new(Position::new(0, 0), Position::new(0, 0)),
                            Some(DiagnosticSeverity::ERROR),
                            None,
                            Some("lkr".to_string()),
                            err.to_string(),
                            None,
                            None,
                        );
                        if diag.range.end.line == 0 && diag.range.end.character == 0 {
                            diag.range = Range::new(
                                Position::new(0, 0),
                                Position::new(0, content.lines().next().map_or(0, |l| l.len() as u32)),
                            );
                        }
                        result.diagnostics.push(diag);
                    }
                }
            }
        }

        // Deduplicate diagnostics by range and message to reduce noise
        self.dedup_diagnostics(&mut result.diagnostics);

        result
    }

    /// Build a new FunctionLayout tree with decl spans populated by scanning tokens.
    /// Heuristics: assigns spans in source order matching names to declarations in the resolver order.
    pub(crate) fn enrich_layout_spans(
        &self,
        layout: &FunctionLayout,
        tokens: &[token::Token],
        spans: &[Span],
    ) -> FunctionLayout {
        // Scan function blocks (including nested) and correlate with layouts
        let fblocks = Self::scan_function_blocks(tokens, spans);
        let (parents, children_map) = Self::compute_fn_block_hierarchy(&fblocks);

        // Top-level declarations outside function blocks + function names as top-level binds
        let toplevel_decl_spans = Self::scan_toplevel_decl_spans(tokens, spans, &fblocks);

        // Helper to assign spans to decls from a queue per name
        fn assign_spans(
            mut decls: Vec<resolve::slots::Decl>,
            pool: &mut HashMap<String, Vec<Span>>,
        ) -> Vec<resolve::slots::Decl> {
            for d in decls.iter_mut() {
                if let Some(list) = pool.get_mut(&d.name) {
                    if !list.is_empty() {
                        d.span = Some(list.remove(0));
                    }
                }
            }
            decls
        }

        // Prepare a toplevel pool by name
        let mut top_pool: HashMap<String, Vec<Span>> = HashMap::new();
        for (name, sp) in toplevel_decl_spans {
            top_pool.entry(name).or_default().push(sp);
        }
        let mut new_root = FunctionLayout {
            decls: assign_spans(layout.decls.clone(), &mut top_pool),
            total_locals: layout.total_locals,
            uses: layout.uses.clone(),
            children: Vec::new(),
        };

        // Signature helpers
        fn layout_param_signature(layout: &FunctionLayout) -> Vec<String> {
            let mut params: Vec<(usize, String)> = layout
                .decls
                .iter()
                .filter(|d| d.is_param)
                .map(|d| (d.index as usize, d.name.clone()))
                .collect();
            params.sort_by_key(|(i, _)| *i);
            params.into_iter().map(|(_, n)| n).collect()
        }
        fn fblock_param_signature(fb: &FnBlockInfo) -> Vec<String> {
            fb.param_spans.iter().map(|(n, _)| n.clone()).collect()
        }
        fn fb_locals_pool(tokens: &[token::Token], spans: &[Span], fb: &FnBlockInfo) -> HashMap<String, Vec<Span>> {
            let mut pool: HashMap<String, Vec<Span>> = HashMap::new();
            for (pname, pspan) in fb.param_spans.iter() {
                pool.entry(pname.clone()).or_default().push(pspan.clone());
            }
            let locals = LkrAnalyzer::scan_decl_spans_in_range(tokens, spans, fb.body_start_idx, fb.body_end_idx);
            for (n, sp) in locals {
                pool.entry(n).or_default().push(sp);
            }
            pool
        }

        // Align a list of child layouts to a set of function block indices in order
        fn align_children(
            layouts: &[FunctionLayout],
            fb_indices: &[usize],
            fblocks: &[FnBlockInfo],
        ) -> Vec<Option<usize>> {
            let mut used = vec![false; layouts.len()];
            let mut mapping: Vec<Option<usize>> = vec![None; fb_indices.len()];
            for (pos, &fi) in fb_indices.iter().enumerate() {
                let fb_sig = fblock_param_signature(&fblocks[fi]);
                let mut best: Option<(usize, i32)> = None; // (layout_idx, score)
                for (li, lay) in layouts.iter().enumerate() {
                    if used[li] {
                        continue;
                    }
                    let lsig = layout_param_signature(lay);
                    let score = if lsig == fb_sig {
                        1000 + lsig.len() as i32
                    } else if lsig.len() == fb_sig.len() {
                        100 + lsig.iter().zip(fb_sig.iter()).filter(|(a, b)| *a == *b).count() as i32
                    } else {
                        lsig.iter().filter(|n| fb_sig.contains(n)).count() as i32
                    };
                    if best.map(|(_, s)| score > s).unwrap_or(true) {
                        best = Some((li, score));
                    }
                }
                if let Some((li, _)) = best {
                    used[li] = true;
                    mapping[pos] = Some(li);
                }
            }
            mapping
        }

        // Build top-level children in source order
        let mut top_indices: Vec<usize> = (0..fblocks.len()).filter(|&i| parents[i].is_none()).collect();
        top_indices.sort();
        let top_mapping = align_children(&layout.children, &top_indices, &fblocks);

        let mut built_children: Vec<FunctionLayout> = Vec::new();
        for (ord, &maybe_li) in top_mapping.iter().enumerate() {
            let fb_idx = top_indices[ord];
            let fb = &fblocks[fb_idx];
            let mut pool = fb_locals_pool(tokens, spans, fb);
            let base = maybe_li
                .and_then(|li| layout.children.get(li))
                .cloned()
                .unwrap_or_else(|| FunctionLayout {
                    decls: Vec::new(),
                    total_locals: 0,
                    uses: Vec::new(),
                    children: Vec::new(),
                });
            let mut enriched_child = FunctionLayout {
                decls: assign_spans(base.decls, &mut pool),
                total_locals: base.total_locals,
                uses: base.uses,
                children: Vec::new(),
            };

            // Nested children alignment
            let child_fb_indices = children_map.get(fb_idx).cloned().unwrap_or_default();
            let child_mapping = align_children(&base.children, &child_fb_indices, &fblocks);
            let mut nested_children: Vec<FunctionLayout> = Vec::new();
            for (cpos, &maybe_cli) in child_mapping.iter().enumerate() {
                let cfi = child_fb_indices[cpos];
                let cfb = &fblocks[cfi];
                let mut cpool = fb_locals_pool(tokens, spans, cfb);
                let cbase = maybe_cli
                    .and_then(|li| base.children.get(li))
                    .cloned()
                    .unwrap_or_else(|| FunctionLayout {
                        decls: Vec::new(),
                        total_locals: 0,
                        uses: Vec::new(),
                        children: Vec::new(),
                    });
                let cenriched = FunctionLayout {
                    decls: assign_spans(cbase.decls, &mut cpool),
                    total_locals: cbase.total_locals,
                    uses: cbase.uses,
                    children: Vec::new(),
                };
                nested_children.push(cenriched);
            }
            enriched_child.children = nested_children;
            built_children.push(enriched_child);
        }
        new_root.children = built_children;
        new_root
    }
}
