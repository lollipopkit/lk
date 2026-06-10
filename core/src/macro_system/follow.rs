use crate::token::{ParseError, Token};

use super::{FragmentKind, PatternElem, RepeatOp, SourceToken, error_at, token_lexeme, token_matches};

#[derive(Debug, Clone)]
enum FollowAtom {
    End,
    Token(Token),
    Fragment(FragmentKind),
}

pub(super) fn validate_pattern_follow_sets(pattern: &[PatternElem], tokens: &[SourceToken]) -> Result<(), ParseError> {
    for (index, elem) in pattern.iter().enumerate() {
        if let PatternElem::Repeat {
            elems,
            separator,
            op,
            span_index,
        } = elem
        {
            validate_pattern_follow_sets(elems, tokens)?;
            if let Some(separator) = separator {
                validate_last_fragment_follow(elems, &FollowAtom::Token(separator.clone()), tokens, *span_index)?;
            } else if matches!(op, RepeatOp::ZeroOrMore | RepeatOp::OneOrMore) {
                for follower in first_follow_atoms(elems, 0) {
                    validate_last_fragment_follow(elems, &follower, tokens, *span_index)?;
                }
            }
        }

        for follower in first_follow_atoms(pattern, index + 1) {
            validate_elem_follow(elem, &follower, tokens)?;
        }
    }
    Ok(())
}

fn validate_last_fragment_follow(
    elems: &[PatternElem],
    follower: &FollowAtom,
    tokens: &[SourceToken],
    fallback_index: usize,
) -> Result<(), ParseError> {
    for (name, kind, span_index) in last_restricted_metavars(elems) {
        validate_fragment_follow(&name, kind, span_index, follower, tokens)?;
    }
    if elems.is_empty() {
        return Err(error_at(tokens, fallback_index, "Macro repetition must not be empty"));
    }
    Ok(())
}

fn validate_elem_follow(elem: &PatternElem, follower: &FollowAtom, tokens: &[SourceToken]) -> Result<(), ParseError> {
    match elem {
        PatternElem::MetaVar { name, kind, span_index } => {
            validate_fragment_follow(name, *kind, *span_index, follower, tokens)
        }
        PatternElem::Repeat { elems, span_index, .. } => {
            validate_last_fragment_follow(elems, follower, tokens, *span_index)
        }
        PatternElem::Token(_) => Ok(()),
    }
}

fn validate_fragment_follow(
    name: &str,
    kind: FragmentKind,
    span_index: usize,
    follower: &FollowAtom,
    tokens: &[SourceToken],
) -> Result<(), ParseError> {
    if fragment_follow_allowed(kind, follower) {
        return Ok(());
    }
    Err(error_at(
        tokens,
        span_index,
        &format!(
            "Macro fragment `${}:{}` cannot be followed by `{}`; expected one of {}",
            name,
            fragment_kind_name(kind),
            follow_atom_label(follower),
            allowed_follow_summary(kind)
        ),
    ))
}

fn first_follow_atoms(pattern: &[PatternElem], start: usize) -> Vec<FollowAtom> {
    if start >= pattern.len() {
        return vec![FollowAtom::End];
    }
    match &pattern[start] {
        PatternElem::Token(token) => vec![FollowAtom::Token(token.clone())],
        PatternElem::MetaVar { kind, .. } => vec![FollowAtom::Fragment(*kind)],
        PatternElem::Repeat { elems, op, .. } => {
            let mut atoms = first_follow_atoms(elems, 0);
            if *op != RepeatOp::OneOrMore {
                atoms.extend(first_follow_atoms(pattern, start + 1));
            }
            dedup_follow_atoms(atoms)
        }
    }
}

