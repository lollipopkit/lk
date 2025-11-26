use super::*;

impl LkrAnalyzer {
    /// Scan function blocks in source order: name, name span, body token range, and param spans.
    pub(crate) fn scan_function_blocks(tokens: &[token::Token], spans: &[Span]) -> Vec<FnBlockInfo> {
        use token::Token as T;
        let mut i = 0usize;
        let mut out: Vec<FnBlockInfo> = Vec::new();
        while i < tokens.len() {
            if !matches!(tokens[i], T::Fn) {
                i += 1;
                continue;
            }
            // Expect function name
            if i + 1 >= tokens.len() {
                break;
            }
            let name = if let T::Id(ref n) = tokens[i + 1] {
                n.clone()
            } else {
                i += 1;
                continue;
            };
            let name_span = match spans.get(i + 1).cloned() {
                Some(sp) => sp,
                None => match spans.get(i).cloned() {
                    Some(sp) => sp,
                    None => continue,
                },
            };
            // Find params region: '(' ... matching ')'
            let mut j = i + 2;
            if j >= tokens.len() || !matches!(tokens[j], T::LParen) {
                i += 1;
                continue;
            }
            let mut paren = 1i32;
            let mut params: Vec<(String, Span)> = Vec::new();
            j += 1;
            while j < tokens.len() && paren > 0 {
                match &tokens[j] {
                    T::LParen => paren += 1,
                    T::RParen => paren -= 1,
                    T::Id(p) if paren == 1 => {
                        if let Some(sp) = spans.get(j) {
                            params.push((p.clone(), sp.clone()));
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            // After params, optional '->' and type, then expect '{'
            while j < tokens.len() && !matches!(tokens[j], T::LBrace) {
                j += 1;
            }
            if j >= tokens.len() || !matches!(tokens[j], T::LBrace) {
                i = j;
                continue;
            }
            // Find matching '}' for body
            let mut brace = 1i32;
            let body_start = j; // points to '{'
            j += 1;
            while j < tokens.len() && brace > 0 {
                match tokens[j] {
                    T::LBrace => brace += 1,
                    T::RBrace => brace -= 1,
                    _ => {}
                }
                j += 1;
            }
            let body_end = j.saturating_sub(1); // index of '}'
            out.push(FnBlockInfo {
                name,
                name_span,
                body_start_idx: body_start,
                body_end_idx: body_end,
                param_spans: params,
            });
            // Do not skip over the entire body; continue scanning to discover nested functions too
            i += 1;
        }
        out
    }

    /// Scan variable declaration spans within [start_idx, end_idx] token range: let-patterns and short defines.
    pub(crate) fn scan_decl_spans_in_range(
        tokens: &[token::Token],
        spans: &[Span],
        start_idx: usize,
        end_idx: usize,
    ) -> Vec<(String, Span)> {
        use token::Token as T;
        let mut out: Vec<(String, Span)> = Vec::new();
        let mut i = start_idx;
        while i <= end_idx && i < tokens.len() {
            match &tokens[i] {
                T::Let => {
                    // Pattern region until top-level ':' or '='
                    let mut j = i + 1;
                    let mut paren = 0i32;
                    let mut bracket = 0i32;
                    let mut brace = 0i32;
                    while j <= end_idx && j < tokens.len() {
                        match tokens[j] {
                            T::LParen => paren += 1,
                            T::RParen => paren -= 1,
                            T::LBracket => bracket += 1,
                            T::RBracket => bracket -= 1,
                            T::LBrace => brace += 1,
                            T::RBrace => brace -= 1,
                            T::Assign | T::Colon if paren == 0 && bracket == 0 && brace == 0 => break,
                            _ => {}
                        }
                        j += 1;
                    }
                    // Within [i+1, j), collect identifier tokens as declarations
                    let mut k = i + 1;
                    while k < j && k <= end_idx {
                        if let T::Id(ref n) = tokens[k] {
                            if let Some(sp) = spans.get(k) {
                                out.push((n.clone(), sp.clone()))
                            }
                        }
                        k += 1;
                    }
                    i = j;
                }
                T::Id(name) => {
                    // Short define: id := expr ;
                    if i + 2 <= end_idx
                        && matches!(tokens.get(i + 1), Some(T::Colon))
                        && matches!(tokens.get(i + 2), Some(T::Assign))
                    {
                        if let Some(sp) = spans.get(i) {
                            out.push((name.clone(), sp.clone()));
                        }
                        i += 3;
                        continue;
                    }
                    i += 1;
                }
                _ => i += 1,
            }
        }
        out
    }

    /// Collect top-level declaration spans and function names (outside any function block body).
    pub(crate) fn scan_toplevel_decl_spans(
        tokens: &[token::Token],
        spans: &[Span],
        fblocks: &Vec<FnBlockInfo>,
    ) -> Vec<(String, Span)> {
        use token::Token as T;
        let mut out: Vec<(String, Span)> = Vec::new();
        // Function names are top-level bindings
        for fb in fblocks {
            out.push((fb.name.clone(), fb.name_span.clone()));
        }
        // Scan all tokens skipping over function bodies
        let mut skip_ranges: Vec<(usize, usize)> =
            fblocks.iter().map(|fb| (fb.body_start_idx, fb.body_end_idx)).collect();
        skip_ranges.sort_by_key(|r| r.0);
        let mut i = 0usize;
        let mut ri = 0usize;
        while i < tokens.len() {
            if ri < skip_ranges.len() {
                let (s, e) = skip_ranges[ri];
                if i >= s && i <= e {
                    i = e + 1;
                    ri += 1;
                    continue;
                }
            }
            match &tokens[i] {
                T::Let => {
                    // As in range scan
                    let mut j = i + 1;
                    let mut paren = 0i32;
                    let mut bracket = 0i32;
                    let mut brace = 0i32;
                    while j < tokens.len() {
                        match tokens[j] {
                            T::LParen => paren += 1,
                            T::RParen => paren -= 1,
                            T::LBracket => bracket += 1,
                            T::RBracket => bracket -= 1,
                            T::LBrace => brace += 1,
                            T::RBrace => brace -= 1,
                            T::Assign | T::Colon if paren == 0 && bracket == 0 && brace == 0 => break,
                            _ => {}
                        }
                        j += 1;
                    }
                    let mut k = i + 1;
                    while k < j {
                        if let T::Id(ref n) = tokens[k] {
                            if let Some(sp) = spans.get(k) {
                                out.push((n.clone(), sp.clone()));
                            }
                        }
                        k += 1;
                    }
                    i = j;
                }
                T::Id(name) => {
                    if i + 2 < tokens.len()
                        && matches!(tokens.get(i + 1), Some(T::Colon))
                        && matches!(tokens.get(i + 2), Some(T::Assign))
                    {
                        if let Some(sp) = spans.get(i) {
                            out.push((name.clone(), sp.clone()));
                        }
                        i += 3;
                        continue;
                    }
                    i += 1;
                }
                _ => i += 1,
            }
        }
        out
    }

    /// Collect variable symbols (params/locals) for a single function layout.
    /// Does not recurse into nested children; returns symbols to be used as function.children.
    pub(crate) fn collect_decl_symbols(layout: &FunctionLayout) -> Vec<DocumentSymbol> {
        use tower_lsp::lsp_types::{DocumentSymbol, Position, Range, SymbolKind};
        let mut out: Vec<DocumentSymbol> = Vec::new();
        for decl in &layout.decls {
            let detail = if decl.is_param { "Parameter" } else { "Local" };
            let (range, selection_range) = if let Some(sp) = &decl.span {
                let start = Position::new(sp.start.line - 1, sp.start.column.saturating_sub(1));
                let end = Position::new(sp.end.line - 1, sp.end.column.saturating_sub(1));
                (Range::new(start, end), Range::new(start, end))
            } else {
                (
                    Range::new(Position::new(0, 0), Position::new(0, 0)),
                    Range::new(Position::new(0, 0), Position::new(0, 0)),
                )
            };
            out.push(DocumentSymbol {
                name: decl.name.clone(),
                detail: Some(format!("{} (slot #{})", detail, decl.index)),
                kind: SymbolKind::VARIABLE,
                tags: None,
                #[allow(deprecated)]
                deprecated: None,
                range,
                selection_range,
                children: None,
            });
        }
        out
    }

    /// Group params and locals into two container nodes under the function.
    pub(crate) fn collect_decl_groups(layout: &FunctionLayout, func_range: Range) -> Vec<DocumentSymbol> {
        use tower_lsp::lsp_types::{DocumentSymbol, SymbolKind};
        let mut params: Vec<DocumentSymbol> = Vec::new();
        let mut locals: Vec<DocumentSymbol> = Vec::new();
        for sym in Self::collect_decl_symbols(layout) {
            // classify by detail text prefix
            if sym.detail.as_ref().map(|d| d.starts_with("Parameter")).unwrap_or(false) {
                params.push(sym);
            } else {
                locals.push(sym);
            }
        }
        let mut groups: Vec<DocumentSymbol> = Vec::new();
        if !params.is_empty() {
            groups.push(DocumentSymbol {
                name: "Parameters".to_string(),
                detail: None,
                kind: SymbolKind::NAMESPACE,
                tags: None,
                #[allow(deprecated)]
                deprecated: None,
                range: func_range,
                selection_range: func_range,
                children: Some(params),
            });
        }
        if !locals.is_empty() {
            groups.push(DocumentSymbol {
                name: "Locals".to_string(),
                detail: None,
                kind: SymbolKind::NAMESPACE,
                tags: None,
                #[allow(deprecated)]
                deprecated: None,
                range: func_range,
                selection_range: func_range,
                children: Some(locals),
            });
        }
        groups
    }

    /// Collect import symbols via token scanning and produce per-import DocumentSymbols.
    pub(crate) fn collect_import_symbols_via_tokens(tokens: &[token::Token], spans: &[Span]) -> Vec<DocumentSymbol> {
        use token::Token as T;
        use tower_lsp::lsp_types::{DocumentSymbol, Position, Range, SymbolKind};
        let mut out: Vec<DocumentSymbol> = Vec::new();
        let mut i = 0usize;
        while i < tokens.len() {
            if !matches!(tokens[i], T::Import) {
                i += 1;
                continue;
            }
            let start_idx = i;
            let mut j = i + 1;
            let mut label = String::from("import");
            // Derive a short label based on common forms
            if let Some(tok) = tokens.get(j) {
                match tok {
                    T::Str(s) => {
                        label = format!("import \"{}\"", s);
                        j += 1;
                    }
                    T::LBrace => {
                        // skip until 'from' then module id
                        j += 1;
                        while j < tokens.len() && !matches!(tokens[j], T::From) {
                            j += 1;
                        }
                        if j + 1 < tokens.len() {
                            if let T::Id(m) = &tokens[j + 1] {
                                label = format!("import {{…}} from {}", m);
                            } else {
                                label = "import {…}".to_string();
                            }
                        }
                    }
                    T::Id(m) => {
                        // maybe alias form later
                        label = format!("import {}", m);
                        // peek for 'as <alias>'
                        let mut k = j + 1;
                        if matches!(tokens.get(k), Some(T::As)) {
                            k += 1;
                            if let Some(T::Id(a)) = tokens.get(k) {
                                label = format!("import {} as {}", m, a);
                            }
                        }
                    }
                    _ => {}
                }
            }
            // Find end at next ';'
            while j < tokens.len() && !matches!(tokens[j], T::Semicolon) {
                j += 1;
            }
            let end_idx = j.min(tokens.len().saturating_sub(1));
            if let (Some(s0), Some(se)) = (spans.get(start_idx), spans.get(end_idx)) {
                let range = Range::new(
                    Position::new(s0.start.line - 1, s0.start.column.saturating_sub(1)),
                    Position::new(se.end.line - 1, se.end.column.saturating_sub(1)),
                );
                out.push(DocumentSymbol {
                    name: label,
                    detail: Some("Import statement".to_string()),
                    kind: SymbolKind::MODULE,
                    tags: None,
                    #[allow(deprecated)]
                    deprecated: None,
                    range,
                    selection_range: range,
                    children: None,
                });
            }
            i = j + 1;
        }
        out
    }

    // Labels are not supported; no label collection helpers

    /// Compute parent and children lists for function blocks based on body containment.
    pub(crate) fn compute_fn_block_hierarchy(fblocks: &[FnBlockInfo]) -> (Vec<Option<usize>>, Vec<Vec<usize>>) {
        let n = fblocks.len();
        let mut parent: Vec<Option<usize>> = vec![None; n];
        for (i, fi) in fblocks.iter().enumerate().take(n) {
            let s_i = fi.body_start_idx;
            let e_i = fi.body_end_idx;
            let mut best: Option<(usize, usize)> = None; // (j, span_len)
            for (j, fj) in fblocks.iter().enumerate().take(n) {
                if i == j {
                    continue;
                }
                let s_j = fj.body_start_idx;
                let e_j = fj.body_end_idx;
                if s_j <= s_i && e_j >= e_i {
                    let span_len = e_j.saturating_sub(s_j);
                    if best.map(|(_, l)| span_len < l).unwrap_or(true) {
                        best = Some((j, span_len));
                    }
                }
            }
            if let Some((pj, _)) = best {
                parent[i] = Some(pj);
            }
        }
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, pi) in parent.iter().enumerate().take(n) {
            if let Some(p) = *pi {
                children[p].push(i);
            }
        }
        (parent, children)
    }

    /// Recursively build a function symbol with nested function children + var/param children.
    pub(crate) fn build_function_symbol_tree(
        fblocks: &[FnBlockInfo],
        children_map: &[Vec<usize>],
        idx: usize,
        layout_opt: Option<&FunctionLayout>,
        tokens: &[token::Token],
        spans: &[Span],
    ) -> DocumentSymbol {
        use tower_lsp::lsp_types::{DocumentSymbol, Position, Range, SymbolKind};
        let fb = &fblocks[idx];
        let name_sp = fb.name_span.clone();
        let body_end = spans.get(fb.body_end_idx).cloned().unwrap_or(name_sp.clone());
        let range = Range::new(
            Position::new(name_sp.start.line - 1, name_sp.start.column.saturating_sub(1)),
            Position::new(body_end.end.line - 1, body_end.end.column.saturating_sub(1)),
        );
        let selection_range = Range::new(
            Position::new(name_sp.start.line - 1, name_sp.start.column.saturating_sub(1)),
            Position::new(name_sp.end.line - 1, name_sp.end.column.saturating_sub(1)),
        );
        let params_label = if fb.param_spans.is_empty() {
            String::new()
        } else {
            let names: Vec<String> = fb.param_spans.iter().map(|(n, _)| n.clone()).collect();
            names.join(", ")
        };
        let mut kids: Vec<DocumentSymbol> = Vec::new();
        // Add variables/params declared in this function grouped
        if let Some(layout) = layout_opt {
            kids.extend(Self::collect_decl_groups(layout, range));
        }
        if kids.is_empty() {
            // Fallback: build groups by scanning tokens (parameters + locals) when layout is unavailable
            let mut params: Vec<DocumentSymbol> = Vec::new();
            for (pname, pspan) in fb.param_spans.iter() {
                let start = Position::new(pspan.start.line - 1, pspan.start.column.saturating_sub(1));
                let end = Position::new(pspan.end.line - 1, pspan.end.column.saturating_sub(1));
                params.push(DocumentSymbol {
                    name: pname.clone(),
                    detail: Some("Parameter".to_string()),
                    kind: SymbolKind::VARIABLE,
                    tags: None,
                    #[allow(deprecated)]
                    deprecated: None,
                    range: Range::new(start, end),
                    selection_range: Range::new(start, end),
                    children: None,
                });
            }
            if !params.is_empty() {
                kids.push(DocumentSymbol {
                    name: "Parameters".to_string(),
                    detail: None,
                    kind: SymbolKind::NAMESPACE,
                    tags: None,
                    #[allow(deprecated)]
                    deprecated: None,
                    range,
                    selection_range: range,
                    children: Some(params),
                });
            }
            let locals = Self::scan_decl_spans_in_range(tokens, spans, fb.body_start_idx, fb.body_end_idx);
            if !locals.is_empty() {
                let mut local_syms: Vec<DocumentSymbol> = Vec::new();
                for (lname, lspan) in locals {
                    let start = Position::new(lspan.start.line - 1, lspan.start.column.saturating_sub(1));
                    let end = Position::new(lspan.end.line - 1, lspan.end.column.saturating_sub(1));
                    local_syms.push(DocumentSymbol {
                        name: lname,
                        detail: Some("Local".to_string()),
                        kind: SymbolKind::VARIABLE,
                        tags: None,
                        #[allow(deprecated)]
                        deprecated: None,
                        range: Range::new(start, end),
                        selection_range: Range::new(start, end),
                        children: None,
                    });
                }
                kids.push(DocumentSymbol {
                    name: "Locals".to_string(),
                    detail: None,
                    kind: SymbolKind::NAMESPACE,
                    tags: None,
                    #[allow(deprecated)]
                    deprecated: None,
                    range,
                    selection_range: range,
                    children: Some(local_syms),
                });
            }
        }
        // Labels syntax is not supported; no function-local label grouping
        // Add nested functions in source order within this function
        let child_idxs = children_map.get(idx).cloned().unwrap_or_default();
        for (ord, child_i) in child_idxs.iter().enumerate() {
            let child_layout_opt = layout_opt.and_then(|l| l.children.get(ord));
            let child_sym =
                Self::build_function_symbol_tree(fblocks, children_map, *child_i, child_layout_opt, tokens, spans);
            kids.push(child_sym);
        }
        // Try to infer return type for function detail
        let detail = if let Some(ret) = Self::infer_fn_return_type_for_block(tokens, fb) {
            if params_label.is_empty() {
                format!("fn() -> {}", ret)
            } else {
                format!("fn({}) -> {}", params_label, ret)
            }
        } else if params_label.is_empty() {
            "Function".to_string()
        } else {
            format!("Function({})", params_label)
        };
        DocumentSymbol {
            name: fb.name.clone(),
            detail: Some(detail),
            kind: SymbolKind::FUNCTION,
            tags: None,
            #[allow(deprecated)]
            deprecated: None,
            range,
            selection_range,
            children: if kids.is_empty() { None } else { Some(kids) },
        }
    }

    /// Infer a function's return type by scanning return statements inside its body.
    pub(crate) fn infer_fn_return_type_for_block(tokens: &[token::Token], fb: &FnBlockInfo) -> Option<String> {
        use token::Token as T;
        let mut k = fb.body_start_idx + 1;
        let body_end = fb.body_end_idx;
        if body_end <= k {
            return None;
        }
        let mut return_types: Vec<val::Type> = Vec::new();
        while k < body_end {
            if matches!(tokens[k], T::Return) {
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
                            let mut checker = typ::TypeChecker::new();
                            if let Ok(ret_ty) = checker.infer_resolved_type(&expr) {
                                return_types.push(ret_ty);
                            }
                        }
                    }
                }
                k = e + 1;
                continue;
            }
            k += 1;
        }
        if return_types.is_empty() {
            return None;
        }
        use std::collections::BTreeMap;
        let mut by_key: BTreeMap<String, val::Type> = BTreeMap::new();
        for t in return_types {
            by_key.entry(t.display()).or_insert(t);
        }
        let parts: Vec<String> = by_key.into_keys().collect();
        Some(if parts.len() == 1 {
            parts[0].clone()
        } else {
            parts.join(" | ")
        })
    }

