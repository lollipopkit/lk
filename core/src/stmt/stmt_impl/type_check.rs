use super::{ForPattern, Program, Stmt};
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::{
    expr::Pattern,
    token::ParseError,
    typ::{
        FunctionSig, NamedParamSig, PendingStrictFunction, PendingStrictParam, StructDef, TraitDef,
        TypeAlias as AliasDef, TypeChecker,
    },
    val::{FunctionNamedParamType, Type},
};
use anyhow::{Result, anyhow};
use hashbrown::HashMap;

impl Stmt {
    /// 静态类型检查语句
    pub fn type_check(&self, type_checker: &mut TypeChecker) -> Result<()> {
        match self {
            Stmt::Attributed { item, .. } => item.type_check(type_checker),
            Stmt::TypeAlias { name, target } => {
                type_checker.registry_mut().register_type_alias(AliasDef {
                    name: name.clone(),
                    target_type: target.clone(),
                });
                Ok(())
            }
            Stmt::Struct { name, fields } => {
                // Register struct in registry for subsequent checks
                let mut fm = HashMap::new();
                let mut missing: Vec<String> = Vec::new();
                for (k, ty_opt) in fields.iter() {
                    match ty_opt {
                        Some(ty) => {
                            fm.insert(k.clone(), ty.clone());
                        }
                        None => {
                            if type_checker.strict_any() {
                                missing.push(k.clone());
                            }
                            fm.insert(k.clone(), Type::Any);
                        }
                    }
                }
                if type_checker.strict_any() && !missing.is_empty() {
                    return Err(anyhow!(format!(
                        "Struct '{}' has fields without explicit types: {}",
                        name,
                        missing.join(", ")
                    )));
                }
                let sd = StructDef {
                    name: name.clone(),
                    fields: fm,
                };
                type_checker.registry_mut().register_struct(sd);
                Ok(())
            }
            Stmt::Trait { name, methods } => {
                // Register trait with method signatures
                let mut map = HashMap::with_capacity(methods.len());
                for (m, ty) in methods.iter() {
                    map.insert(m.clone(), ty.clone());
                }
                let def = TraitDef {
                    name: name.clone(),
                    methods: map,
                };
                type_checker.registry_mut().register_trait(def);
                Ok(())
            }
            Stmt::Impl {
                trait_name: _,
                target_type,
                methods,
            } => {
                let prev = type_checker.set_impl_self_type(Some(type_checker.resolve_aliases(target_type)));
                let result: Result<()> = methods.iter().try_for_each(|method| method.type_check(type_checker));
                type_checker.set_impl_self_type(prev);
                result
            }
            Stmt::Let {
                pattern,
                type_annotation,
                value,
                span,
                is_const,
            } => {
                // 检查表达式的类型
                let expr_type = value.type_check(type_checker)?;
                // Reached: statements below this one may read it. Done after
                // the value, so `const A = A + 1;` still reports the read.
                for name in pattern_names(pattern) {
                    type_checker.define_top_level(&name);
                }

                // 如果有类型注解，验证类型匹配
                //
                // A machine-int annotation *retypes* an integer literal rather
                // than rejecting it — `let x: u8 = 5` is the common case, and
                // requiring `5 as u8` there would make the feature unusable.
                // This is Rust's literal-inference rule, narrowed to the one
                // place it is needed until the checker becomes bidirectional.
                // The range is checked here because having one is the whole
                // point of a fixed width.
                if let Some(Type::MachineInt(kind)) = type_annotation
                    && let Some(literal) = int_literal_value(value)
                {
                    if !kind.accepts_literal(literal) {
                        let error_msg = alloc::format!(
                            "literal {literal} is out of range for {}",
                            Type::MachineInt(*kind).display()
                        );
                        return if let Some(span) = span {
                            Err(anyhow!(ParseError::with_span(error_msg, span.clone())))
                        } else {
                            Err(anyhow!(error_msg))
                        };
                    }
                } else if let Some(expected_type) = type_annotation
                    && !type_checker.is_assignable(&expr_type, expected_type)
                {
                    let error_msg = format!(
                        "Type mismatch in let statement: pattern expected type {}, but expression has type {}",
                        expected_type.display(),
                        expr_type.display()
                    );
                    return if let Some(span) = span {
                        Err(anyhow!(ParseError::with_span(error_msg, span.clone())))
                    } else {
                        Err(anyhow!(error_msg))
                    };
                }

                // The pattern is distributed over the value's type, so each name
                // gets *its own* element type. Binding the whole right-hand side
                // to every name (what this used to do) types `v` in
                // `let [ok, v] = f()` as the entire tuple.
                let bound_type = type_annotation.clone().unwrap_or(expr_type);
                bind_pattern_types(pattern, &bound_type, *is_const, type_checker);

                if let Some(span) = span {
                    let names = pattern_names(pattern);
                    type_checker.record_bindings(span, type_annotation.is_some(), names.iter().map(String::as_str));
                }

                Ok(())
            }
            Stmt::Assign { name, value, span } => {
                // 检查表达式的类型
                let expr_type = value.type_check(type_checker)?;

                // 获取变量的已声明类型
                if let Some(var_type) = type_checker.get_local_type(name) {
                    if type_checker.is_const_local(name) {
                        let error_msg = format!("Cannot assign to const variable '{}'", name);
                        return if let Some(span) = span {
                            Err(anyhow!(ParseError::with_span(error_msg, span.clone())))
                        } else {
                            Err(anyhow!(error_msg))
                        };
                    }
                    if matches!(var_type, Type::Variable(_)) {
                        // Refine previously unknown binding with the inferred expression type.
                        type_checker.add_local_type(name.clone(), expr_type.clone());
                    } else if expr_type.contains_variables() {
                        // Expression has unresolved type variables; add constraint instead of failing.
                        type_checker.add_constraint(expr_type, var_type.clone());
                    } else if !type_checker.is_assignable(&expr_type, var_type) {
                        let error_msg = format!(
                            "Type mismatch in assignment: variable '{}' has type {}, but right-hand side has type {}",
                            name,
                            var_type.display(),
                            expr_type.display()
                        );
                        return if let Some(span) = span {
                            Err(anyhow!(ParseError::with_span(error_msg, span.clone())))
                        } else {
                            Err(anyhow!(error_msg))
                        };
                    }
                } else {
                    return Err(anyhow!(format!(
                        "Undefined variable '{}': cannot assign without declaration",
                        name
                    )));
                }

                Ok(())
            }
            Stmt::CompoundAssign { name, value, span, .. } => {
                let expr_type = value.type_check(type_checker)?;
                if let Some(var_type) = type_checker.get_local_type(name) {
                    if type_checker.is_const_local(name) {
                        let error_msg = format!("Cannot assign to const variable '{}'", name);
                        return if let Some(span) = span {
                            Err(anyhow!(ParseError::with_span(error_msg, span.clone())))
                        } else {
                            Err(anyhow!(error_msg))
                        };
                    }
                    // 检查操作类型兼容性 (var_type op expr_type -> var_type).
                    // If either side is still inferred, keep the relationship as a constraint
                    // so function-body compound assignments can refine unannotated params.
                    if var_type.contains_variables() {
                        type_checker.add_constraint(var_type.clone(), expr_type.clone());
                    } else if expr_type.contains_variables() {
                        type_checker.add_constraint(expr_type.clone(), var_type.clone());
                    } else if !type_checker.is_assignable(&expr_type, var_type)
                        && !type_checker.is_assignable(var_type, &expr_type)
                    {
                        let error_msg = format!(
                            "Type mismatch in compound assignment: variable '{}' has type {}, but right-hand side has type {}",
                            name,
                            var_type.display(),
                            expr_type.display()
                        );
                        return if let Some(span) = span {
                            Err(anyhow!(ParseError::with_span(error_msg, span.clone())))
                        } else {
                            Err(anyhow!(error_msg))
                        };
                    }
                } else {
                    return Err(anyhow!(format!(
                        "Cannot compound assign to undefined variable '{}'",
                        name
                    )));
                }

                Ok(())
            }
            Stmt::Function {
                name,
                params,
                param_types,
                return_type,
                body,
                named_params,
            } => {
                type_checker.push_scope();
                // A body runs after the whole top level, so it may read a
                // binding declared below it.
                // Suspended for the body, and restored on *every* way out —
                // see the restore below. An early `?` inside the body check
                // would otherwise leave the set empty for the rest of the
                // file, quietly disabling the use-before-definition check for
                // every statement after a function that failed to type-check.
                let pending = type_checker.suspend_pending_top_level();

                let mut positional_tys: Vec<Type> = Vec::with_capacity(params.len());
                let mut positional_origin: Vec<bool> = Vec::with_capacity(params.len());
                let impl_self_ty = type_checker.current_impl_self_type().cloned();
                for (i, param) in params.iter().enumerate() {
                    let annotated = param_types.get(i).cloned().flatten();
                    let mut origin_flag = annotated.is_some();
                    let mut ty = if let Some(ref ann) = annotated {
                        ann.clone()
                    } else {
                        type_checker.fresh_type_var()
                    };

                    if i == 0 && param == "self" {
                        if let Some(target_ty) = impl_self_ty.clone() {
                            if let Some(ref ann) = annotated {
                                if !type_checker.is_assignable(ann, &target_ty)
                                    && !type_checker.is_assignable(&target_ty, ann)
                                {
                                    return Err(anyhow!(format!(
                                        "Method '{}' self parameter type {} incompatible with impl target {}",
                                        name,
                                        ann.display(),
                                        target_ty.display()
                                    )));
                                }
                            } else {
                                ty = target_ty;
                                origin_flag = true;
                            }
                        }
                    }

                    positional_origin.push(origin_flag);
                    type_checker.add_local_type(param.clone(), ty.clone());
                    positional_tys.push(ty);
                }

                let mut named_annos: Vec<FunctionNamedParamType> = Vec::with_capacity(named_params.len());
                let mut named_sigs: Vec<NamedParamSig> = Vec::with_capacity(named_params.len());
                let mut named_origin: Vec<bool> = Vec::with_capacity(named_params.len());
                for np in named_params.iter() {
                    let default_ty = if let Some(def_expr) = &np.default {
                        Some(def_expr.type_check(type_checker)?)
                    } else {
                        None
                    };

                    if let (Some(annotation), Some(def_ty)) = (&np.type_annotation, &default_ty) {
                        if !type_checker.is_assignable(def_ty, annotation) {
                            return Err(anyhow!(format!(
                                "Default value type for named param '{}' not assignable to {} (got {})",
                                np.name,
                                annotation.display(),
                                def_ty.display()
                            )));
                        }
                    }

                    let ty = if let Some(annotation) = &np.type_annotation {
                        annotation.clone()
                    } else if let Some(def_ty) = default_ty.clone() {
                        def_ty
                    } else {
                        type_checker.fresh_type_var()
                    };

                    named_origin.push(np.type_annotation.is_some());
                    type_checker.add_local_type(np.name.clone(), ty.clone());

                    named_annos.push(FunctionNamedParamType {
                        name: np.name.clone(),
                        ty: ty.clone(),
                        has_default: np.default.is_some(),
                    });
                    named_sigs.push(NamedParamSig {
                        name: np.name.clone(),
                        ty,
                        has_default: np.default.is_some(),
                    });
                }

                let (return_placeholder, return_was_annotated) = if let Some(ret) = return_type.clone() {
                    (ret, true)
                } else {
                    (type_checker.fresh_type_var(), false)
                };

                let placeholder_func_type = Type::Function {
                    params: positional_tys.clone(),
                    named_params: named_annos.clone(),
                    return_type: Box::new(return_placeholder.clone()),
                };
                type_checker.add_local_type(name.clone(), placeholder_func_type);
                type_checker.add_function_sig(
                    name.clone(),
                    FunctionSig {
                        positional: positional_tys.clone(),
                        named: named_sigs.clone(),
                        return_type: Some(return_placeholder.clone()),
                        annotated: positional_origin.clone(),
                    },
                );

                // The frame collects every `return` as it is checked, while its
                // scope is still live (see `TypeChecker::push_return_frame`). A
                // traversal *after* the body sees every nested `if`/`while`/`for`/
                // `try` scope already popped, which is why an annotated local
                // returned from inside one came back as a fresh type variable.
                // Popped on both paths, like the closure case: propagating the
                // body's error through `?` before popping would leave a dead frame
                // on the stack for an enclosing function's returns to land in.
                type_checker.push_return_frame();
                let body_checked = body.type_check(type_checker);
                let collected_returns = type_checker.pop_return_frame();
                body_checked?;

                fn normalize_union(mut tys: Vec<Type>) -> Type {
                    let mut flat: Vec<Type> = Vec::new();
                    for t in tys.drain(..) {
                        match t {
                            Type::Union(inner) => flat.extend(inner),
                            other => flat.push(other),
                        }
                    }
                    use alloc::collections::BTreeMap;
                    let mut by_key: BTreeMap<String, Type> = BTreeMap::new();
                    for t in flat {
                        by_key.entry(t.display()).or_insert(t);
                    }
                    let mut uniq: Vec<Type> = by_key.into_values().collect();
                    if uniq.len() == 1 {
                        uniq.remove(0)
                    } else {
                        Type::Union(uniq)
                    }
                }

                if return_was_annotated {
                    for ty in &collected_returns {
                        if !type_checker.is_assignable(ty, &return_placeholder) {
                            return Err(anyhow!(format!(
                                "Return type mismatch in function '{}': expected {}, got {}",
                                name,
                                return_placeholder.display(),
                                ty.display()
                            )));
                        }
                    }
                }
                for ty in &collected_returns {
                    type_checker.add_constraint(return_placeholder.clone(), ty.clone());
                }

                type_checker.pop_scope();
                type_checker.restore_pending_top_level(pending);

                let inferred_return = if return_was_annotated {
                    return_placeholder.clone()
                } else if collected_returns.is_empty() {
                    Type::Nil
                } else {
                    normalize_union(collected_returns)
                };

                if type_checker.defer_strict_function_checks() {
                    let pending_positional = params
                        .iter()
                        .cloned()
                        .zip(positional_tys.iter().cloned())
                        .zip(positional_origin.iter().copied())
                        .map(|((name, ty), annotated)| PendingStrictParam { name, ty, annotated })
                        .collect();
                    let pending_named = named_params
                        .iter()
                        .map(|param| param.name.clone())
                        .zip(named_annos.iter().map(|param| param.ty.clone()))
                        .zip(named_origin.iter().copied())
                        .map(|((name, ty), annotated)| PendingStrictParam { name, ty, annotated })
                        .collect();

                    let final_func_type = Type::Function {
                        params: positional_tys.clone(),
                        named_params: named_annos.clone(),
                        return_type: Box::new(inferred_return.clone()),
                    };

                    if let Some(self_ty) = type_checker.current_impl_self_type().cloned() {
                        type_checker.add_method_sig(&self_ty, name, final_func_type.clone());
                    }

                    type_checker.add_local_type(name.clone(), final_func_type);
                    type_checker.add_function_sig(
                        name.clone(),
                        FunctionSig {
                            positional: positional_tys,
                            named: named_sigs,
                            return_type: Some(inferred_return.clone()),
                            annotated: positional_origin.clone(),
                        },
                    );
                    type_checker.add_pending_strict_function(PendingStrictFunction {
                        name: name.clone(),
                        positional: pending_positional,
                        named: pending_named,
                        return_type: inferred_return,
                        return_annotated: return_was_annotated,
                    });

                    return Ok(());
                }

                let subs = type_checker.solve_constraints()?;
                let resolved_positional: Vec<Type> = positional_tys
                    .into_iter()
                    .map(|ty| type_checker.apply_substitutions(ty, &subs))
                    .collect();

                let resolved_named_annos: Vec<FunctionNamedParamType> = named_annos
                    .into_iter()
                    .map(|mut ann| {
                        ann.ty = type_checker.apply_substitutions(ann.ty, &subs);
                        ann
                    })
                    .collect();

                let resolved_named_sigs: Vec<NamedParamSig> = named_sigs
                    .into_iter()
                    .map(|mut sig| {
                        sig.ty = type_checker.apply_substitutions(sig.ty, &subs);
                        sig
                    })
                    .collect();

                let resolved_return = normalize_union(vec![type_checker.apply_substitutions(inferred_return, &subs)]);

                if type_checker.strict_any() {
                    let mut issues: Vec<String> = Vec::new();
                    let mut first_param_name = None;
                    for (idx, param_name) in params.iter().enumerate() {
                        if !positional_origin[idx]
                            && TypeChecker::type_is_strict_any_unresolved(&resolved_positional[idx])
                        {
                            first_param_name.get_or_insert_with(|| param_name.clone());
                            issues.push(format!("parameter '{}'", param_name));
                        }
                    }
                    for (idx, np) in named_params.iter().enumerate() {
                        if !named_origin[idx]
                            && TypeChecker::type_is_strict_any_unresolved(&resolved_named_annos[idx].ty)
                        {
                            first_param_name.get_or_insert_with(|| np.name.clone());
                            issues.push(format!("named parameter '{}'", np.name));
                        }
                    }
                    if !return_was_annotated && TypeChecker::type_is_strict_any_unresolved(&resolved_return) {
                        issues.push("return type".to_string());
                    }
                    if !issues.is_empty() {
                        return Err(TypeChecker::implicit_any_type_err(
                            name,
                            &issues,
                            first_param_name.as_deref(),
                        ));
                    }
                }

                let final_func_type = Type::Function {
                    params: resolved_positional.clone(),
                    named_params: resolved_named_annos.clone(),
                    return_type: Box::new(resolved_return.clone()),
                };

                if let Some(self_ty) = type_checker.current_impl_self_type().cloned() {
                    type_checker.add_method_sig(&self_ty, name, final_func_type.clone());
                }

                type_checker.add_local_type(name.clone(), final_func_type);
                type_checker.add_function_sig(
                    name.clone(),
                    FunctionSig {
                        positional: resolved_positional,
                        named: resolved_named_sigs,
                        return_type: Some(resolved_return),
                        annotated: positional_origin.clone(),
                    },
                );

                Ok(())
            }
            Stmt::If {
                condition,
                then_stmt,
                else_stmt,
            } => {
                condition.type_check(type_checker)?;

                // then 分支
                type_checker.push_scope();
                then_stmt.type_check(type_checker)?;
                type_checker.pop_scope();

                // else 分支
                if let Some(else_stmt) = else_stmt {
                    type_checker.push_scope();
                    else_stmt.type_check(type_checker)?;
                    type_checker.pop_scope();
                }

                Ok(())
            }
            Stmt::IfLet {
                pattern,
                value,
                then_stmt,
                else_stmt,
            } => {
                // 检查值表达式的类型
                let value_type = value.type_check(type_checker)?;

                // 为 then 分支创建新作用域，以便模式变量绑定
                type_checker.push_scope();

                // 根据模式与被匹配值类型，添加类型绑定，并校验模式兼容性
                type_checker.add_bindings_for_pattern(pattern, &value_type).ok();

                // 现在检查 then 分支
                then_stmt.type_check(type_checker)?;

                // 弹出作用域
                type_checker.pop_scope();

                // 检查 else 分支（如果有）
                if let Some(else_stmt) = else_stmt {
                    else_stmt.type_check(type_checker)?;
                }

                Ok(())
            }
            Stmt::While { condition, body } => {
                condition.type_check(type_checker)?;

                // 检查循环体
                body.type_check(type_checker)?;

                Ok(())
            }
            Stmt::WhileLet { pattern, value, body } => {
                // 检查值表达式的类型
                let value_type = value.type_check(type_checker)?;

                // 为循环体创建新作用域，以便模式变量绑定
                type_checker.push_scope();

                // 根据模式与被匹配值类型，添加类型绑定，并校验模式兼容性
                type_checker.add_bindings_for_pattern(pattern, &value_type).ok();

                // 现在简化为检查循环体
                body.type_check(type_checker)?;

                // 弹出作用域
                type_checker.pop_scope();

                Ok(())
            }
            Stmt::For {
                pattern,
                iterable,
                body,
            } => {
                // 检查可迭代表达式的类型
                let iter_type = iterable.type_check(type_checker)?;

                // 验证可迭代类型
                match iter_type {
                    Type::List(_) | Type::String | Type::Map(_, _) | Type::Set(_) | Type::Any | Type::Variable(_) => {
                        // 这些类型都是可迭代的（Any和类型变量在运行时确定）
                    }
                    _ => {
                        return Err(anyhow!(format!(
                            "For loop iterable must be List, String, Map, or Set, but got {}",
                            iter_type.display()
                        )));
                    }
                }

                // 为模式匹配创建新的作用域
                type_checker.push_scope();

                // 根据模式添加变量类型
                Self::add_pattern_types(pattern, &iter_type, type_checker)?;

                // 检查循环体
                body.type_check(type_checker)?;

                // 弹出作用域
                type_checker.pop_scope();

                Ok(())
            }
            Stmt::Expr(expr) => {
                // 表达式语句，只检查类型，不使用结果
                expr.type_check(type_checker)?;
                Ok(())
            }
            Stmt::Block { statements } => {
                // 为块语句创建新的作用域
                type_checker.push_scope();

                // 检查块中的所有语句
                for stmt in statements {
                    stmt.type_check(type_checker)?;
                }

                // 弹出作用域
                type_checker.pop_scope();

                Ok(())
            }
            Stmt::Try {
                body,
                catch_var,
                handler,
            } => {
                // Straight-line scopes, which is the point of keeping this a
                // statement: as `let [ok, e] = try$call(|| { body })` the checker
                // saw a closure and a destructuring `let`, so an annotated local
                // assigned inside the body came back out as a fresh type
                // variable — `let r: Int = 0; try { r = x; } catch e {}` failed
                // with "expected Int, got 'T2".
                type_checker.push_scope();
                for stmt in body {
                    stmt.type_check(type_checker)?;
                }
                type_checker.pop_scope();

                type_checker.push_scope();
                // The caught value is the message string for a plain raise and
                // the raised value itself for `error(v)`, so the binding is as
                // wide as the top type (see `vm::exec::handler`).
                type_checker.add_local_type(catch_var.clone(), Type::Any);
                for stmt in handler {
                    stmt.type_check(type_checker)?;
                }
                type_checker.pop_scope();
                Ok(())
            }
            Stmt::Import(_) => {
                // Use 语句暂时不需要类型检查
                Ok(())
            }
            Stmt::Return { value } => {
                // Recorded here rather than by a traversal after the body: this is
                // the only point at which the returned expression's scope is still
                // live (see `TypeChecker::push_return_frame`).
                let ty = match value {
                    Some(expr) => expr.type_check(type_checker)?,
                    None => Type::Nil,
                };
                type_checker.record_return(ty);
                Ok(())
            }
            Stmt::Break | Stmt::Continue => {
                // 控制流语句暂时不需要类型检查
                Ok(())
            }
            Stmt::Define { name, value, span } => {
                // `x := v` binds exactly what `let x = v` binds, and lowers
                // through the same `lower_define`. Skipping it here meant the
                // name had no type at all: every later read of it went through
                // `check_identifier`'s last line and got a fresh type variable,
                // so nothing downstream of a `:=` could be checked either.
                let value_type = value.type_check(type_checker)?;
                type_checker.define_top_level(name);
                type_checker.add_local_type(name.clone(), value_type);
                if let Some(span) = span {
                    type_checker.record_bindings(span, false, core::iter::once(name.as_str()));
                }
                Ok(())
            }
            Stmt::Empty => Ok(()),
        }
    }

