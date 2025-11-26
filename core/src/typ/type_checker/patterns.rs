use super::TypeChecker;
use crate::expr::Pattern;
use crate::val::Type;
use anyhow::Result;

impl TypeChecker {
    /// Public helper: add variable types introduced by a pattern given the value type
    pub fn add_bindings_for_pattern(&mut self, pattern: &Pattern, value_type: &Type) -> Result<()> {
        let bindings = self.collect_bindings_for_pattern(pattern, value_type)?;
        for (name, ty) in bindings {
            self.add_local_type(name, ty);
        }
        // Validate any guards embedded in the pattern
        self.check_pattern_guards(pattern, value_type)?;
        Ok(())
    }

    /// Ensure a pattern is compatible with a given value type, adding constraints where possible
    pub(super) fn check_pattern_against_type(&mut self, pattern: &Pattern, value_type: &Type) -> Result<()> {
        match pattern {
            Pattern::Literal(v) => {
                // Unify with literal type
                let lit_ty = self.infer_val_type(v)?;
                self.inference_engine.add_constraint(value_type.clone(), lit_ty);
                Ok(())
            }
            Pattern::Variable(_name) => {
                // Variables accept any value
                Ok(())
            }
            Pattern::Wildcard => Ok(()),
            Pattern::List { patterns, rest: _ } => {
                // Expect a list; if element type is unknown, introduce a fresh var
                let elem_ty = match value_type {
                    Type::List(inner) => (**inner).clone(),
                    Type::String => Type::String,
                    other => {
                        // Constrain to List<T>
                        let t = self.inference_engine.fresh_type_var();
                        self.inference_engine
                            .add_constraint(other.clone(), Type::List(Box::new(t.clone())));
                        t
                    }
                };
                for p in patterns {
                    self.check_pattern_against_type(p, &elem_ty)?;
                }
                Ok(())
            }
            Pattern::Map { patterns, rest: _ } => {
                // Expect a map; keys are strings, values have a (possibly inferred) type
                let val_ty = match value_type {
                    Type::Map(_, v) => (**v).clone(),
                    other => {
                        let t = self.inference_engine.fresh_type_var();
                        self.inference_engine
                            .add_constraint(other.clone(), Type::Map(Box::new(Type::String), Box::new(t.clone())));
                        t
                    }
                };
                for (_k, p) in patterns {
                    self.check_pattern_against_type(p, &val_ty)?;
                }
                Ok(())
            }
            Pattern::Or(_alts) => {
                // Do not add constraints for OR patterns; any branch may match at runtime
                // Guards are validated separately by check_pattern_guards.
                Ok(())
            }
            Pattern::Guard { pattern, guard } => {
                // First check inner pattern
                self.check_pattern_against_type(pattern, value_type)?;
                // Then validate the guard with temporary bindings from inner pattern
                let temp_bindings = self.collect_bindings_for_pattern(pattern, value_type)?;
                let snapshot = self.local_types.clone();
                for (n, ty) in temp_bindings {
                    self.add_local_type(n, ty);
                }
                let gty = self.check_expr(guard)?;
                self.local_types = snapshot;
                if gty != Type::Bool {
                    return Err(Self::type_err(
                        "Match guard must be Bool",
                        Some(Type::Bool),
                        Some(gty),
                        Some(*guard.clone()),
                    ));
                }
                Ok(())
            }
            Pattern::Range {
                start,
                end,
                inclusive: _,
            } => {
                let st = self.check_expr(start)?;
                let et = self.check_expr(end)?;
                // Start/end must be same numeric type
                self.inference_engine.add_constraint(st.clone(), et.clone());
                match (&st, &et) {
                    (Type::Int, Type::Int) | (Type::Float, Type::Float) => {
                        self.inference_engine.add_constraint(value_type.clone(), st);
                        Ok(())
                    }
                    _ => Err(Self::type_err(
                        "Range pattern bounds must be numeric and of same type",
                        None,
                        None,
                        None,
                    )),
                }
            }
        }
    }