    /// List available stdlib module names
    pub fn list_stdlib_modules(&self) -> Vec<String> {
        self.registry.get_module_names()
    }

    /// List exports for a given stdlib module name
    pub fn list_module_exports(&self, module: &str) -> Option<Vec<String>> {
        match self.registry.get_module(module) {
            Ok(m) => {
                let exports = m.exports();
                let mut keys: Vec<String> = exports.keys().cloned().collect();
                keys.sort();
                Some(keys)
            }
            Err(_) => None,
        }
    }

    /// Collect imported module aliases from the given content.
    /// Returns mapping alias -> module_name (e.g., "m" -> "math").
    pub fn collect_import_aliases(&mut self, content: &str) -> HashMap<String, String> {
        let mut map = HashMap::new();
        // Tokenize using cached path to be consistent with analysis
        let entry = match self.tokenize_with_spans_cached(content) {
            Ok(p) => p,
            Err(_) => return map,
        };
        let tokens = entry.tokens.clone();
        let spans = entry.spans.clone();

        // Prefer full parse; fall back to recovering parse to extract as many imports as possible
        let mut stmts_acc: Vec<Box<Stmt>> = Vec::new();
        if let Ok(program_arc) = entry.parse_program_arc(content) {
            stmts_acc.extend(program_arc.statements.iter().cloned());
        }
        // Recover for any missed imports (e.g., partial files)
        {
            let mut recover_parser = StmtParser::new_with_spans(tokens.as_ref(), spans.as_ref());
            let (more, _errs) = recover_parser.parse_program_recovering_with_enhanced_errors(content);
            for s in more {
                stmts_acc.push(s);
            }
        }

        for stmt in &stmts_acc {
            if let Stmt::Import(import_stmt) = stmt.as_ref() {
                match import_stmt {
                    ImportStmt::Module { module } => {
                        // import math; -> alias is module name
                        map.insert(module.clone(), module.clone());
                    }
                    ImportStmt::ModuleAlias { module, alias } => {
                        // import math as m; -> alias maps to module
                        map.insert(alias.clone(), module.clone());
                    }
                    ImportStmt::Namespace { alias, source } => {
                        if let stmt::ImportSource::Module(name) = source {
                            // import * as m from math; -> alias maps to module
                            map.insert(alias.clone(), name.clone());
                        }
                    }
                    ImportStmt::Items { source, .. } => {
                        // import { sqrt } from math; -> does not create a module alias
                        // We could track individual items in the future
                        if let stmt::ImportSource::Module(_name) = source {
                            // no alias to insert
                        }
                    }
                    ImportStmt::File { .. } => {
                        // File imports are not stdlib modules; ignore here
                    }
                }
            }
        }

        map
    }

