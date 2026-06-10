use std::collections::HashMap;

use crate::{
    ast::Parser as ExprParser,
    stmt::StmtParser,
    token::{ParseError, Span, Token},
};

use super::{
    Capture, ExpandedToken, FragmentKind, MacroDef, PatternElem, RepeatOp, SourceToken, TemplateElem, find_group,
    is_open_delim, token_lexeme, token_matches,
};

#[derive(Debug, Clone)]
enum PatternMatch {
    Matched(usize),
    Failed(String),
}

pub(super) fn expand_macro_invocation(
    definition: &MacroDef,
    input: &[SourceToken],
    call_span: &Span,
) -> Result<Vec<SourceToken>, ParseError> {
    let mut mismatches = Vec::new();
    for (rule_index, rule) in definition.rules.iter().enumerate() {
        let mut captures = HashMap::new();
        match match_pattern(&rule.matcher, input, 0, &mut captures)? {
            PatternMatch::Matched(pos) if pos == input.len() => {
                let expanded =
                    substitute_template(&rule.template, &captures, definition.crate_anchor.as_deref(), call_span)?;
                let expanded = apply_simple_hygiene(expanded, call_span.start.offset);
                return Ok(expanded.into_iter().map(|token| token.token).collect());
            }
            PatternMatch::Matched(pos) => mismatches.push(format!(
                "rule {}: matched a prefix but left unexpected {}",
                rule_index + 1,
                input_token_label(input, pos)
            )),
            PatternMatch::Failed(reason) => mismatches.push(format!("rule {}: {reason}", rule_index + 1)),
        }
    }
    Err(ParseError::with_span(
        format_no_matching_rule_error(&definition.name, &mismatches),
        call_span.clone(),
    ))
}

fn match_pattern(
    pattern: &[PatternElem],
    input: &[SourceToken],
    mut pos: usize,
    captures: &mut HashMap<String, Capture>,
) -> Result<PatternMatch, ParseError> {
    for (elem_index, elem) in pattern.iter().enumerate() {
        match elem {
            PatternElem::Token(expected) => {
                let Some(actual) = input.get(pos) else {
                    return Ok(PatternMatch::Failed(format!(
                        "expected `{}` at end of input",
                        token_lexeme(expected)
                    )));
                };
                if !token_matches(expected, &actual.token) {
                    return Ok(PatternMatch::Failed(format!(
                        "expected `{}` but found {}",
                        token_lexeme(expected),
                        input_token_label(input, pos)
                    )));
                }
                pos += 1;
            }
            PatternElem::MetaVar { name, kind, .. } => {
                let next_literal = next_literal_token(&pattern[elem_index + 1..]);
                let Some((end, capture)) = capture_fragment(*kind, input, pos, next_literal.as_ref())? else {
                    return Ok(PatternMatch::Failed(format!(
                        "expected `{}` fragment `${}` at {}",
                        fragment_kind_name(*kind),
                        name,
                        input_token_label(input, pos)
                    )));
                };
                captures.insert(name.clone(), Capture::single(capture));
                pos = end;
            }
            PatternElem::Repeat {
                elems, separator, op, ..
            } => {
                let mut repeats: Vec<HashMap<String, Capture>> = Vec::new();
                let mut current = pos;
                loop {
                    let mut local = HashMap::new();
                    match match_pattern(elems, input, current, &mut local)? {
                        PatternMatch::Matched(next) if next > current => {
                            repeats.push(local);
                            current = next;
                            if let Some(separator) = separator {
                                if input
                                    .get(current)
                                    .is_some_and(|token| token_matches(separator, &token.token))
                                {
                                    current += 1;
                                } else {
                                    break;
                                }
                            }
                        }
                        _ => break,
                    }
                    if *op == RepeatOp::Optional {
                        break;
                    }
                }
                if repeats.is_empty() && *op == RepeatOp::OneOrMore {
                    return Ok(PatternMatch::Failed(format!(
                        "expected at least one repetition match at {}",
                        input_token_label(input, current)
                    )));
                }
                merge_repeated_captures(captures, repeats);
                pos = current;
            }
        }
    }
    Ok(PatternMatch::Matched(pos))
}

fn format_no_matching_rule_error(name: &str, mismatches: &[String]) -> String {
    let mut message = format!("No matching rule for macro `{name}`");
    if !mismatches.is_empty() {
        message.push_str("\nMacro rule mismatch notes:");
        for mismatch in mismatches {
            message.push_str("\n  ");
            message.push_str(mismatch);
        }
    }
    message
}

