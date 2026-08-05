use super::{ForPattern, Program, Stmt};
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::{
    expr::Pattern,
    token::ParseError,
    typ::{
        FunctionSig, NamedParamSig, PendingStrictFunction, PendingStrictParam, StructDef, TraitDef,
        TypeAlias as AliasDef, TypeChecker, union_of,
    },
    val::{FunctionNamedParamType, Type},
};
use anyhow::{Result, anyhow};
use hashbrown::HashMap;

impl Stmt {
    /// 静态类型检查语句
    pub fn type_check(&self, type_checker: &mut TypeChecker) -> Result<()> {
        let result = self.type_check_inner(type_checker);
        let Err(error) = result else {
            return Ok(());
        };
        // Give the error this statement's position, if it does not have one.
        // An expression has no position of its own, so without this the only
        // way to place the error is to hunt the token stream for a token that
        // looks like the offending expression — which finds the first such
        // token in the file rather than this one.
        let Some(span) = self.span() else {
            return Err(error);
        };
        Err(match error.downcast::<crate::typ::TypeError>() {
            Ok(mut type_error) => {
                type_error.attach_span(&span);
                anyhow!(type_error)
            }
            Err(error) => error,
        })
    }

    /// This statement's own position, for the statements that carry one.
    fn span(&self) -> Option<crate::token::Span> {
        match self {
            Stmt::Let { span, .. }
            | Stmt::Assign { span, .. }
            | Stmt::CompoundAssign { span, .. }
            | Stmt::Define { span, .. }
            // The variant a bare call statement is, and therefore the one every
            // argument type error is raised under. It carried no span, so those
            // errors carried no position — the same mistake written as a `let`
            // said `1:1-6`, and written as a call said nothing.
            | Stmt::Expr { span, .. } => span.clone(),
            _ => None,
        }
    }

