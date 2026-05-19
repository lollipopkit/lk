use crate::expr::Pattern;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArithFlavor {
    Int,
    Float,
    Any,
}

pub(super) fn collect_pattern_names(pattern: &Pattern, out: &mut Vec<String>) {
    match pattern {
        Pattern::Variable(name) => out.push(name.clone()),
        Pattern::List { patterns, rest } => {
            for sub in patterns {
                collect_pattern_names(sub, out);
            }
            if let Some(rest_name) = rest {
                out.push(rest_name.clone());
            }
        }
        Pattern::Map { patterns, rest } => {
            for (_, sub) in patterns {
                collect_pattern_names(sub, out);
            }
            if let Some(rest_name) = rest {
                out.push(rest_name.clone());
            }
        }
        Pattern::Or(alternatives) => {
            for alt in alternatives {
                collect_pattern_names(alt, out);
            }
        }
        Pattern::Guard { pattern, .. } => {
            collect_pattern_names(pattern, out);
        }
        Pattern::Literal(_) | Pattern::Wildcard | Pattern::Range { .. } => {}
    }
}
