use std::path::Path;

use tower_lsp::lsp_types::{Location, Position, Range, Url};

use lk_core::{macro_system, token};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportedMacroDefinition {
    pub(crate) source: ImportedMacroSource,
    pub(crate) name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ImportedMacroSource {
    File(String),
    Package(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MacroInvocationContext {
    name: String,
    qualifier: Option<String>,
}

pub(crate) fn find_local_macro_definition(
    tokens: &[token::Token],
    spans: &[token::Span],
    offset: usize,
    uri: &Url,
) -> Option<Location> {
    let invocation = macro_invocation_at_offset(tokens, spans, offset)?;
    if invocation.qualifier.is_some() {
        return None;
    }
    for idx in 0..tokens.len() {
        if let Some(location) = macro_rules_definition_at(tokens, spans, idx, &invocation.name, uri) {
            return Some(location);
        }
    }
    None
}

pub(crate) fn imported_macro_definition(
    tokens: &[token::Token],
    spans: &[token::Span],
    offset: usize,
) -> Option<ImportedMacroDefinition> {
    let invocation = macro_invocation_at_offset(tokens, spans, offset)?;
    for (idx, token) in tokens.iter().enumerate() {
        if !matches!(token, token::Token::Use) {
            continue;
        }
        if let Some(definition) = named_macro_import_definition(tokens, idx, &invocation) {
            return Some(definition);
        }
        if let Some(definition) = namespace_macro_import_definition(tokens, idx, &invocation) {
            return Some(definition);
        }
    }
    None
}

pub(crate) fn find_exported_macro_definition_in_content(
    content: &str,
    exported_name: &str,
    uri: &Url,
) -> Option<Location> {
    let (tokens, spans) = token::Tokenizer::tokenize_enhanced_with_spans(content).ok()?;
    let local_name = exported_macro_local_name(&tokens, exported_name).unwrap_or_else(|| exported_name.to_string());
    for idx in 0..tokens.len() {
        if let Some(location) = macro_rules_definition_at(&tokens, &spans, idx, &local_name, uri) {
            return Some(location);
        }
    }
    None
}

fn macro_rules_definition_at(
    tokens: &[token::Token],
    spans: &[token::Span],
    idx: usize,
    expected_name: &str,
    uri: &Url,
) -> Option<Location> {
    if !matches!(tokens.get(idx), Some(token::Token::Id(keyword)) if keyword == "macro_rules") {
        return None;
    }
    let Some(token::Token::Not) = tokens.get(idx + 1) else {
        return None;
    };
    let Some(token::Token::Id(name)) = tokens.get(idx + 2) else {
        return None;
    };
    if name != expected_name {
        return None;
    }
    let span = spans.get(idx + 2)?;
    Some(Location::new(uri.clone(), range_from_span(span)))
}

fn macro_invocation_at_offset(
    tokens: &[token::Token],
    spans: &[token::Span],
    offset: usize,
) -> Option<MacroInvocationContext> {
    for (idx, span) in spans.iter().enumerate() {
        if offset < span.start.offset || offset >= span.end.offset {
            continue;
        }
        match tokens.get(idx) {
            Some(token::Token::Id(_)) => {
                if idx >= 2 && matches!(tokens.get(idx - 1), Some(token::Token::ColonColon)) {
                    if let Some(invocation) = macro_invocation_starting_at(tokens, idx - 2) {
                        return Some(invocation);
                    }
                }
                if let Some(invocation) = macro_invocation_starting_at(tokens, idx) {
                    return Some(invocation);
                }
            }
            Some(token::Token::ColonColon) if idx > 0 => {
                if let Some(invocation) = macro_invocation_starting_at(tokens, idx - 1) {
                    return Some(invocation);
                }
            }
            Some(token::Token::Not) if idx > 0 => {
                if let Some(invocation) = macro_invocation_starting_at(tokens, idx - 1) {
                    return Some(invocation);
                }
                if idx >= 3 {
                    if let Some(invocation) = macro_invocation_starting_at(tokens, idx - 3) {
                        return Some(invocation);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn macro_invocation_starting_at(tokens: &[token::Token], idx: usize) -> Option<MacroInvocationContext> {
    let token::Token::Id(first) = tokens.get(idx)? else {
        return None;
    };
    if is_unqualified_macro_invocation(tokens, idx) {
        return Some(MacroInvocationContext {
            name: first.clone(),
            qualifier: None,
        });
    }

    let Some(token::Token::ColonColon) = tokens.get(idx + 1) else {
        return None;
    };
    let Some(token::Token::Id(name)) = tokens.get(idx + 2) else {
        return None;
    };
    if !is_macro_call_suffix(tokens, idx + 3) {
        return None;
    }
    Some(MacroInvocationContext {
        name: name.clone(),
        qualifier: Some(first.clone()),
    })
}

fn is_unqualified_macro_invocation(tokens: &[token::Token], idx: usize) -> bool {
    matches!(tokens.get(idx), Some(token::Token::Id(_))) && is_macro_call_suffix(tokens, idx + 1)
}

fn is_macro_call_suffix(tokens: &[token::Token], bang_idx: usize) -> bool {
    matches!(tokens.get(bang_idx), Some(token::Token::Not))
        && matches!(
            tokens.get(bang_idx + 1),
            Some(token::Token::LParen | token::Token::LBracket | token::Token::LBrace)
        )
}

fn named_macro_import_definition(
    tokens: &[token::Token],
    use_idx: usize,
    invocation: &MacroInvocationContext,
) -> Option<ImportedMacroDefinition> {
    if invocation.qualifier.is_some() || !matches!(tokens.get(use_idx + 1), Some(token::Token::LBrace)) {
        return None;
    }
    let end = matching_group_end(tokens, use_idx + 1)?;
    if !matches!(tokens.get(end + 1), Some(token::Token::From)) {
        return None;
    }
    let source = macro_import_source(tokens.get(end + 2)?)?;
    let name = named_macro_import_source_name(tokens, use_idx + 2, end, &invocation.name)?;
    Some(ImportedMacroDefinition { source, name })
}

fn namespace_macro_import_definition(
    tokens: &[token::Token],
    use_idx: usize,
    invocation: &MacroInvocationContext,
) -> Option<ImportedMacroDefinition> {
    let qualifier = invocation.qualifier.as_ref()?;
    match tokens.get(use_idx + 1)? {
        token::Token::Str(path) => {
            if default_namespace_alias(path).as_deref()? == qualifier.as_str() {
                Some(ImportedMacroDefinition {
                    source: ImportedMacroSource::File(path.clone()),
                    name: invocation.name.clone(),
                })
            } else {
                None
            }
        }
        token::Token::Id(module) => {
            let alias = import_alias(tokens, use_idx + 2).unwrap_or(module);
            if alias == qualifier {
                Some(ImportedMacroDefinition {
                    source: ImportedMacroSource::Package(module.clone()),
                    name: invocation.name.clone(),
                })
            } else {
                None
            }
        }
        token::Token::Mul => {
            if !matches!(tokens.get(use_idx + 2), Some(token::Token::As)) {
                return None;
            }
            let Some(token::Token::Id(alias)) = tokens.get(use_idx + 3) else {
                return None;
            };
            if alias != qualifier || !matches!(tokens.get(use_idx + 4), Some(token::Token::From)) {
                return None;
            }
            Some(ImportedMacroDefinition {
                source: macro_import_source(tokens.get(use_idx + 5)?)?,
                name: invocation.name.clone(),
            })
        }
        _ => None,
    }
}

fn named_macro_import_source_name(
    tokens: &[token::Token],
    start: usize,
    end: usize,
    invocation_name: &str,
) -> Option<String> {
    let mut idx = start;
    while idx < end {
        match tokens.get(idx)? {
            token::Token::Comma => idx += 1,
            token::Token::Id(name) => {
                let source_name = name.clone();
                let mut alias = source_name.clone();
                idx += 1;
                if matches!(tokens.get(idx), Some(token::Token::As)) {
                    let Some(token::Token::Id(alias_name)) = tokens.get(idx + 1) else {
                        return None;
                    };
                    alias = alias_name.clone();
                    idx += 2;
                }
                if alias == invocation_name {
                    return Some(source_name);
                }
                if matches!(tokens.get(idx), Some(token::Token::Comma)) {
                    idx += 1;
                }
            }
            _ => return None,
        }
    }
    None
}

fn macro_import_source(token: &token::Token) -> Option<ImportedMacroSource> {
    match token {
        token::Token::Str(path) => Some(ImportedMacroSource::File(path.clone())),
        token::Token::Id(module) if module != "macros" => Some(ImportedMacroSource::Package(module.clone())),
        _ => None,
    }
}

fn import_alias(tokens: &[token::Token], idx: usize) -> Option<&String> {
    if matches!(tokens.get(idx), Some(token::Token::As)) {
        let Some(token::Token::Id(alias)) = tokens.get(idx + 1) else {
            return None;
        };
        Some(alias)
    } else {
        None
    }
}

fn matching_group_end(tokens: &[token::Token], start: usize) -> Option<usize> {
    let (open, close) = match tokens.get(start)? {
        token::Token::LBrace => (token::Token::LBrace, token::Token::RBrace),
        token::Token::LParen => (token::Token::LParen, token::Token::RParen),
        token::Token::LBracket => (token::Token::LBracket, token::Token::RBracket),
        _ => return None,
    };
    let mut depth = 0usize;
    for (idx, token) in tokens.iter().enumerate().skip(start) {
        if *token == open {
            depth += 1;
        } else if *token == close {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(idx);
            }
        }
    }
    None
}

fn default_namespace_alias(raw: &str) -> Option<String> {
    Path::new(raw).file_stem()?.to_str().map(str::to_string)
}

fn exported_macro_local_name(tokens: &[token::Token], exported_name: &str) -> Option<String> {
    for (idx, token) in tokens.iter().enumerate() {
        if !matches!(token, token::Token::Id(keyword) if keyword == "export") {
            continue;
        }
        if matches!(tokens.get(idx + 1), Some(token::Token::Id(keyword)) if keyword == "macro_rules") {
            if let Some(token::Token::Id(name)) = tokens.get(idx + 3) {
                if name == exported_name {
                    return Some(name.clone());
                }
            }
        }
        if !matches!(tokens.get(idx + 1), Some(token::Token::LBrace)) {
            continue;
        }
        let end = matching_group_end(tokens, idx + 1)?;
        if let Some(name) = exported_macro_list_local_name(tokens, idx + 2, end, exported_name) {
            return Some(name);
        }
    }
    None
}

fn exported_macro_list_local_name(
    tokens: &[token::Token],
    start: usize,
    end: usize,
    exported_name: &str,
) -> Option<String> {
    let mut idx = start;
    while idx < end {
        match tokens.get(idx)? {
            token::Token::Comma => idx += 1,
            token::Token::Id(name) => {
                let local_name = name.clone();
                let mut alias = local_name.clone();
                idx += 1;
                if matches!(tokens.get(idx), Some(token::Token::As)) {
                    let Some(token::Token::Id(alias_name)) = tokens.get(idx + 1) else {
                        return None;
                    };
                    alias = alias_name.clone();
                    idx += 2;
                }
                if alias == exported_name {
                    return Some(local_name);
                }
                if matches!(tokens.get(idx), Some(token::Token::Comma)) {
                    idx += 1;
                }
            }
            _ => return None,
        }
    }
    None
}

pub(crate) fn generated_ast_item_definition_location(
    origins: &[macro_system::AstMacroOrigin],
    symbol_name: &str,
    uri: &Url,
) -> Option<Location> {
    for origin in origins {
        for item in &origin.generated_item_origins {
            for member in &item.generated_member_origins {
                let Some(name) = generated_member_label_name(&member.label) else {
                    continue;
                };
                if name != symbol_name {
                    continue;
                }
                let span = member
                    .span
                    .as_ref()
                    .or(item.span.as_ref())
                    .or(origin.input_span.as_ref())?;
                return Some(Location::new(uri.clone(), range_from_span(span)));
            }
            if !generated_item_label_matches(&item.label, symbol_name) {
                continue;
            }
            let span = item.span.as_ref().or(origin.input_span.as_ref())?;
            return Some(Location::new(uri.clone(), range_from_span(span)));
        }
    }
    None
}

fn generated_item_label_matches(label: &str, symbol_name: &str) -> bool {
    if ["fn ", "struct ", "trait ", "type "]
        .iter()
        .find_map(|prefix| label.strip_prefix(prefix))
        .is_some_and(|name| name == symbol_name)
    {
        return true;
    }
    generated_impl_label_names(label).any(|name| name == symbol_name)
}

fn generated_impl_label_names(label: &str) -> impl Iterator<Item = &str> {
    let mut names = [None, None];
    if let Some(rest) = label.strip_prefix("impl ") {
        if let Some((trait_name, target_name)) = rest.split_once(" for ") {
            names[0] = Some(trait_name);
            names[1] = Some(target_name);
        } else {
            names[0] = Some(rest);
        }
    }
    names.into_iter().flatten()
}

fn generated_member_label_name(label: &str) -> Option<&str> {
    for prefix in ["fn ", "struct ", "trait ", "type "] {
        if let Some(name) = label.strip_prefix(prefix) {
            return Some(name);
        }
    }
    label
        .strip_prefix("expr ")
        .and_then(generated_expr_label_name)
        .or_else(|| label.strip_prefix("select "))
        .or_else(|| generated_index_label_name(label))
        .or_else(|| label.strip_prefix("stmt "))
        .or_else(|| label.strip_prefix("call "))
        .or_else(|| label.strip_prefix("ref "))
        .or_else(|| label.strip_prefix("assign_ref "))
        .or_else(|| label.strip_prefix("compound_assign_ref "))
        .or_else(|| label.strip_prefix("binding "))
        .or_else(|| label.strip_prefix("struct_field "))
        .or_else(|| label.strip_prefix("map_key "))
        .or_else(|| label.strip_prefix("named_arg "))
        .or_else(|| label.strip_prefix("named_param_type "))
        .or_else(|| label.strip_prefix("import_module "))
        .or_else(|| label.strip_prefix("import_file "))
        .or_else(|| label.strip_prefix("import_item "))
        .or_else(|| label.strip_prefix("import_alias "))
        .or_else(|| label.strip_prefix("import_namespace "))
        .or_else(|| label.strip_prefix("attr "))
        .or_else(|| label.strip_prefix("derive "))
        .or_else(|| label.strip_prefix("type_ref "))
        .or_else(|| label.strip_prefix("type_var "))?
        .rsplit('.')
        .next()
}

fn generated_expr_label_name(label: &str) -> Option<&str> {
    if matches!(label, "match" | "select") {
        return Some(label);
    }
    label.contains('.').then(|| label.rsplit('.').next()).flatten()
}

fn generated_index_label_name(label: &str) -> Option<&str> {
    let rest = label.strip_prefix("index ")?;
    let mut parts = rest.rsplit('.');
    let last = parts.next()?;
    if last.chars().all(|ch| ch.is_ascii_digit()) {
        parts.next()
    } else {
        Some(last)
    }
}

fn range_from_span(span: &token::Span) -> Range {
    Range::new(
        Position::new(span.start.line - 1, span.start.column.saturating_sub(1)),
        Position::new(span.end.line - 1, span.end.column.saturating_sub(1)),
    )
}

#[cfg(test)]
mod tests;
