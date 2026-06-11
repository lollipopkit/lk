use crate::token::Token;

use crate::macro_system::ExpandedToken;

pub(super) fn is_type_annotation_identifier_reference(tokens: &[ExpandedToken], index: usize) -> bool {
    is_let_const_type_annotation_reference(tokens, index)
        || is_delimited_type_annotation_reference(tokens, index)
        || is_function_return_type_reference(tokens, index)
        || is_type_alias_target_reference(tokens, index)
        || is_impl_header_type_reference(tokens, index)
}

pub(super) fn is_hygiene_preserved_identifier_reference(tokens: &[ExpandedToken], index: usize) -> bool {
    matches!(tokens.get(index).map(|token| &token.token.token), Some(Token::Id(_)))
        && (is_attribute_identifier_reference(tokens, index)
            || is_import_path_identifier_reference(tokens, index)
            || is_path_segment_identifier_reference(tokens, index)
            || is_struct_literal_type_identifier_reference(tokens, index))
}

pub(super) fn is_semantic_identifier_definition(tokens: &[ExpandedToken], index: usize) -> bool {
    matches!(
        super::previous_significant_token(tokens, index),
        Some(Token::Fn | Token::Struct | Token::Trait | Token::Type)
    ) || is_struct_field_declaration_name(tokens, index)
}

fn is_let_const_type_annotation_reference(tokens: &[ExpandedToken], index: usize) -> bool {
    let statement_start = super::statement_start_before(tokens, index);
    if !matches!(
        tokens.get(statement_start).map(|token| &token.token.token),
        Some(Token::Let | Token::Const)
    ) {
        return false;
    }
    let Some(colon) = super::last_top_level_token_in_range(tokens, statement_start + 1, index, |token| {
        matches!(token, Token::Colon)
    }) else {
        return false;
    };
    if super::top_level_token_in_range(tokens, statement_start + 1, colon, |token| {
        matches!(token, Token::Assign)
    })
    .is_some()
    {
        return false;
    }
    super::top_level_token_in_range(tokens, colon + 1, index, |token| {
        matches!(token, Token::Assign | Token::Semicolon)
    })
    .is_none()
        && type_annotation_segment_contains(tokens, colon + 1, index + 1)
}

fn is_delimited_type_annotation_reference(tokens: &[ExpandedToken], index: usize) -> bool {
    enclosing_delimiters(tokens, index).into_iter().rev().any(|parent| {
        let context_accepts_type_annotation = match tokens[parent].token.token {
            Token::LParen => super::is_function_parameter_list(tokens, parent),
            Token::LBrace => {
                super::is_function_named_parameter_block(tokens, parent)
                    || (super::is_declaration_block(tokens, parent) && !is_for_object_pattern_block(tokens, parent))
                    || is_struct_body(tokens, parent)
            }
            _ => false,
        };
        if !context_accepts_type_annotation {
            return false;
        }
        let Some(end) = super::matching_delimiter_end_for_open(tokens, parent) else {
            return false;
        };
        let segment_start = super::segment_start_after_delimiter(tokens, parent + 1, index);
        let segment_end = super::segment_end_before_delimiter(tokens, index + 1, end);
        let Some(colon) =
            super::last_top_level_token_in_range(tokens, segment_start, index, |token| matches!(token, Token::Colon))
        else {
            return false;
        };
        if super::top_level_token_in_range(tokens, colon + 1, index, |token| matches!(token, Token::Assign)).is_some() {
            return false;
        }
        let segment_end =
            super::top_level_token_in_range(tokens, index + 1, segment_end, |token| matches!(token, Token::Assign))
                .unwrap_or(segment_end);
        type_annotation_segment_contains(tokens, colon + 1, segment_end)
    })
}

fn is_for_object_pattern_block(tokens: &[ExpandedToken], open_index: usize) -> bool {
    matches!(super::previous_significant_token(tokens, open_index), Some(Token::For))
}

