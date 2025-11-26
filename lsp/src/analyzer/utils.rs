use lkr_core::expr;

/// Helper function to extract variable names from a pattern for LSP analysis
pub fn extract_variables_from_pattern(pattern: &expr::Pattern) -> Option<Vec<String>> {
    let mut variables = Vec::new();

    fn collect_vars(pattern: &expr::Pattern, vars: &mut Vec<String>) {
        match pattern {
            expr::Pattern::Variable(name) => {
                vars.push(name.clone());
            }
            expr::Pattern::List { patterns, rest } => {
                for pattern in patterns {
                    collect_vars(pattern, vars);
                }
                if let Some(rest_var) = rest {
                    vars.push(rest_var.clone());
                }
            }
            expr::Pattern::Map { patterns, rest } => {
                for (_, pattern) in patterns {
                    collect_vars(pattern, vars);
                }
                if let Some(rest_var) = rest {
                    vars.push(rest_var.clone());
                }
            }
            expr::Pattern::Or(patterns) => {
                for pattern in patterns {
                    collect_vars(pattern, vars);
                }
            }
            expr::Pattern::Guard { pattern, .. } => {
                collect_vars(pattern, vars);
            }
            // Other pattern types don't bind variables
            expr::Pattern::Literal(_) | expr::Pattern::Wildcard | expr::Pattern::Range { .. } => {}
        }
    }

    collect_vars(pattern, &mut variables);

    // Remove duplicates (can happen with OR patterns)
    variables.sort();
    variables.dedup();

    if variables.is_empty() {
        None
    } else {
        Some(variables)
    }
}
