mod classification;
mod for_patterns;

use crate::{ast::Parser as ExprParser, expr::Pattern, token::Token};
use classification::{
    is_hygiene_preserved_identifier_reference, is_semantic_identifier_definition,
    is_type_annotation_identifier_reference,
};

use super::ExpandedToken;

#[derive(Debug, Clone)]
struct BindingRename {
    name: String,
    replacement: String,
    binding_index: usize,
    reference_start: usize,
    excluded_start: Option<usize>,
    excluded_end: Option<usize>,
    end: usize,
}

#[derive(Debug, Clone)]
struct ParameterBinding {
    name: String,
    index: usize,
    excluded_start: Option<usize>,
    excluded_end: Option<usize>,
}

pub(super) fn apply_simple_hygiene(tokens: Vec<ExpandedToken>, site: usize) -> Vec<ExpandedToken> {
    let renames = collect_generated_binding_renames(&tokens, site);
    tokens
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, mut token)| {
            if !token.from_capture
                && let Token::Id(name) = &token.token.token
                && let Some(replacement) = rename_for_identifier(&renames, name, index)
                && (is_hygienic_identifier_reference(&tokens, index)
                    || is_generated_binding_identifier(&renames, name, index))
            {
                token.token.token = Token::Id(replacement.to_string());
                token.token.lexeme = replacement.to_string();
            }
            token
        })
        .collect()
}

fn collect_generated_binding_renames(tokens: &[ExpandedToken], site: usize) -> Vec<BindingRename> {
    let mut renames = Vec::new();
    collect_let_const_binding_renames(tokens, site, &mut renames);
    collect_define_binding_renames(tokens, site, &mut renames);
    collect_for_pattern_binding_renames(tokens, site, &mut renames);
    collect_if_while_let_binding_renames(tokens, site, &mut renames);
    collect_select_case_binding_renames(tokens, site, &mut renames);
    collect_match_arm_binding_renames(tokens, site, &mut renames);
    collect_function_param_binding_renames(tokens, site, &mut renames);
    collect_closure_param_binding_renames(tokens, site, &mut renames);
    collect_fn_closure_param_binding_renames(tokens, site, &mut renames);
    renames
}

fn collect_let_const_binding_renames(tokens: &[ExpandedToken], site: usize, renames: &mut Vec<BindingRename>) {
    let mut index = 0;
    while index + 1 < tokens.len() {
        if tokens[index].from_capture
            || !matches!(tokens[index].token.token, Token::Let | Token::Const)
            || is_control_flow_let_keyword(tokens, index)
        {
            index += 1;
            continue;
        }
        let scope_end = generated_binding_scope_end(tokens, index);
        let reference_start = find_statement_reference_start(tokens, index).unwrap_or(scope_end);
        if let Some((pattern, consumed)) = parse_generated_pattern_prefix(tokens, index + 1) {
            collect_pattern_binding_renames(
                tokens,
                index + 1,
                consumed,
                &pattern,
                site,
                reference_start,
                None,
                None,
                scope_end,
                renames,
            );
            index += consumed + 1;
            continue;
        }
        collect_simple_generated_id(tokens, index + 1, site, reference_start, None, None, scope_end, renames);
        index += 1;
    }
}

fn collect_define_binding_renames(tokens: &[ExpandedToken], site: usize, renames: &mut Vec<BindingRename>) {
    let mut index = 0;
    while index + 2 < tokens.len() {
        if !tokens[index].from_capture
            && !tokens[index + 1].from_capture
            && !tokens[index + 2].from_capture
            && matches!(tokens[index].token.token, Token::Id(_))
            && matches!(tokens[index + 1].token.token, Token::Colon)
            && matches!(tokens[index + 2].token.token, Token::Assign)
        {
            let scope_end = generated_binding_scope_end(tokens, index);
            let reference_start = find_statement_reference_start(tokens, index).unwrap_or(scope_end);
            collect_simple_generated_id(tokens, index, site, reference_start, None, None, scope_end, renames);
            index += 3;
        } else {
            index += 1;
        }
    }
}