fn input_token_label(input: &[SourceToken], pos: usize) -> String {
    input.get(pos).map_or_else(
        || "end of input".to_string(),
        |token| format!("`{}`", token_lexeme(&token.token)),
    )
}

fn fragment_kind_name(kind: FragmentKind) -> &'static str {
    match kind {
        FragmentKind::Expr => "expr",
        FragmentKind::Stmt => "stmt",
        FragmentKind::Block => "block",
        FragmentKind::Item => "item",
        FragmentKind::Ident => "ident",
        FragmentKind::Literal => "literal",
        FragmentKind::Tt => "tt",
        FragmentKind::Pat => "pat",
        FragmentKind::Ty => "ty",
        FragmentKind::Path => "path",
    }
}

fn capture_fragment(
    kind: FragmentKind,
    input: &[SourceToken],
    pos: usize,
    next_literal: Option<&Token>,
) -> Result<Option<(usize, Vec<SourceToken>)>, ParseError> {
    if pos >= input.len() {
        return Ok(None);
    }
    match kind {
        FragmentKind::Ident => match &input[pos].token {
            Token::Id(_) => Ok(Some((pos + 1, vec![input[pos].clone()]))),
            _ => Ok(None),
        },
        FragmentKind::Literal => {
            if matches!(
                input[pos].token,
                Token::Str(_)
                    | Token::TemplateString(_)
                    | Token::Int(_)
                    | Token::Float(_)
                    | Token::Bool(_)
                    | Token::Nil
            ) {
                Ok(Some((pos + 1, vec![input[pos].clone()])))
            } else {
                Ok(None)
            }
        }
        FragmentKind::Block => {
            if !matches!(input[pos].token, Token::LBrace) {
                return Ok(None);
            }
            let (_, end) = find_group(input, pos)?;
            Ok(Some((end + 1, input[pos..=end].to_vec())))
        }
        FragmentKind::Tt => {
            if is_open_delim(&input[pos].token) {
                let (_, end) = find_group(input, pos)?;
                Ok(Some((end + 1, input[pos..=end].to_vec())))
            } else {
                Ok(Some((pos + 1, vec![input[pos].clone()])))
            }
        }
        FragmentKind::Expr => Ok(capture_expr_fragment(input, pos, next_literal)
            .or_else(|| capture_scanned_fragment(kind, input, pos, next_literal))),
        FragmentKind::Stmt | FragmentKind::Item => Ok(capture_stmt_fragment(input, pos, next_literal)
            .or_else(|| capture_scanned_fragment(kind, input, pos, next_literal))),
        FragmentKind::Pat => Ok(capture_pat_fragment(input, pos, next_literal)
            .or_else(|| capture_scanned_fragment(kind, input, pos, next_literal))),
        FragmentKind::Ty => Ok(capture_ty_fragment(input, pos, next_literal)
            .or_else(|| capture_scanned_fragment(kind, input, pos, next_literal))),
        FragmentKind::Path => Ok(capture_path_fragment(input, pos, next_literal)
            .or_else(|| capture_scanned_fragment(kind, input, pos, next_literal))),
    }
}

fn capture_expr_fragment(
    input: &[SourceToken],
    pos: usize,
    next_literal: Option<&Token>,
) -> Option<(usize, Vec<SourceToken>)> {
    let tokens = source_tokens_to_tokens(&input[pos..]);
    let mut parser = ExprParser::new(&tokens);
    let Ok((_, consumed)) = parser.parse_prefix() else {
        return None;
    };
    capture_parser_prefix(input, pos, consumed, next_literal)
}

fn capture_stmt_fragment(
    input: &[SourceToken],
    pos: usize,
    next_literal: Option<&Token>,
) -> Option<(usize, Vec<SourceToken>)> {
    let tokens = source_tokens_to_tokens(&input[pos..]);
    let mut parser = StmtParser::new(&tokens);
    if parser.parse_statement().is_err() {
        return None;
    }
    capture_parser_prefix(input, pos, parser.pos, next_literal)
}

fn capture_pat_fragment(
    input: &[SourceToken],
    pos: usize,
    next_literal: Option<&Token>,
) -> Option<(usize, Vec<SourceToken>)> {
    let tokens = source_tokens_to_tokens(&input[pos..]);
    let mut parser = ExprParser::new(&tokens);
    let Ok((_, consumed)) = parser.parse_pattern_prefix() else {
        return None;
    };
    capture_parser_prefix(input, pos, consumed, next_literal)
}

