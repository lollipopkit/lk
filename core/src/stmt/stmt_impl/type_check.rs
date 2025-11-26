use super::{ForPattern, Program, Stmt};
use crate::{
    expr::Pattern,
    token::ParseError,
    typ::{FunctionSig, NamedParamSig, StructDef, TraitDef, TypeAlias as AliasDef, TypeChecker},
    val::{FunctionNamedParamType, Type},
};
use anyhow::{Result, anyhow};
use std::collections::HashMap;

impl Stmt {
    /// 静态类型检查语句
    pub fn type_check(&self, type_checker: &mut TypeChecker) -> Result<()> {
        match self {
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

                // 如果有类型注解，验证类型匹配
                if let Some(expected_type) = type_annotation
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

                // Extract variables from pattern and add their types to the type checker
                if let Some(pattern_vars) = extract_pattern_variables(pattern) {
                    let var_type = type_annotation.clone().unwrap_or(expr_type);
                    for var_name in pattern_vars {
                        type_checker.add_local_binding(var_name, var_type.clone(), *is_const);
                    }
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
                    // 检查操作类型兼容性 (var_type op expr_type -> var_type)
                    // 简化：假设所有算术操作都是类型兼容的
                    if !type_checker.is_assignable(&expr_type, var_type)
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
                    },
                );

                body.type_check(type_checker)?;

                fn collect_return_types(stmt: &Stmt, tc: &mut TypeChecker, out: &mut Vec<Type>) -> anyhow::Result<()> {
                    match stmt {
                        Stmt::Return { value } => {
                            if let Some(expr) = value {
                                let ty = expr.type_check(tc)?;
                                out.push(ty);
                            } else {
                                out.push(Type::Nil);
                            }
                        }
                        Stmt::If {
                            condition,
                            then_stmt,
                            else_stmt,
                        } => {
                            let _ = condition.type_check(tc)?;
                            collect_return_types(then_stmt, tc, out)?;
                            if let Some(es) = else_stmt.as_deref() {
                                collect_return_types(es, tc, out)?;
                            }
                        }
                        Stmt::IfLet {
                            then_stmt,
                            else_stmt,
                            value: _,
                            pattern: _,
                        } => {
                            collect_return_types(then_stmt, tc, out)?;
                            if let Some(es) = else_stmt.as_deref() {
                                collect_return_types(es, tc, out)?;
                            }
                        }
                        Stmt::While { condition, body } => {
                            let _ = condition.type_check(tc)?;
                            collect_return_types(body, tc, out)?;
                        }
                        Stmt::WhileLet { body, .. } => {
                            collect_return_types(body, tc, out)?;
                        }
                        Stmt::For { body, .. } => {
                            collect_return_types(body, tc, out)?;
                        }
                        Stmt::Block { statements } => {
                            for s in statements {
                                collect_return_types(s, tc, out)?;
                            }
                        }
                        _ => {}
                    }
                    Ok(())
                }

                fn normalize_union(mut tys: Vec<Type>) -> Type {
                    let mut flat: Vec<Type> = Vec::new();
                    for t in tys.drain(..) {
                        match t {
                            Type::Union(inner) => flat.extend(inner),
                            other => flat.push(other),
                        }
                    }
                    use std::collections::BTreeMap;
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

                let mut collected_returns: Vec<Type> = Vec::new();
                collect_return_types(body, type_checker, &mut collected_returns)?;

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

                type_checker.pop_scope();

                let inferred_return = if return_was_annotated {
                    return_placeholder.clone()
                } else if collected_returns.is_empty() {
                    Type::Nil
                } else {
                    normalize_union(collected_returns)
                };

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

                let resolved_return = type_checker.apply_substitutions(inferred_return, &subs);

                fn type_is_unresolved(ty: &Type) -> bool {
                    matches!(ty, Type::Any) || ty.contains_variables()
                }

                if type_checker.strict_any() {
                    let mut issues: Vec<String> = Vec::new();
                    for (idx, param_name) in params.iter().enumerate() {
                        if !positional_origin[idx] && type_is_unresolved(&resolved_positional[idx]) {
                            issues.push(format!("parameter '{}'", param_name));
                        }
                    }
                    for (idx, np) in named_params.iter().enumerate() {
                        if !named_origin[idx] && type_is_unresolved(&resolved_named_annos[idx].ty) {
                            issues.push(format!("named parameter '{}'", np.name));
                        }
                    }
                    if !return_was_annotated && type_is_unresolved(&resolved_return) {
                        issues.push("return type".to_string());
                    }
                    if !issues.is_empty() {
                        return Err(anyhow!(format!(
                            "Function '{}' infers implicit Any for {}; add explicit annotations",
                            name,
                            issues.join(", ")
                        )));
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
                    },
                );

                Ok(())
            }
            Stmt::If {
                condition,
                then_stmt,
                else_stmt,
            } => {
                let cond_type = condition.type_check(type_checker)?;
                if !type_checker.is_assignable(&cond_type, &Type::Bool) {
                    return Err(anyhow!(format!(
                        "If condition must be Bool, but got {}",
                        cond_type.display()
                    )));
                }

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
                // 条件表达式必须是 Bool 类型
                let cond_type = condition.type_check(type_checker)?;
                if !type_checker.is_assignable(&cond_type, &Type::Bool) {
                    return Err(anyhow!(format!(
                        "While condition must be Bool, but got {}",
                        cond_type.display()
                    )));
                }

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
                    Type::List(_) | Type::String | Type::Map(_, _) => {
                        // 这些类型都是可迭代的
                    }
                    _ => {
                        return Err(anyhow!(format!(
                            "For loop iterable must be List, String, or Map, but got {}",
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
            Stmt::Import(_) => {
                // Import 语句暂时不需要类型检查
                Ok(())
            }
            Stmt::Break | Stmt::Continue | Stmt::Return { .. } => {
                // 控制流语句暂时不需要类型检查
                Ok(())
            }
            Stmt::Define { .. } | Stmt::Empty => {
                // Define 语句和空语句暂时不需要类型检查
                Ok(())
            }
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
    /// 类型检查程序
    pub fn type_check(&self, type_checker: &mut TypeChecker) -> Result<()> {
        for stmt in &self.statements {
            stmt.type_check(type_checker)?;
        }
        Ok(())
    }
}

/// Helper method to extract variable names from a pattern for type checking
fn extract_pattern_variables(pattern: &Pattern) -> Option<Vec<String>> {
    let mut variables = Vec::new();

    fn collect_vars(pattern: &Pattern, vars: &mut Vec<String>) {
        match pattern {
            Pattern::Variable(name) => {
                vars.push(name.clone());
            }
            Pattern::List { patterns, rest } => {
                for pattern in patterns {
                    collect_vars(pattern, vars);
                }
                if let Some(rest_var) = rest {
                    vars.push(rest_var.clone());
                }
            }
            Pattern::Map { patterns, rest } => {
                for (_, pattern) in patterns {
                    collect_vars(pattern, vars);
                }
                if let Some(rest_var) = rest {
                    vars.push(rest_var.clone());
                }
            }
            Pattern::Or(patterns) => {
                for pattern in patterns {
                    collect_vars(pattern, vars);
                }
            }
            Pattern::Guard { pattern, .. } => {
                collect_vars(pattern, vars);
            }
            // Other pattern types don't bind variables
            Pattern::Literal(_) | Pattern::Wildcard | Pattern::Range { .. } => {}
        }
    }

    collect_vars(pattern, &mut variables);

    // Remove duplicates (can happen with OR patterns)
    variables.sort();
    variables.dedup();

    if variables.is_empty() { None } else { Some(variables) }
}