fn collect_for_pattern_binding_renames(tokens: &[ExpandedToken], site: usize, renames: &mut Vec<BindingRename>) {
    let mut index = 0;
    while index < tokens.len() {
        if tokens[index].from_capture || !matches!(tokens[index].token.token, Token::For) {
            index += 1;
            continue;
        }
        let Some(pattern_end) = find_for_pattern_end(tokens, index + 1) else {
            index += 1;
            continue;
        };
        let body_start = find_for_body_start(tokens, pattern_end + 1);
        let scope_end = body_start
            .and_then(|start| {
                find_matching_delimiter(tokens, start, Token::LBrace, Token::RBrace).map(|index| index + 1)
            })
            .unwrap_or_else(|| generated_binding_scope_end(tokens, index));
        let reference_start = body_start.map_or(scope_end, |start| start + 1);
        if let Some((pattern, consumed)) =
            for_patterns::parse_generated_for_pattern_prefix(tokens, index + 1, pattern_end)
        {
            for_patterns::collect_for_pattern_binding_renames(
                tokens,
                index + 1,
                consumed,
                &pattern,
                site,
                reference_start,
                scope_end,
                renames,
            );
        } else if let Some((pattern, consumed)) = parse_generated_pattern_prefix(tokens, index + 1) {
            collect_pattern_binding_renames(
                tokens,
                index + 1,
                consumed,
                &pattern,
                site,
                reference_start,
                None,
                None,
                scope_end,
                renames,
            );
        } else {
            for_patterns::collect_fallback_for_pattern_ids(
                tokens,
                index + 1,
                pattern_end,
                site,
                reference_start,
                scope_end,
                renames,
            );
        }
        index = pattern_end + 1;
    }
}

fn collect_if_while_let_binding_renames(tokens: &[ExpandedToken], site: usize, renames: &mut Vec<BindingRename>) {
    let mut index = 0;
    while index + 2 < tokens.len() {
        if !tokens[index].from_capture
            && matches!(tokens[index].token.token, Token::If | Token::While)
            && !tokens[index + 1].from_capture
            && matches!(tokens[index + 1].token.token, Token::Let)
        {
            if let Some((pattern, consumed)) = parse_generated_pattern_prefix(tokens, index + 2) {
                let scope_start =
                    find_if_while_let_value_start(tokens, index + 2).map_or(index + 2 + consumed, |assign| assign + 1);
                let body_start = find_control_flow_body_start(tokens, scope_start);
                let scope_end = body_start
                    .and_then(|start| {
                        find_matching_delimiter(tokens, start, Token::LBrace, Token::RBrace).map(|index| index + 1)
                    })
                    .unwrap_or_else(|| generated_binding_scope_end(tokens, index));
                let reference_start = index + 2;
                let excluded_start = find_if_while_let_value_start(tokens, index + 2).map(|assign| assign + 1);
                let excluded_end = body_start;
                collect_pattern_binding_renames(
                    tokens,
                    index + 2,
                    consumed,
                    &pattern,
                    site,
                    reference_start,
                    excluded_start,
                    excluded_end,
                    scope_end,
                    renames,
                );
                index = scope_end.max(index + consumed + 2);
                continue;
            }
        }
        index += 1;
    }
}

fn collect_select_case_binding_renames(tokens: &[ExpandedToken], site: usize, renames: &mut Vec<BindingRename>) {
    let mut index = 0;
    while index + 2 < tokens.len() {
        if !tokens[index].from_capture
            && matches!(tokens[index].token.token, Token::Case)
            && matches!(
                (&tokens[index + 1].token.token, &tokens[index + 2].token.token),
                (Token::Id(_), Token::LeftArrow | Token::Le)
            )
        {
            let scope_end =
                find_select_case_end(tokens, index + 3).unwrap_or_else(|| generated_binding_scope_end(tokens, index));
            let reference_start = find_select_case_binding_reference_start(tokens, index + 3).unwrap_or(scope_end);
            collect_simple_generated_id(tokens, index + 1, site, reference_start, None, None, scope_end, renames);
            index = scope_end.max(index + 3);
        } else {
            index += 1;
        }
    }
}