fn capture_ty_fragment(
    input: &[SourceToken],
    pos: usize,
    next_literal: Option<&Token>,
) -> Option<(usize, Vec<SourceToken>)> {
    let consumed = type_prefix_len(input, pos, next_literal);
    let (_, capture) = capture_parser_prefix(input, pos, consumed, next_literal)?;
    parse_ty_fragment(&source_tokens_to_tokens(&capture)).then_some((pos + consumed, capture))
}

fn capture_path_fragment(
    input: &[SourceToken],
    pos: usize,
    next_literal: Option<&Token>,
) -> Option<(usize, Vec<SourceToken>)> {
    if !matches!(input.get(pos).map(|token| &token.token), Some(Token::Id(_))) {
        return None;
    }
    let mut end = pos + 1;
    while matches!(
        input.get(end).map(|token| &token.token),
        Some(Token::ColonColon | Token::Dot)
    ) && matches!(input.get(end + 1).map(|token| &token.token), Some(Token::Id(_)))
    {
        end += 2;
    }
    capture_parser_prefix(input, pos, end - pos, next_literal)
}

fn capture_parser_prefix(
    input: &[SourceToken],
    pos: usize,
    consumed: usize,
    next_literal: Option<&Token>,
) -> Option<(usize, Vec<SourceToken>)> {
    if consumed == 0 {
        return None;
    }
    let end = pos + consumed;
    if next_literal.is_some_and(|expected| {
        input
            .get(end)
            .is_none_or(|token| !token_matches(expected, &token.token))
    }) {
        return None;
    }
    Some((end, input[pos..end].to_vec()))
}

fn capture_scanned_fragment(
    kind: FragmentKind,
    input: &[SourceToken],
    pos: usize,
    next_literal: Option<&Token>,
) -> Option<(usize, Vec<SourceToken>)> {
    let end = capture_until(input, pos, next_literal);
    if end == pos {
        return None;
    }
    fragment_capture_is_valid(kind, &input[pos..end]).then(|| (end, input[pos..end].to_vec()))
}

fn source_tokens_to_tokens(tokens: &[SourceToken]) -> Vec<Token> {
    tokens.iter().map(|token| token.token.clone()).collect()
}

fn type_prefix_len(input: &[SourceToken], start: usize, next_literal: Option<&Token>) -> usize {
    let mut index = start;
    let mut paren = 0i32;
    let mut bracket = 0i32;
    let mut angle = 0i32;
    while index < input.len() {
        let token = &input[index].token;
        if paren == 0 && bracket == 0 && angle == 0 {
            if next_literal.is_some_and(|expected| token_matches(expected, token)) {
                break;
            }
            if matches!(
                token,
                Token::Comma
                    | Token::Semicolon
                    | Token::Assign
                    | Token::LBrace
                    | Token::RBrace
                    | Token::RParen
                    | Token::RBracket
            ) {
                break;
            }
        }
        match token {
            Token::Id(_) | Token::FnArrow | Token::Question | Token::Pipe => index += 1,
            Token::Lt => {
                angle += 1;
                index += 1;
            }
            Token::Gt => {
                if angle == 0 {
                    break;
                }
                angle -= 1;
                index += 1;
            }
            Token::LParen => {
                paren += 1;
                index += 1;
            }
            Token::RParen => {
                if paren == 0 {
                    break;
                }
                paren -= 1;
                index += 1;
            }
            Token::LBracket => {
                bracket += 1;
                index += 1;
            }
            Token::RBracket => {
                if bracket == 0 {
                    break;
                }
                bracket -= 1;
                index += 1;
            }
            Token::Comma if paren > 0 || bracket > 0 || angle > 0 => index += 1,
            _ => break,
        }
    }
    index - start
}

fn fragment_capture_is_valid(kind: FragmentKind, capture: &[SourceToken]) -> bool {
    let tokens = source_tokens_to_tokens(capture);
    match kind {
        FragmentKind::Expr => ExprParser::new(&tokens).parse().is_ok(),
        FragmentKind::Stmt => parse_stmt_fragment(&tokens),
        FragmentKind::Item => StmtParser::new(&tokens).parse_program().is_ok(),
        FragmentKind::Pat => parse_pat_fragment(&tokens),
        FragmentKind::Ty => parse_ty_fragment(&tokens),
        FragmentKind::Path => parse_path_fragment(&tokens),
        FragmentKind::Block | FragmentKind::Ident | FragmentKind::Literal | FragmentKind::Tt => true,
    }
}

