use std::collections::HashMap;

use crate::token::ParseError;

use super::{PatternElem, SourceToken, TemplateElem, error_at};

#[derive(Debug, Clone, PartialEq, Eq)]
struct BindingShape {
    repeat_depth: usize,
    span_index: usize,
}

pub(super) fn validate_rule_repetition_shapes(
    matcher: &[PatternElem],
    template: &[TemplateElem],
    tokens: &[SourceToken],
) -> Result<(), ParseError> {
    let mut bindings = HashMap::new();
    collect_pattern_bindings(matcher, 0, &mut bindings, tokens)?;
    validate_template_shapes(template, 0, &bindings, tokens)?;
    Ok(())
}

fn collect_pattern_bindings(
    pattern: &[PatternElem],
    repeat_depth: usize,
    bindings: &mut HashMap<String, BindingShape>,
    tokens: &[SourceToken],
) -> Result<(), ParseError> {
    for elem in pattern {
        match elem {
            PatternElem::MetaVar { name, span_index, .. } => {
                if let Some(previous) = bindings.get(name) {
                    return Err(error_at(
                        tokens,
                        *span_index,
                        &format!(
                            "Macro metavariable `${name}` is already bound in this matcher at repetition depth {}",
                            previous.repeat_depth
                        ),
                    ));
                }
                bindings.insert(
                    name.clone(),
                    BindingShape {
                        repeat_depth,
                        span_index: *span_index,
                    },
                );
            }
            PatternElem::Repeat { elems, .. } => {
                collect_pattern_bindings(elems, repeat_depth + 1, bindings, tokens)?;
            }
            PatternElem::Token(_) => {}
        }
    }
    Ok(())
}

fn validate_template_shapes(
    template: &[TemplateElem],
    repeat_depth: usize,
    bindings: &HashMap<String, BindingShape>,
    tokens: &[SourceToken],
) -> Result<(), ParseError> {
    for elem in template {
        match elem {
            TemplateElem::MetaVar(name) => {
                let Some(binding) = bindings.get(name) else {
                    continue;
                };
                if binding.repeat_depth != repeat_depth {
                    return Err(error_at(
                        tokens,
                        binding.span_index,
                        &format!(
                            "Macro metavariable `${name}` appears at repetition depth {repeat_depth} in the template but was bound at depth {} in the matcher",
                            binding.repeat_depth
                        ),
                    ));
                }
            }
            TemplateElem::Repeat { elems, .. } => {
                validate_template_shapes(elems, repeat_depth + 1, bindings, tokens)?;
            }
            TemplateElem::Token(_) | TemplateElem::CrateAnchor(_) => {}
        }
    }
    Ok(())
}