fn collect_match_arm_binding_renames(tokens: &[ExpandedToken], site: usize, renames: &mut Vec<BindingRename>) {
    let mut index = 0;
    while index < tokens.len() {
        if tokens[index].from_capture || !matches!(tokens[index].token.token, Token::Match) {
            index += 1;
            continue;
        }
        let Some(body_start) = find_match_body_start(tokens, index + 1) else {
            index += 1;
            continue;
        };
        let Some(body_end) = find_matching_delimiter(tokens, body_start, Token::LBrace, Token::RBrace) else {
            index += 1;
            continue;
        };
        let mut arm_start = body_start + 1;
        while arm_start < body_end {
            if matches!(tokens[arm_start].token.token, Token::Comma | Token::Semicolon) {
                arm_start += 1;
                continue;
            }
            let Some((pattern, consumed)) = parse_generated_pattern_prefix(tokens, arm_start) else {
                arm_start += 1;
                continue;
            };
            let Some(arrow) = find_top_level_token(tokens, arm_start + consumed, body_end, |token| {
                matches!(token, Token::Arrow)
            }) else {
                arm_start += consumed.max(1);
                continue;
            };
            let arm_end = find_match_arm_end(tokens, arrow + 1, body_end);
            collect_pattern_binding_renames(
                tokens, arm_start, consumed, &pattern, site, arm_start, None, None, arm_end, renames,
            );
            arm_start = arm_end;
        }
        index = body_end + 1;
    }
}

fn collect_function_param_binding_renames(tokens: &[ExpandedToken], site: usize, renames: &mut Vec<BindingRename>) {
    let mut index = 0;
    while index + 2 < tokens.len() {
        if tokens[index].from_capture || !matches!(tokens[index].token.token, Token::Fn) {
            index += 1;
            continue;
        }
        let Some(open) = find_next_token(tokens, index + 1, |token| matches!(token, Token::LParen)) else {
            index += 1;
            continue;
        };
        let Some(close) = find_matching_delimiter(tokens, open, Token::LParen, Token::RParen) else {
            index += 1;
            continue;
        };
        if tokens
            .get(close + 1)
            .is_some_and(|token| matches!(token.token.token, Token::Arrow))
        {
            index = close + 1;
            continue;
        }
        let Some(body_start) = find_function_body_start(tokens, close + 1) else {
            index = close + 1;
            continue;
        };
        let Some(scope_end) = find_matching_delimiter(tokens, body_start, Token::LBrace, Token::RBrace) else {
            index = close + 1;
            continue;
        };
        collect_parameter_list_binding_renames(tokens, open + 1, close, site, scope_end, renames);
        index = close + 1;
    }
}

fn collect_closure_param_binding_renames(tokens: &[ExpandedToken], site: usize, renames: &mut Vec<BindingRename>) {
    let mut index = 0;
    while index < tokens.len() {
        if tokens[index].from_capture
            || !matches!(tokens[index].token.token, Token::Pipe)
            || !is_closure_start_context(tokens, index)
        {
            index += 1;
            continue;
        }
        let Some(close) = find_closure_param_end(tokens, index + 1) else {
            index += 1;
            continue;
        };
        let scope_end = find_closure_body_end(tokens, close + 1).unwrap_or(tokens.len());
        collect_parameter_list_binding_renames(tokens, index + 1, close, site, scope_end, renames);
        index = scope_end.max(close + 1);
    }
}

fn collect_fn_closure_param_binding_renames(tokens: &[ExpandedToken], site: usize, renames: &mut Vec<BindingRename>) {
    let mut index = 0;
    while index + 3 < tokens.len() {
        if tokens[index].from_capture
            || !matches!(tokens[index].token.token, Token::Fn)
            || !matches!(tokens[index + 1].token.token, Token::LParen)
        {
            index += 1;
            continue;
        }
        let Some(close) = find_matching_delimiter(tokens, index + 1, Token::LParen, Token::RParen) else {
            index += 1;
            continue;
        };
        if !tokens
            .get(close + 1)
            .is_some_and(|token| matches!(token.token.token, Token::Arrow))
        {
            index = close + 1;
            continue;
        }
        let body_start = close + 2;
        let scope_end = find_closure_body_end(tokens, body_start).unwrap_or(tokens.len());
        collect_parameter_list_binding_renames(tokens, index + 2, close, site, scope_end, renames);
        index = scope_end.max(body_start);
    }
}