fn parse_stmt_fragment(tokens: &[Token]) -> bool {
    if StmtParser::new(tokens).parse_program().is_ok() {
        return true;
    }
    let mut with_semicolon = tokens.to_vec();
    with_semicolon.push(Token::Semicolon);
    StmtParser::new(&with_semicolon).parse_program().is_ok()
}

fn parse_pat_fragment(tokens: &[Token]) -> bool {
    let mut wrapped = Vec::with_capacity(tokens.len() + 6);
    wrapped.extend([Token::Match, Token::Int(0), Token::LBrace]);
    wrapped.extend(tokens.iter().cloned());
    wrapped.extend([Token::Arrow, Token::Int(0), Token::RBrace]);
    ExprParser::new(&wrapped).parse().is_ok()
}

fn parse_ty_fragment(tokens: &[Token]) -> bool {
    let mut wrapped = Vec::with_capacity(tokens.len() + 6);
    wrapped.extend([Token::Let, Token::Id("__lk_macro_type_probe".to_string()), Token::Colon]);
    wrapped.extend(tokens.iter().cloned());
    wrapped.extend([Token::Assign, Token::Nil, Token::Semicolon]);
    StmtParser::new(&wrapped).parse_program().is_ok()
}

fn parse_path_fragment(tokens: &[Token]) -> bool {
    let Some(Token::Id(_)) = tokens.first() else {
        return false;
    };
    let mut expect_segment = false;
    for token in &tokens[1..] {
        if expect_segment {
            if !matches!(token, Token::Id(_)) {
                return false;
            }
            expect_segment = false;
            continue;
        }
        if matches!(token, Token::ColonColon | Token::Dot) {
            expect_segment = true;
        } else {
            return false;
        }
    }
    !expect_segment
}

fn capture_until(input: &[SourceToken], start: usize, next_literal: Option<&Token>) -> usize {
    let mut index = start;
    let mut paren = 0i32;
    let mut brace = 0i32;
    let mut bracket = 0i32;
    while index < input.len() {
        let token = &input[index].token;
        if paren == 0 && brace == 0 && bracket == 0 {
            if next_literal.is_some_and(|expected| token_matches(expected, token)) {
                break;
            }
            if next_literal.is_none()
                && matches!(
                    token,
                    Token::Comma | Token::Semicolon | Token::RParen | Token::RBrace | Token::RBracket
                )
            {
                break;
            }
        }
        match token {
            Token::LParen => paren += 1,
            Token::RParen => {
                if paren == 0 {
                    break;
                }
                paren -= 1;
            }
            Token::LBrace => brace += 1,
            Token::RBrace => {
                if brace == 0 {
                    break;
                }
                brace -= 1;
            }
            Token::LBracket => bracket += 1,
            Token::RBracket => {
                if bracket == 0 {
                    break;
                }
                bracket -= 1;
            }
            _ => {}
        }
        index += 1;
    }
    index
}

fn next_literal_token(pattern: &[PatternElem]) -> Option<Token> {
    pattern.iter().find_map(|elem| match elem {
        PatternElem::Token(token) => Some(token.clone()),
        _ => None,
    })
}

fn merge_repeated_captures(captures: &mut HashMap<String, Capture>, repeats: Vec<HashMap<String, Capture>>) {
    let mut grouped: HashMap<String, Vec<Vec<SourceToken>>> = HashMap::new();
    for repeat in repeats {
        for (name, capture) in repeat {
            if let Some(tokens) = capture.alternatives.into_iter().next() {
                grouped.entry(name).or_default().push(tokens);
            }
        }
    }
    for (name, alternatives) in grouped {
        captures.insert(name, Capture::repeated(alternatives));
    }
}

fn substitute_template(
    template: &[TemplateElem],
    captures: &HashMap<String, Capture>,
    crate_anchor: Option<&str>,
    call_span: &Span,
) -> Result<Vec<ExpandedToken>, ParseError> {
    substitute_template_at(template, captures, crate_anchor, None, call_span)
}