    /// Segment the document into logical chunks using a lightweight state machine:
    /// - Split at semicolons when not inside strings/comments and with paren/bracket depth 0
    /// - Split at closing '}' to capture full blocks (e.g., if/while/fn bodies)
    /// - Preserve multi-line strings and block comments
    pub(crate) fn segment_document(&self, content: &str) -> Vec<(usize, usize, usize)> {
        // Returns a list of (start_byte, end_byte, start_line_idx0)
        let mut chunks = Vec::new();
        if content.trim().is_empty() {
            return chunks;
        }

        let mut start_byte = 0usize;
        let mut start_line = 0usize; // 0-based

        let mut line = 0usize;
        let mut paren = 0i32;
        let mut bracket = 0i32;
        let mut brace = 0i32;
        let mut in_block_comment = false;
        let mut in_line_comment = false;
        let mut in_string: Option<char> = None;
        let mut prev_was_backslash = false;

        let bytes = content.as_bytes();
        let mut i = 0usize;
        while i < bytes.len() {
            let b = bytes[i];
            let ch = b as char;

            // Track line numbers
            if ch == '\n' {
                line += 1;
                in_line_comment = false; // end of line comment
                prev_was_backslash = false;
                i += 1;
                continue;
            }

            if in_line_comment {
                i += 1;
                continue;
            }

            if in_block_comment {
                // Look for end of block comment '*/'
                if ch == '*' && i + 1 < bytes.len() && bytes[i + 1] as char == '/' {
                    in_block_comment = false;
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }

            if let Some(q) = in_string {
                // Inside string; handle escapes
                if ch == q && !prev_was_backslash {
                    in_string = None;
                    prev_was_backslash = false;
                    i += 1;
                } else {
                    prev_was_backslash = ch == '\\' && !prev_was_backslash;
                    if !prev_was_backslash {
                        prev_was_backslash = false;
                    }
                    i += 1;
                }
                continue;
            }

            // Not inside string/comment
            // Handle comment starts
            if ch == '/' && i + 1 < bytes.len() {
                let n = bytes[i + 1] as char;
                if n == '/' {
                    in_line_comment = true;
                    i += 2;
                    continue;
                }
                if n == '*' {
                    in_block_comment = true;
                    i += 2;
                    continue;
                }
            }

            // Handle string start
            if ch == '"' || ch == '\'' {
                in_string = Some(ch);
                prev_was_backslash = false;
                i += 1;
                continue;
            }

            // Track nesting
            match ch {
                '(' => paren += 1,
                ')' => paren -= 1,
                '[' => bracket += 1,
                ']' => bracket -= 1,
                '{' => brace += 1,
                '}' => {
                    brace -= 1;
                    // A closing brace at depth 0 is a good chunk boundary
                    if paren == 0 && bracket == 0 && brace == 0 {
                        let end_byte = i + 1; // include '}'
                                              // Avoid empty whitespace-only chunks
                        if !content[start_byte..end_byte].trim().is_empty() {
                            chunks.push((start_byte, end_byte, start_line));
                            if chunks.len() >= MAX_SCAN_CHUNKS {
                                return chunks;
                            }
                        }
                        start_byte = end_byte;
                        start_line = line;
                    }
                }
                ';' => {
                    // Statement terminator outside paren/bracket nesting
                    if paren == 0 && bracket == 0 {
                        let end_byte = i + 1; // include ';'
                        if !content[start_byte..end_byte].trim().is_empty() {
                            chunks.push((start_byte, end_byte, start_line));
                            if chunks.len() >= MAX_SCAN_CHUNKS {
                                return chunks;
                            }
                        }
                        start_byte = end_byte;
                        start_line = line;
                    }
                }
                _ => {}
            }

            i += 1;
        }

        // Trailing chunk
        if start_byte < bytes.len() {
            let tail = &content[start_byte..];
            if !tail.trim().is_empty() {
                chunks.push((start_byte, bytes.len(), start_line));
            }
        }

        chunks
    }

    /// Chunk-based diagnostics scan. Attempts to parse multi-line logical chunks
    /// to surface multiple independent errors with better positions.
    pub(crate) fn scan_chunks_for_diagnostics(&self, content: &str) -> Vec<Diagnostic> {
        let mut diags = Vec::new();
        let chunks = self.segment_document(content);
        if chunks.is_empty() {
            return diags;
        }

        for (start_b, end_b, start_line) in chunks.into_iter().take(MAX_SCAN_CHUNKS) {
            let chunk = &content[start_b..end_b];
            if chunk.trim().is_empty() {
                continue;
            }

            // Prefer tokenization errors which carry precise spans
            match Tokenizer::tokenize_enhanced_with_spans(chunk) {
                Err(parse_err) => {
                    let range = if let Some(span) = &parse_err.span {
                        let start_pos = Position::new(
                            (start_line as u32) + (span.start.line - 1),
                            span.start.column.saturating_sub(1),
                        );
                        let end_pos = Position::new(
                            (start_line as u32) + (span.end.line - 1),
                            span.end.column.saturating_sub(1),
                        );
                        Range::new(start_pos, end_pos)
                    } else {
                        Range::new(
                            Position::new(start_line as u32, 0),
                            Position::new(start_line as u32, chunk.chars().count() as u32),
                        )
                    };

                    if diags.len() >= MAX_DIAGNOSTICS {
                        break;
                    }
                    diags.push(Diagnostic::new(
                        range,
                        Some(DiagnosticSeverity::ERROR),
                        None,
                        Some("lkr".to_string()),
                        format!("Tokenization error: {}", parse_err.message),
                        None,
                        None,
                    ));
                    continue;
                }
                Ok((chunk_tokens, chunk_spans)) => {
                    // Try parsing as statement program first to catch control structures
                    let mut sp = StmtParser::new_with_spans(&chunk_tokens, &chunk_spans);
                    match sp.parse_program_with_enhanced_errors(chunk) {
                        Ok(_) => {
                            // No statement-level error in this chunk; continue
                        }
                        Err(stmt_err) => {
                            // Try expression recovery first for potentially multiple, more specific spans
                            let expr_errs = ExprParser::recover_expression_errors(&chunk_tokens, &chunk_spans, chunk);

                            if !expr_errs.is_empty() {
                                // Use expression errors if available (more specific)
                                for ee in expr_errs {
                                    if diags.len() >= MAX_DIAGNOSTICS {
                                        break;
                                    }
                                    let range = if let Some(span) = &ee.span {
                                        let start_pos = Position::new(
                                            (start_line as u32) + (span.start.line - 1),
                                            span.start.column.saturating_sub(1),
                                        );
                                        let end_pos = Position::new(
                                            (start_line as u32) + (span.end.line - 1),
                                            span.end.column.saturating_sub(1),
                                        );
                                        Range::new(start_pos, end_pos)
                                    } else {
                                        Range::new(
                                            Position::new(start_line as u32, 0),
                                            Position::new(start_line as u32, chunk.chars().count() as u32),
                                        )
                                    };
                                    diags.push(Diagnostic::new(
                                        range,
                                        Some(DiagnosticSeverity::ERROR),
                                        None,
                                        Some("lkr".to_string()),
                                        ee.message.clone(),
                                        None,
                                        None,
                                    ));
                                }
                            } else {
                                // Fall back to statement error if no expression errors found
                                let range = if let Some(span) = &stmt_err.span {
                                    let start_pos = Position::new(
                                        (start_line as u32) + (span.start.line - 1),
                                        span.start.column.saturating_sub(1),
                                    );
                                    let end_pos = Position::new(
                                        (start_line as u32) + (span.end.line - 1),
                                        span.end.column.saturating_sub(1),
                                    );
                                    Range::new(start_pos, end_pos)
                                } else {
                                    Range::new(
                                        Position::new(start_line as u32, 0),
                                        Position::new(start_line as u32, chunk.chars().count() as u32),
                                    )
                                };

                                if diags.len() < MAX_DIAGNOSTICS {
                                    diags.push(Diagnostic::new(
                                        range,
                                        Some(DiagnosticSeverity::ERROR),
                                        None,
                                        Some("lkr".to_string()),
                                        stmt_err.message.clone(),
                                        None,
                                        None,
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            if diags.len() >= MAX_DIAGNOSTICS {
                break;
            }
        }

        diags
    }

    /// Best-effort line-wise scan to accumulate multiple diagnostics.
    /// This helps surface multiple independent errors in a single document
    /// instead of stopping at the first parse failure.
    pub(crate) fn scan_lines_for_diagnostics(&self, content: &str) -> Vec<Diagnostic> {
        let mut diags = Vec::new();

        for (line_idx, line) in content.lines().enumerate().take(MAX_SCAN_LINES) {
            // Skip empty or whitespace-only lines to reduce noise
            if line.trim().is_empty() {
                continue;
            }

            // Try tokenizing the single line first to get precise position if it fails
            match Tokenizer::tokenize_enhanced_with_spans(line) {
                Err(parse_err) => {
                    if diags.len() >= MAX_DIAGNOSTICS {
                        break;
                    }
                    let range = if let Some(span) = &parse_err.span {
                        let start_pos = Position::new(line_idx as u32, span.start.column.saturating_sub(1));
                        let end_pos = Position::new(line_idx as u32, span.end.column.saturating_sub(1));
                        Range::new(start_pos, end_pos)
                    } else {
                        // Fallback: highlight whole line
                        Range::new(
                            Position::new(line_idx as u32, 0),
                            Position::new(line_idx as u32, line.chars().count() as u32),
                        )
                    };

                    diags.push(Diagnostic::new(
                        range,
                        Some(DiagnosticSeverity::ERROR),
                        None,
                        Some("lkr".to_string()),
                        format!("Tokenization error: {}", parse_err.message),
                        None,
                        None,
                    ));
                    continue; // Cannot parse further for this line
                }
                Ok((line_tokens, line_spans)) => {
                    // Try parsing this line as a (mini) program using the statement parser
                    let mut sp = StmtParser::new_with_spans(&line_tokens, &line_spans);
                    match sp.parse_program_with_enhanced_errors(line) {
                        Ok(_) => {
                            // No statement-level error on this line
                        }
                        Err(parse_err) => {
                            // Try expression recovery first for potentially multiple, more specific spans
                            let expr_errs = ExprParser::recover_expression_errors(&line_tokens, &line_spans, line);

                            if !expr_errs.is_empty() {
                                // Use expression errors if available (more specific)
                                for ee in expr_errs {
                                    if diags.len() >= MAX_DIAGNOSTICS {
                                        break;
                                    }
                                    let range = if let Some(span) = &ee.span {
                                        let start_pos =
                                            Position::new(line_idx as u32, span.start.column.saturating_sub(1));
                                        let end_pos = Position::new(line_idx as u32, span.end.column.saturating_sub(1));
                                        Range::new(start_pos, end_pos)
                                    } else {
                                        Range::new(
                                            Position::new(line_idx as u32, 0),
                                            Position::new(line_idx as u32, line.chars().count() as u32),
                                        )
                                    };
                                    diags.push(Diagnostic::new(
                                        range,
                                        Some(DiagnosticSeverity::ERROR),
                                        None,
                                        Some("lkr".to_string()),
                                        ee.message.clone(),
                                        None,
                                        None,
                                    ));
                                }
                            } else {
                                // Fall back to statement error if no expression errors found
                                let range = if let Some(span) = &parse_err.span {
                                    let start_pos = Position::new(line_idx as u32, span.start.column.saturating_sub(1));
                                    let end_pos = Position::new(line_idx as u32, span.end.column.saturating_sub(1));
                                    Range::new(start_pos, end_pos)
                                } else {
                                    // Fallback: highlight whole line
                                    Range::new(
                                        Position::new(line_idx as u32, 0),
                                        Position::new(line_idx as u32, line.chars().count() as u32),
                                    )
                                };

                                diags.push(Diagnostic::new(
                                    range,
                                    Some(DiagnosticSeverity::ERROR),
                                    None,
                                    Some("lkr".to_string()),
                                    parse_err.message.clone(),
                                    None,
                                    None,
                                ));
                            }
                        }
                    }
                }
            }
            if diags.len() >= MAX_DIAGNOSTICS {
                break;
            }
        }

        diags
    }

    pub(crate) fn collect_type_diagnostics(
        program: &Program,
        tokens: &[token::Token],
        spans: &[Span],
        content: &str,
    ) -> Vec<Diagnostic> {
        let mut checker = TypeChecker::new_strict();
        match program.type_check(&mut checker) {
            Ok(_) => Vec::new(),
            Err(err) => {
                let range = Self::type_error_range(&err, tokens, spans, content);
                let mut diagnostic = Diagnostic::new(
                    range,
                    Some(DiagnosticSeverity::ERROR),
                    None,
                    Some("lkr".to_string()),
                    err.to_string(),
                    None,
                    None,
                );
                diagnostic.code = Some(NumberOrString::String("lkr_type_error".to_string()));
                vec![diagnostic]
            }
        }
    }

    pub(crate) fn type_error_range(
        err: &anyhow::Error,
        tokens: &[token::Token],
        spans: &[Span],
        content: &str,
    ) -> Range {
        if let Some(type_error) = Self::type_error_from_anyhow(err) {
            if let Some(expr) = &type_error.expr {
                if let Some(range) = Self::range_for_expr(expr, tokens, spans) {
                    return range;
                }
            }
        }
        Self::default_error_range(content)
    }

    pub(crate) fn type_error_from_anyhow(err: &anyhow::Error) -> Option<&typ::TypeError> {
        err.downcast_ref::<typ::TypeError>()
    }

    pub(crate) fn range_for_expr(expr: &Expr, tokens: &[token::Token], spans: &[Span]) -> Option<Range> {
        match expr {
            Expr::Var(name) => {
                Self::find_token_range(tokens, spans, |tok| matches!(tok, token::Token::Id(id) if id == name))
            }
            Expr::Val(val) => match val {
                val::Val::Str(s) => Self::find_token_range(
                    tokens,
                    spans,
                    |tok| matches!(tok, token::Token::Str(lit) if lit == s.as_ref()),
                ),
                val::Val::Int(i) => {
                    Self::find_token_range(tokens, spans, |tok| matches!(tok, token::Token::Int(n) if n == i))
                }
                val::Val::Float(f) => Self::find_token_range(
                    tokens,
                    spans,
                    |tok| matches!(tok, token::Token::Float(n) if (*n - *f).abs() < f64::EPSILON),
                ),
                val::Val::Bool(b) => {
                    Self::find_token_range(tokens, spans, |tok| matches!(tok, token::Token::Bool(n) if n == b))
                }
                val::Val::Nil => Self::find_token_range(tokens, spans, |tok| matches!(tok, token::Token::Nil)),
                _ => None,
            },
            Expr::Paren(inner) => Self::range_for_expr(inner, tokens, spans),
            Expr::Call(name, _) => {
                Self::find_token_range(tokens, spans, |tok| matches!(tok, token::Token::Id(id) if id == name))
            }
            Expr::CallExpr(callee, _) => Self::range_for_expr(callee, tokens, spans),
            Expr::CallNamed(callee, _, _) => Self::range_for_expr(callee, tokens, spans),
            Expr::Bin(_, _, _) => None,
            _ => None,
        }
    }

    pub(crate) fn find_token_range<F>(tokens: &[token::Token], spans: &[Span], predicate: F) -> Option<Range>
    where
        F: Fn(&token::Token) -> bool,
    {
        tokens.iter().enumerate().find_map(|(idx, tok)| {
            if predicate(tok) && idx < spans.len() {
                Some(Self::span_to_range(&spans[idx]))
            } else {
                None
            }
        })
    }

    pub(crate) fn span_to_range(span: &Span) -> Range {
        Range::new(
            Position::new(span.start.line.saturating_sub(1), span.start.column.saturating_sub(1)),
            Position::new(span.end.line.saturating_sub(1), span.end.column.saturating_sub(1)),
        )
    }

    pub(crate) fn default_error_range(content: &str) -> Range {
        for (idx, line) in content.lines().enumerate() {
            let width = line.chars().count() as u32;
            if width > 0 {
                return Range::new(Position::new(idx as u32, 0), Position::new(idx as u32, width));
            }
        }
        Range::new(Position::new(0, 0), Position::new(0, 0))
    }

    pub(crate) fn analyze_statements(&self, statements: &[Box<Stmt>], result: &mut AnalysisResult) {
        for (i, stmt) in statements.iter().enumerate() {
            match stmt.as_ref() {
                Stmt::Let { pattern, .. } => {
                    // Extract variable names from pattern and create symbols for each
                    if let Some(variables) = extract_variables_from_pattern(pattern) {
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
                Stmt::Function { .. } => {}
                Stmt::Import(_import_stmt) => { /* imports are grouped via token scan later */ }
                _ => {}
            }
        }
    }

    pub(crate) fn dedup_diagnostics(&self, diagnostics: &mut Vec<Diagnostic>) {
        diagnostics.sort_by(|a, b| {
            let ra = &a.range;
            let rb = &b.range;
            (
                ra.start.line,
                ra.start.character,
                ra.end.line,
                ra.end.character,
                a.message.clone(),
            )
                .cmp(&(
                    rb.start.line,
                    rb.start.character,
                    rb.end.line,
                    rb.end.character,
                    b.message.clone(),
                ))
        });
        diagnostics.dedup_by(|a, b| a.range == b.range && a.message == b.message);
    }
}
