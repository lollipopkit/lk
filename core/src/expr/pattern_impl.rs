use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::{Result, anyhow};

use super::expr_impl::Pattern;
use crate::val::Val;
use crate::vm::VmContext;

impl std::fmt::Display for Pattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Pattern::Literal(val) => write!(f, "{}", val),
            Pattern::Variable(name) => write!(f, "{}", name),
            Pattern::Wildcard => write!(f, "_"),
            Pattern::List { patterns, rest } => {
                write!(f, "[")?;
                for (i, pattern) in patterns.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", pattern)?;
                }
                if let Some(rest_name) = rest {
                    if !patterns.is_empty() {
                        write!(f, ", ")?;
                    }
                    write!(f, "..{}", rest_name)?;
                }
                write!(f, "]")
            }
            Pattern::Map { patterns, rest } => {
                write!(f, "{{")?;
                for (i, (key, pattern)) in patterns.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "\"{}\": {}", key, pattern)?;
                }
                if let Some(rest_name) = rest {
                    if !patterns.is_empty() {
                        write!(f, ", ")?;
                    }
                    write!(f, "..{}", rest_name)?;
                }
                write!(f, "}}")
            }
            Pattern::Or(patterns) => {
                for (i, pattern) in patterns.iter().enumerate() {
                    if i > 0 {
                        write!(f, " | ")?;
                    }
                    write!(f, "{}", pattern)?;
                }
                Ok(())
            }
            Pattern::Guard { pattern, guard } => {
                write!(f, "{} if {}", pattern, guard)
            }
            Pattern::Range { start, end, inclusive } => {
                let op = if *inclusive { "..=" } else { ".." };
                write!(f, "{}{}{}", start, op, end)
            }
        }
    }
}

impl Pattern {
    /// Check if this pattern matches a value, returning bindings if it matches.
    pub fn matches(&self, value: &Val, ctx: Option<&VmContext>) -> Result<Option<Vec<(String, Val)>>> {
        let mut bindings = Vec::new();
        if self.matches_impl(value, &mut bindings, ctx)? {
            Ok(Some(bindings))
        } else {
            Ok(None)
        }
    }

    fn matches_impl(&self, value: &Val, bindings: &mut Vec<(String, Val)>, ctx: Option<&VmContext>) -> Result<bool> {
        match self {
            Pattern::Literal(pattern_val) => Ok(value == pattern_val),
            Pattern::Variable(name) => {
                bindings.push((name.clone(), value.clone()));
                Ok(true)
            }
            Pattern::Wildcard => Ok(true),
            Pattern::List { patterns, rest } => {
                let list_items: Vec<Val> = match value {
                    value if value.as_list().is_some() => value.as_list().expect("checked list").as_ref().clone(),
                    _ if value.as_str().is_some() => value
                        .as_str()
                        .unwrap()
                        .chars()
                        .map(|character| Val::from_str(&character.to_string()))
                        .collect(),
                    _ => return Ok(false),
                };
                if patterns.len() > list_items.len() && rest.is_none() {
                    return Ok(false);
                }
                for (index, pattern) in patterns.iter().enumerate() {
                    if index >= list_items.len() || !pattern.matches_impl(&list_items[index], bindings, ctx)? {
                        return Ok(false);
                    }
                }
                if let Some(rest_name) = rest {
                    let rest_items: Vec<Val> = list_items.iter().skip(patterns.len()).cloned().collect();
                    bindings.push((rest_name.clone(), Val::list(Arc::from(rest_items))));
                } else if patterns.len() != list_items.len() {
                    return Ok(false);
                }
                Ok(true)
            }
            Pattern::Map { patterns, rest } => {
                let Some(map) = value.as_map() else {
                    return Ok(false);
                };
                let map_ref = map.as_ref();
                for (key, pattern) in patterns {
                    let Some(field_val) = map_ref.get(key.as_str()) else {
                        return Ok(false);
                    };
                    if !pattern.matches_impl(field_val, bindings, ctx)? {
                        return Ok(false);
                    }
                }
                if let Some(rest_name) = rest {
                    let matched_keys: HashSet<&str> = patterns.iter().map(|(key, _)| key.as_str()).collect();
                    let rest_map: HashMap<String, Val> = map_ref
                        .iter()
                        .filter(|(key, _)| !matched_keys.contains(key.as_str()))
                        .map(|(key, value)| (key.to_string(), value.clone()))
                        .collect();
                    bindings.push((rest_name.clone(), rest_map.into()));
                }
                Ok(true)
            }
            Pattern::Or(patterns) => {
                for pattern in patterns {
                    let mut temp_bindings = Vec::new();
                    if pattern.matches_impl(value, &mut temp_bindings, ctx)? {
                        bindings.extend(temp_bindings);
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Pattern::Guard { pattern, guard } => {
                let mut temp_bindings = Vec::new();
                if !pattern.matches_impl(value, &mut temp_bindings, ctx)? {
                    return Ok(false);
                }
                if let Some(ctx_ref) = ctx {
                    let mut temp_ctx = ctx_ref.clone();
                    temp_ctx.push_scope();
                    for (name, value) in &temp_bindings {
                        temp_ctx.set(name.clone(), value.clone());
                    }
                    let guard_result = guard.eval_with_ctx(&mut temp_ctx)?;
                    temp_ctx.pop_scope();
                    if let Val::Bool(true) = guard_result {
                        bindings.extend(temp_bindings);
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                } else if !temp_bindings.is_empty() {
                    Err(anyhow!("Guard conditions with bindings require evaluation context"))
                } else {
                    Err(anyhow!("Guard evaluation requires context"))
                }
            }
            Pattern::Range { start, end, inclusive } => {
                if let Some(mut ctx_ref) = ctx.cloned() {
                    let start_val = start.eval_with_ctx(&mut ctx_ref)?;
                    let end_val = end.eval_with_ctx(&mut ctx_ref)?;
                    match (value, &start_val, &end_val) {
                        (Val::Int(value), Val::Int(start), Val::Int(end)) => Ok(if *inclusive {
                            *value >= *start && *value <= *end
                        } else {
                            *value >= *start && *value < *end
                        }),
                        (Val::Float(value), Val::Float(start), Val::Float(end)) => Ok(if *inclusive {
                            *value >= *start && *value <= *end
                        } else {
                            *value >= *start && *value < *end
                        }),
                        _ => Ok(false),
                    }
                } else {
                    Err(anyhow!("Range pattern evaluation requires context"))
                }
            }
        }
    }
}