fn substitute_template_at(
    template: &[TemplateElem],
    captures: &HashMap<String, Capture>,
    crate_anchor: Option<&str>,
    repeat_index: Option<usize>,
    call_span: &Span,
) -> Result<Vec<ExpandedToken>, ParseError> {
    let mut output = Vec::new();
    for elem in template {
        match elem {
            TemplateElem::Token(token) => output.push(ExpandedToken {
                token: token.clone(),
                from_capture: false,
            }),
            TemplateElem::MetaVar(name) => {
                let capture = captures.get(name).ok_or_else(|| {
                    ParseError::with_span(format!("Unknown macro metavariable `${name}`"), call_span.clone())
                })?;
                let replacement = if let Some(index) = repeat_index {
                    capture.alternatives.get(index).ok_or_else(|| {
                        ParseError::with_span(
                            format!("Macro repetition index out of range for `${name}`"),
                            call_span.clone(),
                        )
                    })?
                } else if capture.alternatives.len() == 1 {
                    &capture.alternatives[0]
                } else {
                    return Err(ParseError::with_span(
                        format!("Repeated metavariable `${name}` used outside repetition"),
                        call_span.clone(),
                    ));
                };
                output.extend(replacement.iter().cloned().map(|token| ExpandedToken {
                    token,
                    from_capture: true,
                }));
            }
            TemplateElem::CrateAnchor(token) => {
                let Some(crate_anchor) = crate_anchor else {
                    return Err(ParseError::with_span(
                        "`$crate` is only available inside a registered macro definition".to_string(),
                        call_span.clone(),
                    ));
                };
                let mut token = token.clone();
                token.token = Token::Id(crate_anchor.to_string());
                token.lexeme = crate_anchor.to_string();
                output.push(ExpandedToken {
                    token,
                    from_capture: false,
                });
            }
            TemplateElem::Repeat { elems, separator, op } => {
                let Some(count) = repetition_count(elems, captures, call_span)? else {
                    return Err(ParseError::with_span(
                        "Macro repetition requires at least one metavariable".to_string(),
                        call_span.clone(),
                    ));
                };
                if count == 0 && *op == RepeatOp::OneOrMore {
                    return Err(ParseError::with_span(
                        "Macro repetition expected at least one match".to_string(),
                        call_span.clone(),
                    ));
                }
                if count > 1 && *op == RepeatOp::Optional {
                    return Err(ParseError::with_span(
                        "Optional macro repetition matched more than one item".to_string(),
                        call_span.clone(),
                    ));
                }
                let limit = if *op == RepeatOp::Optional { count.min(1) } else { count };
                for index in 0..limit {
                    if index > 0
                        && let Some(separator) = separator
                    {
                        output.push(ExpandedToken {
                            token: separator.clone(),
                            from_capture: false,
                        });
                    }
                    output.extend(substitute_template_at(
                        elems,
                        captures,
                        crate_anchor,
                        Some(index),
                        call_span,
                    )?);
                }
            }
        }
    }
    Ok(output)
}

fn repetition_count(
    elems: &[TemplateElem],
    captures: &HashMap<String, Capture>,
    call_span: &Span,
) -> Result<Option<usize>, ParseError> {
    let mut count = None;
    collect_repetition_count(elems, captures, call_span, &mut count)?;
    Ok(count)
}

fn collect_repetition_count(
    elems: &[TemplateElem],
    captures: &HashMap<String, Capture>,
    call_span: &Span,
    count: &mut Option<usize>,
) -> Result<(), ParseError> {
    for elem in elems {
        match elem {
            TemplateElem::MetaVar(name) => {
                let Some(capture) = captures.get(name) else {
                    continue;
                };
                let candidate = capture.alternatives.len();
                if let Some(existing) = count {
                    if *existing != candidate {
                        return Err(ParseError::with_span(
                            format!(
                                "Macro repetition metavariable `${name}` matched {candidate} item(s), expected {existing}"
                            ),
                            call_span.clone(),
                        ));
                    }
                } else {
                    *count = Some(candidate);
                }
            }
            TemplateElem::Repeat { elems, .. } => {
                collect_repetition_count(elems, captures, call_span, count)?;
            }
            TemplateElem::Token(_) | TemplateElem::CrateAnchor(_) => {}
        }
    }
    Ok(())
}

fn apply_simple_hygiene(tokens: Vec<ExpandedToken>, site: usize) -> Vec<ExpandedToken> {
    let mut renames = HashMap::new();
    let mut index = 0usize;
    while index + 1 < tokens.len() {
        if !tokens[index].from_capture
            && matches!(tokens[index].token.token, Token::Let | Token::Const)
            && !tokens[index + 1].from_capture
            && let Token::Id(name) = &tokens[index + 1].token.token
        {
            renames
                .entry(name.clone())
                .or_insert_with(|| format!("__lk_macro_{site}_{name}"));
        }
        index += 1;
    }

    tokens
        .into_iter()
        .map(|mut token| {
            if !token.from_capture
                && let Token::Id(name) = &token.token.token
                && let Some(replacement) = renames.get(name)
            {
                token.token.token = Token::Id(replacement.clone());
                token.token.lexeme = replacement.clone();
            }
            token
        })
        .collect()
}