    /// Collect variable bindings and their types from a pattern
    fn collect_bindings_for_pattern(&mut self, pattern: &Pattern, value_type: &Type) -> Result<Vec<(String, Type)>> {
        let mut out = Vec::new();
        match pattern {
            Pattern::Variable(name) => {
                out.push((name.clone(), value_type.clone()));
            }
            Pattern::Wildcard | Pattern::Literal(_) | Pattern::Range { .. } => {}
            Pattern::List { patterns, rest } => {
                let (elem_ty, rest_ty) = match value_type {
                    Type::List(inner) => ((**inner).clone(), Type::List(inner.clone())),
                    Type::String => (Type::String, Type::List(Box::new(Type::String))),
                    other => {
                        let t = self.inference_engine.fresh_type_var();
                        self.inference_engine
                            .add_constraint(other.clone(), Type::List(Box::new(t.clone())));
                        let rest_t = Type::List(Box::new(t.clone()));
                        (t, rest_t)
                    }
                };
                for p in patterns {
                    out.extend(self.collect_bindings_for_pattern(p, &elem_ty)?);
                }
                if let Some(rest_name) = rest {
                    out.push((rest_name.clone(), rest_ty));
                }
            }
            Pattern::Map { patterns, rest } => {
                let vty = match value_type {
                    Type::Map(_, v) => (**v).clone(),
                    other => {
                        let t = self.inference_engine.fresh_type_var();
                        self.inference_engine
                            .add_constraint(other.clone(), Type::Map(Box::new(Type::String), Box::new(t.clone())));
                        t
                    }
                };
                for (_k, p) in patterns {
                    out.extend(self.collect_bindings_for_pattern(p, &vty)?);
                }
                if let Some(rest_name) = rest {
                    out.push((
                        rest_name.clone(),
                        Type::Map(Box::new(Type::String), Box::new(vty.clone())),
                    ));
                }
            }
            Pattern::Or(alts) => {
                // For OR patterns, only bind variables that appear in all alternatives.
                // Their type becomes the union of the types from each alternative.
                use std::collections::{HashMap, HashSet};
                let mut alt_maps: Vec<HashMap<String, Type>> = Vec::with_capacity(alts.len());
                for alt in alts {
                    let entries = self.collect_bindings_for_pattern(alt, value_type)?;
                    let mut map = HashMap::with_capacity(entries.len());
                    for (n, t) in entries {
                        map.insert(n, t);
                    }
                    alt_maps.push(map);
                }

                if alt_maps.is_empty() {
                    return Ok(out);
                }

                let mut common: HashSet<String> = alt_maps[0].keys().cloned().collect();
                for m in &alt_maps[1..] {
                    common.retain(|k| m.contains_key(k));
                }

                for name in common {
                    let mut types = Vec::new();
                    for m in &alt_maps {
                        if let Some(t) = m.get(&name) {
                            types.push(t.clone());
                        }
                    }
                    let ty = if types.len() == 1 {
                        types.remove(0)
                    } else {
                        Type::Union(types)
                    };
                    out.push((name, ty));
                }
            }
            Pattern::Guard { pattern, .. } => {
                out.extend(self.collect_bindings_for_pattern(pattern, value_type)?);
            }
        }
        Ok(out)
    }

    /// Validate any guards embedded inside a pattern (Bool requirement)
    fn check_pattern_guards(&mut self, pattern: &Pattern, value_type: &Type) -> Result<()> {
        match pattern {
            Pattern::Guard { pattern, guard } => {
                // Bind variables from inner pattern temporarily, then type-check guard
                let bindings = self.collect_bindings_for_pattern(pattern, value_type)?;
                let snapshot = self.local_types.clone();
                for (n, ty) in bindings {
                    self.add_local_type(n, ty);
                }
                let gty = self.check_expr(guard)?;
                self.local_types = snapshot;
                if gty != Type::Bool {
                    return Err(Self::type_err(
                        "Match guard must be Bool",
                        Some(Type::Bool),
                        Some(gty),
                        Some(*guard.clone()),
                    ));
                }
                Ok(())
            }
            Pattern::List { patterns, .. } => {
                for p in patterns {
                    self.check_pattern_guards(p, value_type)?;
                }
                Ok(())
            }
            Pattern::Map { patterns, .. } => {
                for (_k, p) in patterns {
                    self.check_pattern_guards(p, value_type)?;
                }
                Ok(())
            }
            Pattern::Or(alts) => {
                for p in alts {
                    self.check_pattern_guards(p, value_type)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}