fn collect_parameter_list_binding_renames(
    tokens: &[ExpandedToken],
    start: usize,
    end: usize,
    site: usize,
    scope_end: usize,
    renames: &mut Vec<BindingRename>,
) {
    let mut bindings = Vec::new();
    let mut index = start;
    while index < end {
        match &tokens[index].token.token {
            Token::Id(_) => {
                collect_parameter_binding(tokens, index, end, &mut bindings);
                index = skip_parameter_tail(tokens, index + 1, end);
            }
            Token::LBrace => {
                let Some(close) = find_matching_delimiter(tokens, index, Token::LBrace, Token::RBrace) else {
                    index += 1;
                    continue;
                };
                let mut named_index = index + 1;
                while named_index < close {
                    if matches!(tokens[named_index].token.token, Token::Id(_)) {
                        collect_parameter_binding(tokens, named_index, close, &mut bindings);
                        named_index = skip_parameter_tail(tokens, named_index + 1, close);
                    } else {
                        named_index += 1;
                    }
                }
                index = close + 1;
            }
            _ => index += 1,
        }
    }
    push_parameter_binding_renames(bindings, site, scope_end, renames);
}

fn collect_parameter_binding(tokens: &[ExpandedToken], index: usize, end: usize, bindings: &mut Vec<ParameterBinding>) {
    if tokens.get(index).is_some_and(|token| !token.from_capture)
        && let Token::Id(name) = &tokens[index].token.token
        && name != "_"
    {
        let (excluded_start, excluded_end) = parameter_default_exclusion(tokens, index, end);
        bindings.push(ParameterBinding {
            name: name.clone(),
            index,
            excluded_start,
            excluded_end,
        });
    }
}

fn push_parameter_binding_renames(
    bindings: Vec<ParameterBinding>,
    site: usize,
    scope_end: usize,
    renames: &mut Vec<BindingRename>,
) {
    let mut seen = Vec::<String>::new();
    for binding in &bindings {
        if seen.iter().any(|name| name == &binding.name) {
            continue;
        }
        seen.push(binding.name.clone());
        let replacement = format!("__lk_macro_{site}_{}_{}", binding.index, binding.name);
        for same_name in bindings.iter().filter(|candidate| candidate.name == binding.name) {
            renames.push(BindingRename {
                name: same_name.name.clone(),
                replacement: replacement.clone(),
                binding_index: same_name.index,
                reference_start: same_name.index,
                excluded_start: same_name.excluded_start,
                excluded_end: same_name.excluded_end,
                end: scope_end,
            });
        }
    }
}

fn collect_pattern_binding_renames(
    tokens: &[ExpandedToken],
    start: usize,
    consumed: usize,
    pattern: &Pattern,
    site: usize,
    reference_start: usize,
    excluded_start: Option<usize>,
    excluded_end: Option<usize>,
    scope_end: usize,
    renames: &mut Vec<BindingRename>,
) {
    let mut names = Vec::new();
    collect_pattern_names(pattern, &mut names);
    for name in names {
        let binding_indices = find_generated_ids_in_range(tokens, start, start + consumed, &name);
        let Some(first_index) = binding_indices.first().copied() else {
            continue;
        };
        let replacement = format!("__lk_macro_{site}_{first_index}_{name}");
        for index in binding_indices {
            renames.push(BindingRename {
                name: name.clone(),
                replacement: replacement.clone(),
                binding_index: index,
                reference_start,
                excluded_start,
                excluded_end,
                end: scope_end,
            });
        }
    }
}

fn collect_pattern_names(pattern: &Pattern, names: &mut Vec<String>) {
    match pattern {
        Pattern::Variable(name) if name != "_" => push_pattern_name(names, name),
        Pattern::List { patterns, rest } => {
            for pattern in patterns {
                collect_pattern_names(pattern, names);
            }
            if let Some(rest) = rest
                && rest != "_"
            {
                push_pattern_name(names, rest);
            }
        }
        Pattern::Map { patterns, rest } => {
            for (_, pattern) in patterns {
                collect_pattern_names(pattern, names);
            }
            if let Some(rest) = rest
                && rest != "_"
            {
                push_pattern_name(names, rest);
            }
        }
        Pattern::Or(patterns) => {
            for pattern in patterns {
                collect_pattern_names(pattern, names);
            }
        }
        Pattern::Guard { pattern, .. } => collect_pattern_names(pattern, names),
        Pattern::Literal(_) | Pattern::Wildcard | Pattern::Range { .. } | Pattern::Variable(_) => {}
    }
}