fn is_function_return_type_reference(tokens: &[ExpandedToken], index: usize) -> bool {
    let statement_start = super::statement_start_before(tokens, index);
    if !matches!(
        tokens.get(statement_start).map(|token| &token.token.token),
        Some(Token::Fn)
    ) {
        return false;
    }
    let Some(arrow) = super::last_top_level_token_in_range(tokens, statement_start + 1, index, |token| {
        matches!(token, Token::FnArrow)
    }) else {
        return false;
    };
    if super::top_level_token_in_range(tokens, arrow + 1, index, |token| {
        matches!(token, Token::LBrace | Token::Semicolon)
    })
    .is_some()
    {
        return false;
    }
    let segment_end = super::top_level_boundary_in_range(tokens, index + 1, tokens.len(), |token| {
        matches!(token, Token::LBrace | Token::Semicolon)
    })
    .unwrap_or(tokens.len());
    type_annotation_segment_contains(tokens, arrow + 1, segment_end)
}

fn is_type_alias_target_reference(tokens: &[ExpandedToken], index: usize) -> bool {
    let statement_start = super::statement_start_before(tokens, index);
    if !matches!(
        tokens.get(statement_start).map(|token| &token.token.token),
        Some(Token::Type)
    ) {
        return false;
    }
    let Some(assign) = super::last_top_level_token_in_range(tokens, statement_start + 1, index, |token| {
        matches!(token, Token::Assign)
    }) else {
        return false;
    };
    if super::top_level_token_in_range(tokens, assign + 1, index, |token| matches!(token, Token::Semicolon)).is_some() {
        return false;
    }
    let segment_end = super::top_level_token_in_range(tokens, index + 1, tokens.len(), |token| {
        matches!(token, Token::Semicolon)
    })
    .unwrap_or(tokens.len());
    type_annotation_segment_contains(tokens, assign + 1, segment_end)
}

fn is_impl_header_type_reference(tokens: &[ExpandedToken], index: usize) -> bool {
    let statement_start = super::statement_start_before(tokens, index);
    if !matches!(
        tokens.get(statement_start).map(|token| &token.token.token),
        Some(Token::Impl)
    ) {
        return false;
    }
    let Some(body_start) = super::top_level_boundary_in_range(tokens, statement_start + 1, tokens.len(), |token| {
        matches!(token, Token::LBrace)
    }) else {
        return false;
    };
    index > statement_start
        && index < body_start
        && matches!(tokens[index].token.token, Token::Id(_))
        && tokens[statement_start + 1..body_start]
            .iter()
            .all(|token| is_impl_header_token(&token.token.token))
}

fn is_import_path_identifier_reference(tokens: &[ExpandedToken], index: usize) -> bool {
    let Some(statement_start) = import_statement_start_before(tokens, index) else {
        return false;
    };
    let statement_end = super::top_level_token_in_range(tokens, statement_start + 1, tokens.len(), |token| {
        matches!(token, Token::Semicolon)
    })
    .unwrap_or(tokens.len());
    index > statement_start && index < statement_end && matches!(tokens[index].token.token, Token::Id(_))
}

fn import_statement_start_before(tokens: &[ExpandedToken], index: usize) -> Option<usize> {
    let mut cursor = index.min(tokens.len());
    while cursor > 0 {
        cursor -= 1;
        match tokens[cursor].token.token {
            Token::Use => return Some(cursor),
            Token::Semicolon => return None,
            _ => {}
        }
    }
    None
}

fn is_path_segment_identifier_reference(tokens: &[ExpandedToken], index: usize) -> bool {
    matches!(tokens.get(index).map(|token| &token.token.token), Some(Token::Id(_)))
        && (matches!(
            tokens
                .get(index.checked_sub(1).unwrap_or(tokens.len()))
                .map(|token| &token.token.token),
            Some(Token::ColonColon)
        ) || matches!(
            tokens.get(index + 1).map(|token| &token.token.token),
            Some(Token::ColonColon)
        ))
}