fn last_restricted_metavars(pattern: &[PatternElem]) -> Vec<(String, FragmentKind, usize)> {
    let mut out = Vec::new();
    for elem in pattern.iter().rev() {
        match elem {
            PatternElem::MetaVar { name, kind, span_index } => {
                if fragment_has_follow_restrictions(*kind) {
                    out.push((name.clone(), *kind, *span_index));
                }
                break;
            }
            PatternElem::Repeat { elems, op, .. } => {
                out.extend(last_restricted_metavars(elems));
                if *op == RepeatOp::OneOrMore {
                    break;
                }
            }
            PatternElem::Token(_) => break,
        }
    }
    out
}

fn dedup_follow_atoms(atoms: Vec<FollowAtom>) -> Vec<FollowAtom> {
    let mut out = Vec::new();
    for atom in atoms {
        if !out.iter().any(|existing| follow_atoms_match(existing, &atom)) {
            out.push(atom);
        }
    }
    out
}

fn follow_atoms_match(left: &FollowAtom, right: &FollowAtom) -> bool {
    match (left, right) {
        (FollowAtom::End, FollowAtom::End) => true,
        (FollowAtom::Token(left), FollowAtom::Token(right)) => token_matches(left, right),
        (FollowAtom::Fragment(left), FollowAtom::Fragment(right)) => left == right,
        _ => false,
    }
}

fn fragment_has_follow_restrictions(kind: FragmentKind) -> bool {
    matches!(
        kind,
        FragmentKind::Expr | FragmentKind::Stmt | FragmentKind::Pat | FragmentKind::Ty | FragmentKind::Path
    )
}

fn fragment_follow_allowed(kind: FragmentKind, follower: &FollowAtom) -> bool {
    if matches!(follower, FollowAtom::End) || !fragment_has_follow_restrictions(kind) {
        return true;
    }
    if matches!(
        follower,
        FollowAtom::Token(Token::RParen | Token::RBracket | Token::RBrace)
    ) {
        return true;
    }
    match kind {
        FragmentKind::Expr | FragmentKind::Stmt => {
            matches!(
                follower,
                FollowAtom::Token(Token::Arrow | Token::Comma | Token::Semicolon)
                    | FollowAtom::Fragment(FragmentKind::Block)
            )
        }
        FragmentKind::Pat => {
            matches!(
                follower,
                FollowAtom::Token(Token::Arrow | Token::Comma | Token::Assign | Token::If | Token::In)
            )
        }
        FragmentKind::Ty | FragmentKind::Path => {
            matches!(follower, FollowAtom::Token(token) if ty_path_follow_token_allowed(token))
                || matches!(follower, FollowAtom::Fragment(FragmentKind::Block))
        }
        FragmentKind::Block | FragmentKind::Item | FragmentKind::Ident | FragmentKind::Literal | FragmentKind::Tt => {
            true
        }
    }
}

fn ty_path_follow_token_allowed(token: &Token) -> bool {
    matches!(
        token,
        Token::Arrow
            | Token::Comma
            | Token::Assign
            | Token::Pipe
            | Token::Semicolon
            | Token::Colon
            | Token::Gt
            | Token::LBracket
            | Token::LBrace
            | Token::As
    ) || matches!(token, Token::Id(name) if name == "where")
}

fn follow_atom_label(atom: &FollowAtom) -> String {
    match atom {
        FollowAtom::End => "end of matcher".to_string(),
        FollowAtom::Token(token) => token_lexeme(token),
        FollowAtom::Fragment(kind) => format!("${}:{}", "_", fragment_kind_name(*kind)),
    }
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

fn allowed_follow_summary(kind: FragmentKind) -> &'static str {
    match kind {
        FragmentKind::Expr | FragmentKind::Stmt => "`=>`, `,`, `;`, or a `block` fragment",
        FragmentKind::Pat => "`=>`, `,`, `=`, `if`, or `in`",
        FragmentKind::Ty | FragmentKind::Path => {
            "`=>`, `,`, `=`, `|`, `;`, `:`, `>`, `[`, `{`, `as`, `where`, or a `block` fragment"
        }
        FragmentKind::Block | FragmentKind::Item | FragmentKind::Ident | FragmentKind::Literal | FragmentKind::Tt => {
            "any token"
        }
    }
}