fn push_pattern_name(names: &mut Vec<String>, name: &str) {
    if !names.iter().any(|existing| existing == name) {
        names.push(name.to_string());
    }
}

fn find_generated_ids_in_range(tokens: &[ExpandedToken], start: usize, end: usize, name: &str) -> Vec<usize> {
    (start..end.min(tokens.len()))
        .filter(|index| {
            !tokens[*index].from_capture
                && matches!(&tokens[*index].token.token, Token::Id(candidate) if candidate == name)
        })
        .collect()
}

fn collect_simple_generated_id(
    tokens: &[ExpandedToken],
    index: usize,
    site: usize,
    reference_start: usize,
    excluded_start: Option<usize>,
    excluded_end: Option<usize>,
    scope_end: usize,
    renames: &mut Vec<BindingRename>,
) {
    if tokens.get(index).is_some_and(|token| !token.from_capture)
        && let Token::Id(name) = &tokens[index].token.token
        && name != "_"
    {
        renames.push(BindingRename {
            name: name.clone(),
            replacement: format!("__lk_macro_{site}_{index}_{name}"),
            binding_index: index,
            reference_start,
            excluded_start,
            excluded_end,
            end: scope_end,
        });
    }
}

fn rename_for_identifier<'a>(renames: &'a [BindingRename], name: &str, index: usize) -> Option<&'a str> {
    renames
        .iter()
        .rev()
        .find(|rename| {
            rename.name == name
                && (index == rename.binding_index || (index >= rename.reference_start && index < rename.end))
                && !rename_excludes_index(rename, index)
        })
        .map(|rename| rename.replacement.as_str())
}

fn rename_excludes_index(rename: &BindingRename, index: usize) -> bool {
    match (rename.excluded_start, rename.excluded_end) {
        (Some(start), Some(end)) => index >= start && index < end,
        _ => false,
    }
}

fn is_generated_binding_identifier(renames: &[BindingRename], name: &str, index: usize) -> bool {
    renames
        .iter()
        .any(|rename| rename.name == name && rename.binding_index == index)
}

fn is_hygienic_identifier_reference(tokens: &[ExpandedToken], index: usize) -> bool {
    if tokens
        .get(index.wrapping_sub(1))
        .is_some_and(|previous| matches!(previous.token.token, Token::Dot | Token::OptionalDot))
    {
        return false;
    }
    if is_hygiene_preserved_identifier_reference(tokens, index)
        || is_type_annotation_identifier_reference(tokens, index)
        || is_semantic_identifier_definition(tokens, index)
    {
        return false;
    }
    if tokens
        .get(index + 1)
        .is_some_and(|next| matches!(next.token.token, Token::Colon))
        && !tokens
            .get(index + 2)
            .is_some_and(|next| matches!(next.token.token, Token::Assign))
        && is_expression_name_key(tokens, index)
    {
        return false;
    }
    true
}

fn is_expression_name_key(tokens: &[ExpandedToken], index: usize) -> bool {
    let Some(container_start) = innermost_enclosing_delimiter(tokens, index) else {
        return false;
    };
    match tokens[container_start].token.token {
        Token::LBrace => {
            !is_declaration_block(tokens, container_start)
                && !is_function_named_parameter_block(tokens, container_start)
        }
        Token::LParen => !is_function_parameter_list(tokens, container_start),
        _ => false,
    }
}

fn parse_generated_pattern_prefix(tokens: &[ExpandedToken], start: usize) -> Option<(Pattern, usize)> {
    if start >= tokens.len() || tokens[start].from_capture {
        return None;
    }
    let plain_tokens = tokens[start..]
        .iter()
        .map(|token| token.token.token.clone())
        .collect::<Vec<_>>();
    ExprParser::new(&plain_tokens).parse_pattern_prefix().ok()
}

fn generated_binding_scope_end(tokens: &[ExpandedToken], index: usize) -> usize {
    innermost_enclosing_delimiter(tokens, index)
        .filter(|open| matches!(tokens[*open].token.token, Token::LBrace))
        .and_then(|open| find_matching_delimiter(tokens, open, Token::LBrace, Token::RBrace))
        .unwrap_or(tokens.len())
}

