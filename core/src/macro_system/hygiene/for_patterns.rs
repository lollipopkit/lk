use crate::macro_system::ExpandedToken;
use crate::stmt::{ForPattern, StmtParser};

use super::{BindingRename, collect_simple_generated_id, find_generated_ids_in_range};

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
                excluded_start: None,
                excluded_end: None,
                end: scope_end,
            });
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

/// Parses a `for` pattern prefix from generated macro tokens.
///
/// Preconditions: `start <= end` and `end <= tokens.len()`.
/// Returns the parsed pattern and number of tokens consumed, or `None` if the
/// tokens at `start..end` don't form a valid for-pattern prefix.
pub(super) fn parse_generated_for_pattern_prefix(
    tokens: &[ExpandedToken],
    start: usize,
    end: usize,
) -> Option<(ForPattern, usize)> {
    debug_assert!(
        start <= end,
        "parse_generated_for_pattern_prefix: start ({start}) > end ({end})"
    );
    debug_assert!(
        end <= tokens.len(),
        "parse_generated_for_pattern_prefix: end ({end}) > tokens.len() ({})",
        tokens.len()
    );
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
        ForPattern::Variable(name) if name != "_" => push_for_pattern_name(names, name),
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
                push_for_pattern_name(names, rest);
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

fn push_for_pattern_name(names: &mut Vec<String>, name: &str) {
    if !names.iter().any(|existing| existing == name) {
        names.push(name.to_string());
    }
}