    /// 为 for 循环模式添加类型信息
    fn add_pattern_types(pattern: &ForPattern, iter_type: &Type, type_checker: &mut TypeChecker) -> Result<()> {
        match pattern {
            ForPattern::Variable(name) => {
                // 根据可迭代类型确定变量类型
                let var_type = match iter_type {
                    Type::List(inner) => (**inner).clone(),
                    Type::String => Type::String,
                    Type::Map(k, v) => {
                        // Map 迭代返回 [key, value] 对，使用 Tuple 表示
                        Type::Tuple(vec![(**k).clone(), (**v).clone()])
                    }
                    Type::Set(inner) => (**inner).clone(),
                    _ => Type::Any,
                };
                type_checker.add_local_type(name.clone(), var_type);
            }
            ForPattern::Ignore => {
                // 忽略模式，不需要添加类型
            }
            ForPattern::Tuple(patterns) => match iter_type {
                Type::List(inner_types) => {
                    for pattern in patterns {
                        Self::add_pattern_types(pattern, inner_types, type_checker)?;
                    }
                }
                Type::Map(k, v) => {
                    // 直接迭代 Map：元素为 [key, value]
                    for (i, pattern) in patterns.iter().enumerate() {
                        let elem_ty = if i == 0 { (**k).clone() } else { (**v).clone() };
                        Self::add_pattern_types(pattern, &elem_ty, type_checker)?;
                    }
                }
                _ => {}
            },
            ForPattern::Array { patterns, rest } => match iter_type {
                Type::List(inner_types) => {
                    // 为固定模式的每个部分添加类型
                    for pattern in patterns {
                        Self::add_pattern_types(pattern, inner_types, type_checker)?;
                    }
                    if let Some(rest_var) = rest {
                        type_checker.add_local_type(rest_var.clone(), (**inner_types).clone());
                    }
                }
                Type::Map(k, v) => {
                    // 为 [k, v] 模式提供类型
                    for (i, pattern) in patterns.iter().enumerate() {
                        let elem_ty = if i == 0 { (**k).clone() } else { (**v).clone() };
                        Self::add_pattern_types(pattern, &elem_ty, type_checker)?;
                    }
                    // 数组解构下的 rest 在 Map 迭代语义中不太适用，忽略处理
                }
                _ => {}
            },
            ForPattern::Object(entries) => {
                // 目前仅支持元素为 Map<K, V> 的列表：List<Map<K,V>>
                // 将每个绑定变量加入作用域，类型为 V（未知则 Any）
                let value_ty = match iter_type {
                    Type::List(inner) => match &**inner {
                        Type::Map(_k, v) => Some((**v).clone()),
                        _ => None,
                    },
                    // 直接迭代 Map 时 create_iterator 产生 [key,value] 对，不适配对象解构
                    _ => None,
                }
                .unwrap_or(Type::Any);

                for (_key, subpat) in entries {
                    match subpat {
                        ForPattern::Variable(name) => {
                            type_checker.add_local_type(name.clone(), value_ty.clone());
                        }
                        ForPattern::Ignore => {}
                        // 对于嵌套模式，保守地继续使用相同的 value_ty
                        other => {
                            Self::add_pattern_types(other, &value_ty, type_checker)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

impl Program {
    /// Registers every top-level function's signature before any body is
    /// checked.
    ///
    /// Functions are hoisted at run time — the compiler builds the whole
    /// function table before the entry executes — so a call may appear above
    /// the definition. The checker walked statements in order, so such a call
    /// found no signature and produced `Any`, and the error surfaced somewhere
    /// else entirely: `((n / helper(2)) as Int)` failed with "cannot cast
    /// Box<Any> to Int" if `helper` happened to be defined further down the
    /// file, and type-checked if it was defined above.
    ///
    /// What is registered here is only what the annotations state; an
    /// unannotated parameter or return is `Any`, exactly as permissive as
    /// before. The ordered walk replaces each entry with the inferred
    /// signature when it reaches the definition.
    fn predeclare_function_signatures(&self, type_checker: &mut TypeChecker) {
        fn item(stmt: &Stmt) -> &Stmt {
            match stmt {
                Stmt::Attributed { item, .. } => self::item_of(item),
                other => other,
            }
        }
        for stmt in &self.statements {
            let Stmt::Function {
                name,
                params,
                param_types,
                named_params,
                return_type,
                ..
            } = item(stmt)
            else {
                continue;
            };
            let positional: Vec<Type> = (0..params.len())
                .map(|i| param_types.get(i).cloned().flatten().unwrap_or(Type::Any))
                .collect();
            let annotated: Vec<bool> = (0..params.len())
                .map(|i| param_types.get(i).cloned().flatten().is_some())
                .collect();
            let named: Vec<NamedParamSig> = named_params
                .iter()
                .map(|param| NamedParamSig {
                    name: param.name.clone(),
                    ty: param.type_annotation.clone().unwrap_or(Type::Any),
                    has_default: param.default.is_some(),
                })
                .collect();
            let returns = return_type.clone().unwrap_or(Type::Any);
            let named_annos: Vec<FunctionNamedParamType> = named
                .iter()
                .map(|param| FunctionNamedParamType {
                    name: param.name.clone(),
                    ty: param.ty.clone(),
                    has_default: param.has_default,
                })
                .collect();
            type_checker.add_local_type(
                name.clone(),
                Type::Function {
                    params: positional.clone(),
                    named_params: named_annos,
                    return_type: Box::new(returns.clone()),
                },
            );
            type_checker.add_function_sig(
                name.clone(),
                FunctionSig {
                    positional,
                    named,
                    return_type: Some(returns),
                    annotated,
                },
            );
        }
    }

    /// 类型检查程序
    pub fn type_check(&self, type_checker: &mut TypeChecker) -> Result<()> {
        self.predeclare_function_signatures(type_checker);
        type_checker.set_pending_top_level(self.top_level_binding_names());
        if type_checker.strict_any() {
            let previous_defer = type_checker.begin_deferred_strict_function_checks();
            let result = (|| {
                for stmt in &self.statements {
                    stmt.type_check(type_checker)?;
                }
                type_checker.finalize_deferred_strict_function_checks()
            })();
            type_checker.restore_deferred_strict_function_checks(previous_defer);
            return result;
        }

        for stmt in &self.statements {
            stmt.type_check(type_checker)?;
        }
        Ok(())
    }

    /// Type-check every statement, reporting all the errors instead of the first.
    ///
    /// `type_check` stops at the first failure, which is what a compiler wants:
    /// the program is not going to run either way. A tool wants the opposite —
    /// one mistyped line should not take the diagnostics for the other forty
    /// with it, nor the types recorded for them (see
    /// `TypeChecker::observe_bindings`).
    pub fn type_check_collecting(&self, type_checker: &mut TypeChecker) -> Vec<anyhow::Error> {
        self.predeclare_function_signatures(type_checker);
        type_checker.set_pending_top_level(self.top_level_binding_names());

        let strict = type_checker.strict_any();
        let previous_defer = strict.then(|| type_checker.begin_deferred_strict_function_checks());

        let depth = type_checker.scope_depth();
        let mut errors = Vec::new();
        for stmt in &self.statements {
            if let Err(err) = stmt.type_check(type_checker) {
                errors.push(err);
                // The failed statement returned through the `?` that would have
                // closed its scopes; leaving them open would check the next
                // statement against bindings it cannot see.
                type_checker.unwind_scopes_to(depth);
            }
        }

        if let Some(previous_defer) = previous_defer {
            if let Err(err) = type_checker.finalize_deferred_strict_function_checks() {
                errors.push(err);
            }
            type_checker.restore_deferred_strict_function_checks(previous_defer);
        }
        errors
    }
}

impl Program {
    /// Every name bound by a top-level `let`/`const`, in any pattern.
    ///
    /// Used to catch a read of one from *above* its definition — see
    /// `TypeChecker::pending_top_level`. Function declarations are not in here:
    /// they are hoisted, and calling one declared below is ordinary.
    fn top_level_binding_names(&self) -> crate::compat::collections::HashSet<String> {
        let mut names = crate::compat::collections::HashSet::new();
        for stmt in &self.statements {
            if let Stmt::Let { pattern, .. } = item_of(stmt) {
                collect_pattern_names(pattern, &mut names);
            }
        }
        names
    }
}

fn pattern_names(pattern: &Pattern) -> crate::compat::collections::HashSet<String> {
    let mut names = crate::compat::collections::HashSet::new();
    collect_pattern_names(pattern, &mut names);
    names
}

fn collect_pattern_names(pattern: &Pattern, out: &mut crate::compat::collections::HashSet<String>) {
    match pattern {
        Pattern::Variable(name) => {
            out.insert(name.clone());
        }
        Pattern::List { patterns, rest } => {
            for item in patterns {
                collect_pattern_names(item, out);
            }
            if let Some(rest) = rest {
                out.insert(rest.clone());
            }
        }
        Pattern::Map { patterns, rest } => {
            for (_, value) in patterns {
                collect_pattern_names(value, out);
            }
            if let Some(rest) = rest {
                out.insert(rest.clone());
            }
        }
        _ => {}
    }
}

/// The declaration an attribute wraps, however many attributes there are.
fn item_of(stmt: &Stmt) -> &Stmt {
    match stmt {
        Stmt::Attributed { item, .. } => item_of(item),
        other => other,
    }
}

/// Binds every name in `pattern` to the type that position holds in `value_ty`.
///
/// Distribution, not the whole value: a list/tuple pattern takes tuple positions
/// or the list's element type, a map pattern takes the map's value type, and a
/// union distributes into a union of what each member yields. Anything this
/// cannot see through (a type variable, `Any`, a mismatched shape) yields `Any`
/// — permissive on purpose, so an unknown shape never rejects on an invented
/// type.
fn bind_pattern_types(pattern: &Pattern, value_ty: &Type, is_const: bool, tc: &mut TypeChecker) {
    match pattern {
        Pattern::Variable(name) => tc.add_local_binding(name.clone(), value_ty.clone(), is_const),
        Pattern::List { patterns, rest } => {
            for (index, sub) in patterns.iter().enumerate() {
                bind_pattern_types(sub, &element_type_at(value_ty, index), is_const, tc);
            }
            if let Some(rest) = rest {
                // The tail holds *every* remaining position, so its element type
                // is their union — taking only position `patterns.len()` typed
                // `rest` of a `Tuple<Int, String, Bool>` as `List<String>`, which
                // both rejected `rest[1]` as a `Bool` and accepted `rest` as a
                // `List<String>`. Positions themselves are lost, hence `List`.
                let tail = match tail_element_type(value_ty, patterns.len()) {
                    Type::Any => Type::Any,
                    element => Type::List(Box::new(element)),
                };
                tc.add_local_binding(rest.clone(), tail, is_const);
            }
        }
        Pattern::Map { patterns, rest } => {
            for (_, sub) in patterns {
                bind_pattern_types(sub, &map_value_type(value_ty), is_const, tc);
            }
            if let Some(rest) = rest {
                tc.add_local_binding(rest.clone(), value_ty.clone(), is_const);
            }
        }
        // A guard does not change what the inner pattern binds; alternatives bind
        // the same names, so each alternative is distributed independently.
        Pattern::Guard { pattern, .. } => bind_pattern_types(pattern, value_ty, is_const, tc),
        Pattern::Or(alternatives) => {
            for alternative in alternatives {
                bind_pattern_types(alternative, value_ty, is_const, tc);
            }
        }
        Pattern::Literal(_) | Pattern::Wildcard | Pattern::Range { .. } => {}
    }
}

/// The element type of everything from position `from` onward — what a `..rest`
/// binding holds.
fn tail_element_type(value_ty: &Type, from: usize) -> Type {
    match value_ty {
        Type::Tuple(elements) => union_of(elements.iter().skip(from).cloned()),
        Type::List(element) => (**element).clone(),
        Type::Optional(inner) => tail_element_type(inner, from),
        Type::Union(members) => union_of(members.iter().map(|member| tail_element_type(member, from))),
        _ => Type::Any,
    }
}

/// The type at position `index` of a destructured value.
fn element_type_at(value_ty: &Type, index: usize) -> Type {
    match value_ty {
        Type::Tuple(elements) => elements.get(index).cloned().unwrap_or(Type::Any),
        // A `List<T>`'s element type applies to every position — except when `T`
        // is itself a union, which is the *join* over positions rather than what
        // any one of them holds (a heterogeneous literal can infer
        // `List<Int | String>`). Claiming the union per position would reject
        // `let [first, _] = [1, {…}]; first + 1`, so the honest answer for a lost
        // per-position type is `Any`.
        Type::List(element) => match &**element {
            Type::Union(_) => Type::Any,
            element => element.clone(),
        },
        Type::Optional(inner) => element_type_at(inner, index),
        Type::Union(members) => union_of(members.iter().map(|member| element_type_at(member, index))),
        _ => Type::Any,
    }
}

fn map_value_type(value_ty: &Type) -> Type {
    match value_ty {
        // As with a list's element type: a union value type is the join over *all*
        // keys, not what the key this pattern names holds, so binding the union
        // would reject legitimate uses of the extracted value.
        Type::Map(_, value) => match &**value {
            Type::Union(_) => Type::Any,
            value => value.clone(),
        },
        Type::Optional(inner) => map_value_type(inner),
        Type::Union(members) => union_of(members.iter().map(map_value_type)),
        _ => Type::Any,
    }
}

/// Collapses distributed alternatives: identical types stay themselves, `Any`
/// anywhere swallows the rest (nothing is known), otherwise a union.
fn union_of(types: impl IntoIterator<Item = Type>) -> Type {
    let mut out: Vec<Type> = Vec::new();
    for ty in types {
        if ty == Type::Any {
            return Type::Any;
        }
        if !out.contains(&ty) {
            out.push(ty);
        }
    }
    match out.len() {
        0 => Type::Any,
        1 => out.pop().expect("checked len"),
        _ => Type::Union(out),
    }
}

/// The integer value of a literal expression.
///
/// A leading minus needs no special case: the lexer folds it into the literal
/// (`Token::Int(-5)`), and constant folding has already run by the time the
/// checker sees the expression, so `1 + 2` arrives here as `3`.
fn int_literal_value(expr: &crate::expr::Expr) -> Option<i128> {
    use crate::expr::Expr;
    use crate::val::LiteralVal;

    match expr {
        Expr::Literal(LiteralVal::Int(value)) => Some(i128::from(*value)),
        Expr::Paren(inner) => int_literal_value(inner),
        _ => None,
    }
}