fn find_statement_reference_start(tokens: &[ExpandedToken], index: usize) -> Option<usize> {
    let scope_end = generated_binding_scope_end(tokens, index);
    top_level_boundary_in_range(tokens, index, scope_end, |token| matches!(token, Token::Semicolon))
        .map(|index| index + 1)
}

fn find_select_case_end(tokens: &[ExpandedToken], start: usize) -> Option<usize> {
    let arrow = find_top_level_token(tokens, start, tokens.len(), |token| matches!(token, Token::Arrow))?;
    Some(
        find_top_level_token(tokens, arrow + 1, tokens.len(), |token| {
            matches!(token, Token::Semicolon | Token::Comma)
        })
        .map_or(tokens.len(), |index| index + 1),
    )
}

fn find_select_case_binding_reference_start(tokens: &[ExpandedToken], start: usize) -> Option<usize> {
    find_top_level_token(tokens, start, tokens.len(), |token| {
        matches!(token, Token::If | Token::Arrow)
    })
}

fn find_match_body_start(tokens: &[ExpandedToken], start: usize) -> Option<usize> {
    let mut paren = 0i32;
    let mut bracket = 0i32;
    for (index, token) in tokens.iter().enumerate().skip(start) {
        match &token.token.token {
            Token::LBrace if paren == 0 && bracket == 0 => return Some(index),
            Token::LParen => paren += 1,
            Token::RParen => paren -= 1,
            Token::LBracket => bracket += 1,
            Token::RBracket => bracket -= 1,
            _ => {}
        }
    }
    None
}

fn find_match_arm_end(tokens: &[ExpandedToken], start: usize, body_end: usize) -> usize {
    find_top_level_token(tokens, start, body_end, |token| {
        matches!(token, Token::Comma | Token::Semicolon)
    })
    .map_or(body_end, |index| index + 1)
}

fn find_for_pattern_end(tokens: &[ExpandedToken], start: usize) -> Option<usize> {
    find_top_level_token(tokens, start, tokens.len(), |token| matches!(token, Token::In))
}

fn find_if_while_let_value_start(tokens: &[ExpandedToken], start: usize) -> Option<usize> {
    find_top_level_token(tokens, start, tokens.len(), |token| matches!(token, Token::Assign))
}

fn is_control_flow_let_keyword(tokens: &[ExpandedToken], index: usize) -> bool {
    matches!(
        previous_significant_token(tokens, index),
        Some(Token::If | Token::While)
    )
}

fn find_for_body_start(tokens: &[ExpandedToken], start: usize) -> Option<usize> {
    find_next_token_including_captures(tokens, start, |token| matches!(token, Token::LBrace))
}

fn find_control_flow_body_start(tokens: &[ExpandedToken], start: usize) -> Option<usize> {
    find_next_token_including_captures(tokens, start, |token| matches!(token, Token::LBrace))
}

fn find_function_body_start(tokens: &[ExpandedToken], start: usize) -> Option<usize> {
    find_next_token_including_captures(tokens, start, |token| matches!(token, Token::LBrace))
}

fn find_closure_param_end(tokens: &[ExpandedToken], start: usize) -> Option<usize> {
    let mut index = start;
    while index < tokens.len() {
        if !tokens[index].from_capture && matches!(tokens[index].token.token, Token::Pipe) {
            return Some(index);
        }
        match &tokens[index].token.token {
            Token::LParen | Token::LBracket | Token::LBrace => {
                index = skip_group(tokens, index)?;
            }
            _ => index += 1,
        }
    }
    None
}

fn find_closure_body_end(tokens: &[ExpandedToken], start: usize) -> Option<usize> {
    if matches!(tokens.get(start).map(|token| &token.token.token), Some(Token::LBrace)) {
        return find_matching_delimiter(tokens, start, Token::LBrace, Token::RBrace).map(|index| index + 1);
    }
    top_level_boundary_in_range(tokens, start, tokens.len(), |token| {
        matches!(
            token,
            Token::Comma | Token::Semicolon | Token::RParen | Token::RBracket | Token::RBrace
        )
    })
    .or(Some(tokens.len()))
}

