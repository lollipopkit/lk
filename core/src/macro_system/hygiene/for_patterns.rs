use crate::macro_system::ExpandedToken;
use crate::stmt::{ForPattern, StmtParser};

use super::{BindingRename, collect_simple_generated_id, find_generated_id_in_range};

pub(super) fn collect_for_pattern_binding_renames(
    tokens: &[ExpandedToken],
    start: usize,
    consumed: usize,
    pattern: &ForPattern,
    site: usize,
    reference_start: usize,
    scope_end: usize,
    renames: &mut Vec<BindingRename>,
) {
    let mut names = Vec::new();
    collect_for_pattern_names(pattern, &mut names);
    for name in names {
        if let Some(index) = find_generated_id_in_range(tokens, start, start + consumed, &name) {
            collect_simple_generated_id(tokens, index, site, reference_start, None, None, scope_end, renames);
        }
    }
}

pub(super) fn collect_fallback_for_pattern_ids(
    tokens: &[ExpandedToken],
    start: usize,
    end: usize,
    site: usize,
    reference_start: usize,
    scope_end: usize,
    renames: &mut Vec<BindingRename>,
) {
    for id_index in start..end {
        collect_simple_generated_id(tokens, id_index, site, reference_start, None, None, scope_end, renames);
    }
}

pub(super) fn parse_generated_for_pattern_prefix(
    tokens: &[ExpandedToken],
    start: usize,
    end: usize,
) -> Option<(ForPattern, usize)> {
    if start >= end || tokens[start].from_capture {
        return None;
    }
    let plain_tokens = tokens[start..end]
        .iter()
        .map(|token| token.token.token.clone())
        .collect::<Vec<_>>();
    let mut parser = StmtParser::new(&plain_tokens);
    let pattern = parser.parse_for_pattern().ok()?;
    Some((pattern, parser.pos))
}

fn collect_for_pattern_names(pattern: &ForPattern, names: &mut Vec<String>) {
    match pattern {
        ForPattern::Variable(name) if name != "_" => names.push(name.clone()),
        ForPattern::Tuple(patterns) => {
            for pattern in patterns {
                collect_for_pattern_names(pattern, names);
            }
        }
        ForPattern::Array { patterns, rest } => {
            for pattern in patterns {
                collect_for_pattern_names(pattern, names);
            }
            if let Some(rest) = rest
                && rest != "_"
            {
                names.push(rest.clone());
            }
        }
        ForPattern::Object(entries) => {
            for (_, pattern) in entries {
                collect_for_pattern_names(pattern, names);
            }
        }
        ForPattern::Ignore | ForPattern::Variable(_) => {}
    }
}