fn is_struct_literal_type_identifier_reference(tokens: &[ExpandedToken], index: usize) -> bool {
    if !matches!(tokens.get(index).map(|token| &token.token.token), Some(Token::Id(_)))
        || !matches!(
            tokens.get(index + 1).map(|token| &token.token.token),
            Some(Token::LBrace)
        )
    {
        return false;
    }
    !matches!(
        super::previous_significant_token(tokens, index),
        Some(
            Token::If
                | Token::While
                | Token::For
                | Token::In
                | Token::Match
                | Token::Fn
                | Token::Struct
                | Token::Trait
                | Token::Impl
                | Token::Type
                | Token::Let
                | Token::Const
                | Token::Dot
                | Token::OptionalDot
                | Token::ColonColon
        )
    )
}

fn is_attribute_identifier_reference(tokens: &[ExpandedToken], index: usize) -> bool {
    let Some(open) = attribute_open_before(tokens, index) else {
        return false;
    };
    let Some(close) = super::matching_delimiter_end_for_open(tokens, open) else {
        return false;
    };
    index > open && index < close
}

fn attribute_open_before(tokens: &[ExpandedToken], index: usize) -> Option<usize> {
    let mut cursor = index.min(tokens.len());
    while cursor > 0 {
        cursor -= 1;
        match tokens[cursor].token.token {
            Token::LBracket
                if cursor > 0
                    && matches!(
                        tokens.get(cursor - 1).map(|token| &token.token.token),
                        Some(Token::Hash)
                    ) =>
            {
                return Some(cursor);
            }
            Token::Semicolon | Token::LBrace | Token::RBrace => return None,
            _ => {}
        }
    }
    None
}

fn type_annotation_segment_contains(tokens: &[ExpandedToken], start: usize, end: usize) -> bool {
    start < end
        && tokens[start..end.min(tokens.len())]
            .iter()
            .any(|token| matches!(token.token.token, Token::Id(_)))
        && tokens[start..end.min(tokens.len())]
            .iter()
            .all(|token| is_type_annotation_token(&token.token.token))
}

fn is_type_annotation_token(token: &Token) -> bool {
    matches!(
        token,
        Token::Id(_)
            | Token::Lt
            | Token::Gt
            | Token::Comma
            | Token::LParen
            | Token::RParen
            | Token::LBrace
            | Token::RBrace
            | Token::Colon
            | Token::Assign
            | Token::Arrow
            | Token::FnArrow
            | Token::Question
            | Token::Pipe
            | Token::LBracket
            | Token::RBracket
            | Token::ColonColon
    )
}

fn is_impl_header_token(token: &Token) -> bool {
    matches!(token, Token::For) || is_type_annotation_token(token)
}

fn is_struct_field_declaration_name(tokens: &[ExpandedToken], index: usize) -> bool {
    if !matches!(tokens.get(index).map(|token| &token.token.token), Some(Token::Id(_))) {
        return false;
    }
    let Some(parent) = super::innermost_enclosing_delimiter(tokens, index) else {
        return false;
    };
    if !matches!(tokens[parent].token.token, Token::LBrace) || !is_struct_body(tokens, parent) {
        return false;
    }
    let Some(body_end) = super::matching_delimiter_end_for_open(tokens, parent) else {
        return false;
    };
    let segment_start = super::segment_start_after_delimiter(tokens, parent + 1, index);
    index == segment_start
        && super::segment_end_before_delimiter(tokens, index + 1, body_end) > index
        && super::top_level_token_in_range(tokens, index + 1, body_end, |token| {
            matches!(token, Token::Colon | Token::Comma)
        })
        .is_some()
}

fn is_struct_body(tokens: &[ExpandedToken], open_index: usize) -> bool {
    matches!(
        super::previous_significant_token_before(tokens, open_index.saturating_sub(1)),
        Some(Token::Struct)
    )
}

fn enclosing_delimiters(tokens: &[ExpandedToken], index: usize) -> Vec<usize> {
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
    stack
}