fn is_closure_start_context(tokens: &[ExpandedToken], index: usize) -> bool {
    index == 0
        || matches!(
            tokens[index - 1].token.token,
            Token::LParen
                | Token::LBracket
                | Token::LBrace
                | Token::Comma
                | Token::Assign
                | Token::Arrow
                | Token::Return
                | Token::Let
                | Token::Const
                | Token::If
                | Token::Else
                | Token::While
                | Token::For
                | Token::In
                | Token::Colon
        )
}

fn skip_parameter_tail(tokens: &[ExpandedToken], start: usize, end: usize) -> usize {
    find_top_level_token(tokens, start, end, |token| matches!(token, Token::Comma)).map_or(end, |index| index + 1)
}

fn parameter_default_exclusion(
    tokens: &[ExpandedToken],
    param_index: usize,
    end: usize,
) -> (Option<usize>, Option<usize>) {
    let segment_end =
        find_top_level_token(tokens, param_index + 1, end, |token| matches!(token, Token::Comma)).unwrap_or(end);
    let Some(assign) = find_top_level_token(tokens, param_index + 1, segment_end, |token| {
        matches!(token, Token::Assign)
    }) else {
        return (None, None);
    };
    (Some(assign + 1), Some(segment_end))
}

fn find_next_token(tokens: &[ExpandedToken], start: usize, predicate: impl Fn(&Token) -> bool) -> Option<usize> {
    tokens
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, token)| (!token.from_capture && predicate(&token.token.token)).then_some(index))
}

fn find_next_token_including_captures(
    tokens: &[ExpandedToken],
    start: usize,
    predicate: impl Fn(&Token) -> bool,
) -> Option<usize> {
    tokens
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, token)| predicate(&token.token.token).then_some(index))
}

fn find_top_level_token(
    tokens: &[ExpandedToken],
    start: usize,
    end: usize,
    predicate: impl Fn(&Token) -> bool,
) -> Option<usize> {
    top_level_token_indices(tokens, start, end)
        .into_iter()
        .find(|index| predicate(&tokens[*index].token.token))
}

fn skip_group(tokens: &[ExpandedToken], open_index: usize) -> Option<usize> {
    let (open, close) = match tokens[open_index].token.token {
        Token::LParen => (Token::LParen, Token::RParen),
        Token::LBracket => (Token::LBracket, Token::RBracket),
        Token::LBrace => (Token::LBrace, Token::RBrace),
        _ => return None,
    };
    find_matching_delimiter(tokens, open_index, open, close).map(|index| index + 1)
}

fn statement_start_before(tokens: &[ExpandedToken], index: usize) -> usize {
    let mut cursor = index.min(tokens.len());
    while cursor > 0 {
        cursor -= 1;
        if matches!(
            tokens[cursor].token.token,
            Token::Semicolon | Token::LBrace | Token::RBrace
        ) {
            return cursor + 1;
        }
    }
    0
}

fn last_top_level_token_in_range(
    tokens: &[ExpandedToken],
    start: usize,
    end: usize,
    predicate: impl Fn(&Token) -> bool,
) -> Option<usize> {
    let mut found = None;
    for index in top_level_token_indices(tokens, start, end) {
        if predicate(&tokens[index].token.token) {
            found = Some(index);
        }
    }
    found
}

fn top_level_token_in_range(
    tokens: &[ExpandedToken],
    start: usize,
    end: usize,
    predicate: impl Fn(&Token) -> bool,
) -> Option<usize> {
    top_level_token_indices(tokens, start, end)
        .into_iter()
        .find(|index| predicate(&tokens[*index].token.token))
}

fn top_level_token_indices(tokens: &[ExpandedToken], start: usize, end: usize) -> Vec<usize> {
    let mut paren = 0i32;
    let mut bracket = 0i32;
    let mut brace = 0i32;
    let mut out = Vec::new();
    for (index, token) in tokens.iter().enumerate().take(end.min(tokens.len())).skip(start) {
        match &token.token.token {
            Token::LParen => paren += 1,
            Token::RParen => paren -= 1,
            Token::LBracket => bracket += 1,
            Token::RBracket => bracket -= 1,
            Token::LBrace => brace += 1,
            Token::RBrace => brace -= 1,
            _ if paren == 0 && bracket == 0 && brace == 0 => out.push(index),
            _ => {}
        }
    }
    out
}