    fn type_check_inner(&self, type_checker: &mut TypeChecker) -> Result<()> {
        match self {
            Stmt::Attributed { item, .. } | Stmt::Defer { body: item, .. } => item.type_check(type_checker),
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
                            type_checker
                                .check_type_annotation(ty, &alloc::format!("field '{k}' of struct '{name}'"))?;
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
            Stmt::Trait { name, methods, .. } => {
                // Register trait with method signatures
                let mut map = HashMap::with_capacity(methods.len());
                for (m, ty) in methods.iter() {
                    // A trait's method signatures are annotations like any
                    // other, and were the one kind nothing checked: a trait
                    // could promise a type that does not exist, and every impl
                    // of it would then be measured against nothing.
                    type_checker.check_type_annotation(ty, &alloc::format!("method '{m}' of trait '{name}'"))?;
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
                trait_name,
                target_type,
                methods,
            } => {
                // The *target* was unchecked while the trait name was checked
                // and the method bodies were checked: `impl Show for
                // Nonexistent { … }` registered methods on a type nothing
                // declares, so they could never be reached and nothing said so.
                type_checker.check_type_annotation(target_type, "the impl target")?;
                // A builtin container dispatches with its element type erased —
                // a `TypedList::Mixed` has nothing else to report — so
                // `impl T for List<Int>` names something the runtime cannot
                // tell from `List<String>`. It used to register under a key
                // nothing looks up, and the call failed later with "List has no
                // method", which is true and unhelpful.
                let resolved_target = type_checker.resolve_aliases(target_type);
                let erased = crate::typ::TypeChecker::dispatch_type(&resolved_target);
                if erased != resolved_target {
                    let bare = match erased {
                        crate::val::Type::List(_) => "List",
                        crate::val::Type::Map(_, _) => "Map",
                        crate::val::Type::Set(_) => "Set",
                        _ => "the bare type",
                    };
                    return Err(anyhow::anyhow!(
                        "Type Error: an impl target cannot name an element type: `{}` is not \
                         distinguishable from another element type at run time — write `{bare}`",
                        resolved_target.display()
                    ));
                }
                // Every method the trait declares has to be here. The check
                // existed (`TypeRegistry::validate_trait_impl`) and only ran at
                // *run* time, when the VM registers impls — so `lk check`, the
                // pre-flight command, passed a program that could not run and
                // said nothing. Trait defaults are already copied in by
                // `stmt::trait_defaults`, so "present" is the whole question.
                if let Some(trait_name) = trait_name
                    && let Some(trait_def) = type_checker.registry().get_trait(trait_name)
                {
                    let declared: Vec<String> = trait_def.methods.keys().cloned().collect();
                    for required in declared {
                        let present = methods
                            .iter()
                            .any(|method| matches!(item_of(method), Stmt::Function { name, .. } if *name == required));
                        if !present {
                            return Err(anyhow!(format!(
                                "Method '{required}' required by trait '{trait_name}' not implemented for type '{}'",
                                target_type.display()
                            )));
                        }
                    }
                }
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
                if let Some(annotation) = type_annotation {
                    type_checker.check_type_annotation(annotation, "this binding")?;
                }
                // 检查表达式的类型
                //
                // A function-type annotation flows *into* a lambda instead of
                // being compared against it afterwards. Checked in isolation a
                // lambda types as `('T0) -> Any`, which does not unify with the
                // annotation written for it — so a lambda could not be
                // annotated at all, while a named `fn` assigned to the same
                // binding was accepted. Same narrow bidirectionality as the
                // machine-int literal rule below, for the same reason: the
                // alternative is a feature nobody can use.
                let expr_type = type_checker.check_expr_against(value, type_annotation.as_ref())?;
                // A `let` at the top level may not take a name a *declaration*
                // already binds. A `fn` or a type declaration is hoisted, so
                // source order does not apply to it and "the `let` shadows it"
                // has no coherent meaning — it showed as `fn pick() {…}` then
                // `let pick = …;` resolving to the `let` in *either* order,
                // silently. Two `fn`s of one name were already refused; this is
                // the same collision, and it is the mistake that put a dead
                // `fn apply` next to a live `let apply` in `closure.lk`.
                //
                // Inside a callable body it *is* ordinary shadowing: the local
                // is order-sensitive within its scope and the declaration is
                // outside it.
                if !type_checker.inside_callable_body() {
                    for name in pattern_names(pattern) {
                        if let Some(kind) = type_checker.top_level_declaration_kind(&name) {
                            let error_msg = format!(
                                "`{name}` is already declared as a {kind} in this module: a {kind} is visible \
                                 before the line it is written on, so a `let` of the same name cannot shadow it — \
                                 rename one of them"
                            );
                            return if let Some(span) = span {
                                Err(anyhow!(ParseError::with_span(error_msg, span.clone())))
                            } else {
                                Err(anyhow!(error_msg))
                            };
                        }
                    }
                }
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
                    } else if *var_type == Type::Nil && expr_type != Type::Nil && !expr_type.contains_variables() {
                        // A binding that started as `nil` and now holds something
                        // is *that*, made optional — it is not `Nil` any more.
                        //
                        // Leaving it `Nil` is what made `let caught = nil; …;
                        // caught == "kaboom"` compare a string against nil. The
                        // comparison did not fail only because the solver ends in
                        // a rule that accepts any two disagreeing concrete types;
                        // the type was wrong either way, and everything reading
                        // it downstream — a hint, a hover, a completion — read
                        // the wrong one.
                        let widened = Type::Optional(Box::new(expr_type.clone()));
                        type_checker.add_local_type(name.clone(), widened);
                    } else if expr_type == Type::Nil && !matches!(var_type, Type::Optional(_) | Type::Any) {
                        // The other direction: a typed binding assigned `nil`
                        // becomes optional rather than staying what it was.
                        let widened = Type::Optional(Box::new(var_type.clone()));
                        type_checker.add_local_type(name.clone(), widened);
                    } else if expr_type.contains_variables() {
                        // Expression has unresolved type variables; add constraint instead of failing.
                        type_checker.add_constraint(expr_type, var_type.clone());
                    } else if var_type.contains_variables() {
                        // The same thing said the other way round, which was
                        // missing: a *binding* whose type is not yet resolved
                        // cannot reject an assignment either, because there is
                        // nothing settled to reject it against.
                        //
                        // Nothing hit it while `map.get` was typed `Any`. Once
                        // it started saying `Val?`, `let v = m.get(k); if v ==
                        // nil { v = 0; }` — a map read followed by a default —
                        // reported a mismatch between `'T?` and `Int`.
                        type_checker.add_constraint(var_type.clone(), expr_type);
                    } else if let Type::MachineInt(kind) = var_type
                        && !matches!(expr_type, Type::MachineInt(_))
                        && let Some(literal) = int_literal_value(value)
                    {
                        // A literal takes the variable's machine width, the same
                        // as `let x: u8 = 5` does one line earlier and as
                        // `x + 1` and `x > 1` do.
                        //
                        // This was the last of the four and it was found by
                        // converting a driver: `mask = 0xfffffffc;` on a `u32`
                        // was refused, which is the shape a register-mask
                        // variable has every time.
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
                // A `fn` inside another callable is *parsed*, and then the
                // compiler cannot find it: function indices are collected from
                // top-level statements only, so `fn outer() { fn helper() {…}
                // return helper(1); }` failed with "Compiler undefined function
                // `helper`" — a construct the grammar accepts and the backend
                // does not, reported in the backend's words.
                //
                // Refused here, in the language's words, with both ways to say
                // it instead. Supporting it is a feature (a nested `fn` cannot
                // capture, so it is a hoist plus a scoped name), not this.
                if type_checker.inside_callable_body() {
                    return Err(anyhow!(format!(
                        "a function cannot be declared inside another: move `{name}` to the top level, \
                         or bind a closure with `let {name} = |…| …;` if it needs the enclosing scope"
                    )));
                }
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
                    if let Some(ref ann) = annotated {
                        type_checker.check_type_annotation(ann, &alloc::format!("parameter '{param}'"))?;
                    }
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

                if let Some(ret) = return_type {
                    type_checker.check_type_annotation(ret, &alloc::format!("the return type of '{name}'"))?;
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
                type_checker.push_return_frame(return_was_annotated.then(|| return_placeholder.clone()));
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
                // Resolved before the solver sees them: a `type` alias is a
                // second spelling, not a second type, and the solver has no
                // registry to look it up in. `fn f(v: Int) -> U` with
                // `type U = Int` failed with "Cannot unify U with Int" —
                // aliases worked in a binding and in a parameter, and broke in
                // exactly one position.
                let declared_return = type_checker.resolve_aliases(&return_placeholder);
                for ty in &collected_returns {
                    let returned = type_checker.resolve_aliases(ty);
                    type_checker.add_constraint(declared_return.clone(), returned);
                }

                // A declared return type is a promise about *every* path. A
                // body that can reach its closing brace answers `nil` on that
                // path, and the failure surfaces at the caller: `g(false) + 1`
                // reported "Add expected numbers or strings, got Nil and Int",
                // naming the operator rather than the function that promised an
                // `Int`.
                //
                // Only annotations that exclude nil are checked — `-> Nil`,
                // `-> Any` and `-> Int?` all admit the fall-through value, and
                // an unannotated function's return type is *inferred* from what
                // it returns, so there is no promise to break.
                if return_was_annotated {
                    let declared = type_checker.resolve_aliases(&return_placeholder);
                    if !declared_admits_nil(&declared) && !super::flow::always_diverges(body) {
                        return Err(anyhow!(format!(
                            "function '{name}' can reach its end without returning, but declares `-> {}`: the path that falls through answers nil. Add a `return`, or declare `-> {}?`",
                            declared.display(),
                            declared.display()
                        )));
                    }
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
                type_checker.check_condition(condition)?;

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
                match &iter_type {
                    Type::List(_) | Type::String | Type::Map(_, _) | Type::Set(_) | Type::Any | Type::Variable(_) => {
                        // 这些类型都是可迭代的（Any和类型变量在运行时确定）
                    }
                    // 窗口按它自己的长度和索引迭代，两个后端都是如此
                    // （`to_iter` 把 slice 句柄原样交回去，不materialize）。
                    Type::Generic { name, .. } if name == "Slice" => {}
                    // Bytes 同理：它是一个序列，元素是 Int。
                    Type::Named(name) if name == "Bytes" => {}
                    // 元组就是列表，`is_assignable_to` 已经这么说了。
                    Type::Tuple(_) => {}
                    _ => {
                        return Err(anyhow!(format!(
                            "For loop iterable must be List, String, Map, Set, Bytes, Slice or Tuple, but got {}",
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
            Stmt::Expr { value: expr, .. } => {
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
            Stmt::Import(import) => {
                // The one thing an import contributes to type checking: the
                // *name* it binds. A standard library module bound to a name
                // shadows whatever that name meant, and `chan` is also a
                // callable global — see `is_imported_stdlib_module`.
                match import {
                    crate::stmt::ImportStmt::Module { module } => {
                        type_checker.add_imported_stdlib_module(module.clone());
                    }
                    crate::stmt::ImportStmt::ModuleAlias { alias, .. } => {
                        type_checker.add_imported_stdlib_module(alias.clone());
                    }
                    // `use { a, b } from m;` binds the *members*, not the
                    // module, and a file import binds a namespace that
                    // `imported_members` already covers.
                    _ => {}
                }
                Ok(())
            }
            Stmt::Return { value } => {
                // Recorded here rather than by a traversal after the body: this is
                // the only point at which the returned expression's scope is still
                // live (see `TypeChecker::push_return_frame`).
                let ty = match value {
                    // Against the declaration, not merely compared with it
                    // afterwards: a lambda typed in isolation does not match the
                    // function type written for it, so
                    // `fn make() -> (Int) -> Int { return |x| … }` was rejected.
                    Some(expr) => {
                        let declared = type_checker.declared_return();
                        type_checker.check_expr_against(expr, declared.as_ref())?
                    }
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

    /// Register every `struct`, `trait` and `type` alias before the ordered
    /// walk.
    ///
    /// Function signatures are hoisted (see
    /// [`Self::predeclare_function_signatures`]) so calling one declared below
    /// is ordinary. Type declarations were not, which no one noticed while an
    /// undeclared name silently became `Type::Named` — the annotation checked
    /// against nothing either way. The moment unknown names became an error,
    /// `fn f() -> Point { … }` above `struct Point { … }` started failing. A
    /// declaration's position in the file is not something a type should
    /// depend on.
    pub(crate) fn predeclare_type_declarations(&self, type_checker: &mut TypeChecker) {
        for stmt in &self.statements {
            match item_of(stmt) {
                Stmt::Struct { name, fields } => {
                    let fields = fields
                        .iter()
                        .map(|(field, ty)| (field.clone(), ty.clone().unwrap_or(Type::Any)))
                        .collect();
                    type_checker.registry_mut().register_struct(StructDef {
                        name: name.clone(),
                        fields,
                    });
                }
                Stmt::Trait { name, methods, .. } => {
                    type_checker.registry_mut().register_trait(TraitDef {
                        name: name.clone(),
                        methods: methods.iter().cloned().collect(),
                    });
                }
                Stmt::TypeAlias { name, target } => {
                    type_checker.registry_mut().register_type_alias(AliasDef {
                        name: name.clone(),
                        target_type: target.clone(),
                    });
                }
                _ => {}
            }
        }
    }

    /// Every `impl` method name, checked for the two collisions the language
    /// resolved silently by taking the last one.
    ///
    /// A program-level pass rather than something the ordered walk accumulates,
    /// for the reason `collect_function_names` is one: the question is about the
    /// *set* of declarations, and the checker's registry deliberately **replaces**
    /// a re-registered `impl` (a REPL context is reused across runs), so it
    /// cannot tell "declared twice here" from "seen again".
    ///
    /// Two mistakes, one namespace:
    ///  - the same method defined twice for one type — two `impl Show for P`
    ///    blocks each with a `show`, or two `impl P` blocks each with a `get`.
    ///    Two top-level `fn`s of one name were already refused.
    ///  - a method named like a *field*. `p.get(…)` cannot say which it means,
    ///    and which one it got depended on the argument count: `p.get()` read
    ///    the field (the method unreachable), while `p.f(3)` called the method
    ///    (the field's closure unreachable).
    fn check_method_name_collisions(&self) -> Result<()> {
        use crate::compat::collections::{HashMap, HashSet};

        let mut fields_of: HashMap<&str, HashSet<&str>> = HashMap::new();
        for stmt in &self.statements {
            if let Stmt::Struct { name, fields } = item_of(stmt) {
                fields_of.insert(name.as_str(), fields.iter().map(|(field, _)| field.as_str()).collect());
            }
        }

        let mut seen: HashSet<(String, &str)> = HashSet::new();
        for stmt in &self.statements {
            let Stmt::Impl {
                target_type, methods, ..
            } = item_of(stmt)
            else {
                continue;
            };
            let target = target_type.display();
            for method in methods {
                let Stmt::Function { name, .. } = item_of(method) else {
                    continue;
                };
                if !seen.insert((target.clone(), name.as_str())) {
                    return Err(anyhow!(format!(
                        "`{name}` is defined twice for `{target}`: two definitions of one method, \
                         where only the last one could ever run — remove one"
                    )));
                }
                if fields_of
                    .get(target.as_str())
                    .is_some_and(|fields| fields.contains(name.as_str()))
                {
                    return Err(anyhow!(format!(
                        "`{target}` already has a field named `{name}`, so `.{name}(…)` cannot say which \
                         one it means — rename the method or the field"
                    )));
                }
            }
        }
        Ok(())
    }

    /// 类型检查程序
    pub fn type_check(&self, type_checker: &mut TypeChecker) -> Result<()> {
        self.check_method_name_collisions()?;
        self.predeclare_type_declarations(type_checker);
        self.predeclare_function_signatures(type_checker);
        type_checker.set_pending_top_level(self.top_level_binding_names());

        // Both modes defer: constraints are solved once, at the end, instead of
        // at the end of every function.
        //
        // Only the strict one used to. The other solved the *global* constraint
        // pool each time a function finished, so a later function's constraints
        // met an earlier one's leftovers and which types were compared depended
        // on how much of the file had been read — cutting one example at 40
        // lines failed, at 50 passed, at 60 failed again. The pool cannot be
        // made per-function instead: inferring a parameter from a call site
        // further down is a feature.
        let previous_defer = type_checker.begin_deferred_strict_function_checks();
        let result = (|| {
            for stmt in &self.statements {
                stmt.type_check(type_checker)?;
            }
            type_checker.finalize_deferred_strict_function_checks()
        })();
        type_checker.restore_deferred_strict_function_checks(previous_defer);
        result
    }

    /// Type-check every statement, reporting all the errors instead of the first.
    ///
    /// `type_check` stops at the first failure, which is what a compiler wants:
    /// the program is not going to run either way. A tool wants the opposite —
    /// one mistyped line should not take the diagnostics for the other forty
    /// with it, nor the types recorded for them (see
    /// `TypeChecker::observe_bindings`).
    pub fn type_check_collecting(&self, type_checker: &mut TypeChecker) -> Vec<anyhow::Error> {
        // Collected like any other, so the LSP reports it and keeps going.
        let mut collision = Vec::new();
        if let Err(err) = self.check_method_name_collisions() {
            collision.push(err);
        }
        self.predeclare_type_declarations(type_checker);
        self.predeclare_function_signatures(type_checker);
        type_checker.set_pending_top_level(self.top_level_binding_names());

        // Deferred whether or not this is a strict run — see `Program::type_check`.
        let previous_defer = type_checker.begin_deferred_strict_function_checks();

        let depth = type_checker.scope_depth();
        let mut errors = collision;
        for stmt in &self.statements {
            if let Err(err) = stmt.type_check(type_checker) {
                errors.push(err);
                // The failed statement returned through the `?` that would have
                // closed its scopes; leaving them open would check the next
                // statement against bindings it cannot see.
                type_checker.unwind_scopes_to(depth);
            }
        }

        if let Err(err) = type_checker.finalize_deferred_strict_function_checks() {
            errors.push(err);
        }
        type_checker.restore_deferred_strict_function_checks(previous_defer);
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
        // A *negative* carrier cast to `u64` is a 64-bit bit pattern, and it is
        // measured as one — otherwise `let a: usize = 0xFFFF_FFFF_FFFF_FFFF` is
        // refused for being a `u64`, which on every target this compiles for is
        // the same 64 bits.
        //
        // The parser builds exactly this shape for a radix literal too wide for
        // the carrier (see its `Token::UInt` arm), so this is where the two ends
        // meet. Restricted to a negative carrier on purpose: without that,
        // `let a: u32 = 5 as u64` would start passing, and an explicit cast
        // should not be quietly re-typed.
        Expr::Cast(inner, Type::MachineInt(crate::val::IntKind::U64)) => match inner.as_ref() {
            Expr::Literal(LiteralVal::Int(value)) if *value < 0 => Some(i128::from(*value as u64)),
            _ => None,
        },
        _ => None,
    }
}

/// Does a declared return type accept the `nil` a fall-through path produces?
///
/// `Any` and a type variable do; so does anything optional, which is how a
/// function says "may answer nothing". Everything else is a promise.
fn declared_admits_nil(declared: &Type) -> bool {
    match declared {
        Type::Nil | Type::Any | Type::Optional(_) => true,
        Type::Union(members) => members.iter().any(declared_admits_nil),
        // An unresolved variable is inference still in progress, not a promise
        // this can hold anyone to.
        Type::Variable(_) => true,
        _ => false,
    }
}