fn top_level_boundary_in_range(
    tokens: &[ExpandedToken],
    start: usize,
    end: usize,
    predicate: impl Fn(&Token) -> bool,
) -> Option<usize> {
    let mut paren = 0i32;
    let mut bracket = 0i32;
    let mut brace = 0i32;
    for (index, token) in tokens.iter().enumerate().take(end.min(tokens.len())).skip(start) {
        if paren == 0 && bracket == 0 && brace == 0 && predicate(&token.token.token) {
            return Some(index);
        }
        match &token.token.token {
            Token::LParen => paren += 1,
            Token::RParen => paren -= 1,
            Token::LBracket => bracket += 1,
            Token::RBracket => bracket -= 1,
            Token::LBrace => brace += 1,
            Token::RBrace => brace -= 1,
            _ => {}
        }
    }
    None
}

fn matching_delimiter_end_for_open(tokens: &[ExpandedToken], open: usize) -> Option<usize> {
    let (open_token, close_token) = match tokens.get(open).map(|token| &token.token.token) {
        Some(Token::LParen) => (Token::LParen, Token::RParen),
        Some(Token::LBrace) => (Token::LBrace, Token::RBrace),
        Some(Token::LBracket) => (Token::LBracket, Token::RBracket),
        _ => return None,
    };
    find_matching_delimiter(tokens, open, open_token, close_token)
}

fn segment_start_after_delimiter(tokens: &[ExpandedToken], start: usize, index: usize) -> usize {
    let mut segment_start = start;
    for token_index in top_level_token_indices(tokens, start, index) {
        if matches!(tokens[token_index].token.token, Token::Comma) {
            segment_start = token_index + 1;
        }
    }
    segment_start
}

fn segment_end_before_delimiter(tokens: &[ExpandedToken], start: usize, end: usize) -> usize {
    top_level_token_in_range(tokens, start, end, |token| matches!(token, Token::Comma)).unwrap_or(end)
}

fn innermost_enclosing_delimiter(tokens: &[ExpandedToken], index: usize) -> Option<usize> {
    let mut stack = Vec::new();
    for (cursor, token) in tokens.iter().enumerate().take(index) {
        match token.token.token {
            Token::LParen | Token::LBrace | Token::LBracket => stack.push(cursor),
            Token::RParen | Token::RBrace | Token::RBracket => {
                stack.pop();
            }
            _ => {}
        }
    }
    stack.pop()
}

fn is_declaration_block(tokens: &[ExpandedToken], open_index: usize) -> bool {
    open_index > 0
        && matches!(
            tokens[open_index - 1].token.token,
            Token::If
                | Token::Else
                | Token::While
                | Token::For
                | Token::Fn
                | Token::Struct
                | Token::Trait
                | Token::Impl
        )
}

fn is_function_parameter_list(tokens: &[ExpandedToken], open_index: usize) -> bool {
    if matches!(previous_significant_token(tokens, open_index), Some(Token::Fn)) {
        return true;
    }
    matches!(previous_significant_token(tokens, open_index), Some(Token::Id(_)))
        && previous_significant_token_before(tokens, open_index.saturating_sub(1))
            .is_some_and(|token| matches!(token, Token::Fn))
}

fn is_function_named_parameter_block(tokens: &[ExpandedToken], open_index: usize) -> bool {
    let Some(parent) = innermost_enclosing_delimiter(tokens, open_index) else {
        return false;
    };
    matches!(tokens[parent].token.token, Token::LParen) && is_function_parameter_list(tokens, parent)
}

fn previous_significant_token(tokens: &[ExpandedToken], index: usize) -> Option<&Token> {
    index
        .checked_sub(1)
        .and_then(|start| tokens[..=start].iter().rev().map(|token| &token.token.token).next())
}

fn previous_significant_token_before(tokens: &[ExpandedToken], index: usize) -> Option<&Token> {
    tokens
        .get(..index)
        .and_then(|prefix| prefix.iter().rev().map(|token| &token.token.token).next())
}

fn find_matching_delimiter(tokens: &[ExpandedToken], open_index: usize, open: Token, close: Token) -> Option<usize> {
    let mut depth = 0i32;
    for (index, token) in tokens.iter().enumerate().skip(open_index) {
        if token.token.token == open {
            depth += 1;
        } else if token.token.token == close {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}
