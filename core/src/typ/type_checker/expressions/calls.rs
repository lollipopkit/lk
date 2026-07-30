#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::expr::Expr;
use crate::typ::type_checker::TypeChecker;
use crate::typ::type_checker::expressions::bind_instance_variables;
use crate::val::Type;
use anyhow::Result;
use hashbrown::HashMap;

impl TypeChecker {
    /// Check function call type
    pub(super) fn check_function_call(&mut self, func: &Expr, args: &[Box<Expr>]) -> Result<Type> {
        if let Some(return_type) = self.check_stdlib_function_call(func, args)? {
            return Ok(return_type);
        }

        // `m.f(..)` where `m` is a namespace bound by an import. Checked before
        // the method path below, which asks what methods the *receiver's type*
        // has — a namespace is not a value with methods, and its own type says
        // nothing about what it exports.
        if let Expr::Access(base, field) = func
            && let Expr::Var(namespace) = base.as_ref()
            && let Some(member) = super::stdlib::segment_name(field)
            && let Some(Type::Function {
                params,
                named_params: _,
                return_type,
            }) = self.imported_member_type(namespace, member)
        {
            if params.len() != args.len() {
                return Err(Self::type_err(
                    &format!("Function expects {} arguments", params.len()),
                    None,
                    None,
                    Some(func.clone()),
                ));
            }
            for (index, (param_type, arg)) in params.iter().zip(args.iter()).enumerate() {
                let arg_type = self.check_expr_against(arg, Some(param_type))?;
                self.check_argument(param_type, &arg_type, index, arg)?;
            }
            return Ok(*return_type);
        }

        if let Expr::Access(obj_expr, field_expr) = func {
            let receiver_ty = self.check_expr(obj_expr)?;
            if let Expr::Literal(field_val) = field_expr.as_ref()
                && let Some(name) = field_val.as_str()
            {
                if let Some(Type::Function {
                    params,
                    named_params,
                    return_type,
                }) = self.get_method_sig(&receiver_ty, name.as_ref())
                {
                    if params.is_empty() {
                        return Err(Self::type_err(
                            "Method signature missing receiver parameter",
                            None,
                            None,
                            Some(func.clone()),
                        ));
                    }
                    let mut params_iter = params.into_iter();
                    let self_param = params_iter.next().unwrap();
                    self.inference_engine.add_constraint(self_param, receiver_ty.clone());

                    let remaining_params: Vec<Type> = params_iter.collect();
                    if remaining_params.len() != args.len() {
                        return Err(Self::type_err(
                            &format!("Method expects {} arguments", remaining_params.len()),
                            None,
                            None,
                            None,
                        ));
                    }
                    for (index, (param_type, arg)) in remaining_params.iter().zip(args.iter()).enumerate() {
                        let arg_type = self.check_expr_against(arg, Some(param_type))?;
                        // +1: the receiver occupies position 0 of the
                        // signature, so the caller's first argument is the
                        // second parameter.
                        self.check_argument(param_type, &arg_type, index + 1, arg)?;
                    }
                    for decl in named_params {
                        let is_optional = matches!(decl.ty, Type::Optional(_)) || decl.has_default;
                        if !is_optional {
                            return Err(Self::type_err(
                                &format!("Missing required named argument: {}", decl.name),
                                None,
                                None,
                                None,
                            ));
                        }
                    }
                    return Ok(*return_type);
                } else if let Some(return_type) =
                    self.check_declared_builtin_method(&receiver_ty, name.as_ref(), args)?
                {
                    return Ok(return_type);
                } else if let Some(return_type) =
                    self.check_builtin_container_method(&receiver_ty, name.as_ref(), args, func)?
                {
                    return Ok(return_type);
                } else {
                    for arg in args {
                        self.check_expr(arg)?;
                    }
                    return Ok(Type::Any);
                }
            }
        }

        if let Expr::Var(name) = func {
            match name.as_str() {
                "Set" => {
                    if args.len() > 1 {
                        return Err(Self::type_err("Set() expects 0 or 1 argument", None, None, None));
                    }
                    let Some(arg) = args.first() else {
                        return Ok(Type::Set(Box::new(Type::Any)));
                    };
                    let arg_ty = self.check_expr(arg)?;
                    return match self.resolve_aliases(&arg_ty) {
                        Type::List(elem) => Ok(Type::Set(elem)),
                        Type::Set(elem) => Ok(Type::Set(elem)),
                        Type::Any | Type::Variable(_) => Ok(Type::Set(Box::new(Type::Any))),
                        other => Err(Self::type_err(
                            "Set(value) expects List or Set",
                            Some(Type::List(Box::new(Type::Any))),
                            Some(other),
                            Some(arg.as_ref().clone()),
                        )),
                    };
                }
                "chan" => {
                    if args.is_empty() || args.len() > 2 {
                        return Err(Self::type_err("chan() expects 1 or 2 arguments", None, None, None));
                    }
                    let capacity_ty = self.check_expr(&args[0])?;
                    self.enforce_int_type(&args[0], capacity_ty, "chan capacity")?;
                    if args.len() == 2 {
                        let type_arg_ty = self.check_expr(&args[1])?;
                        if self.resolve_aliases(&type_arg_ty) != Type::String {
                            return Err(Self::type_err(
                                "chan() type hint must be String when provided",
                                Some(Type::String),
                                Some(type_arg_ty),
                                Some(args[1].as_ref().clone()),
                            ));
                        }
                    }
                    return Ok(Type::Channel(Box::new(Type::Any)));
                }
                "send" => {
                    if args.len() != 2 {
                        return Err(Self::type_err("send() expects 2 arguments", None, None, None));
                    }
                    let channel_ty = self.check_expr(&args[0])?;
                    let value_ty = self.check_expr(&args[1])?;
                    match self.resolve_aliases(&channel_ty) {
                        Type::Channel(inner) => {
                            self.inference_engine.add_constraint((*inner).clone(), value_ty);
                            return Ok(Type::Nil);
                        }
                        // Un-inferred (e.g. a plain fn parameter): constrain
                        // to Channel instead of rejecting.
                        Type::Any | Type::Variable(_) => {
                            self.inference_engine
                                .add_constraint(channel_ty, Type::Channel(Box::new(Type::Any)));
                            return Ok(Type::Nil);
                        }
                        other => {
                            return Err(Self::type_err(
                                "send() pattern requires a channel",
                                Some(Type::Channel(Box::new(Type::Any))),
                                Some(other),
                                Some(args[0].as_ref().clone()),
                            ));
                        }
                    }
                }
                "recv" => {
                    if args.len() != 1 {
                        return Err(Self::type_err("recv() expects exactly 1 argument", None, None, None));
                    }
                    let channel_ty = self.check_expr(&args[0])?;
                    return match self.resolve_aliases(&channel_ty) {
                        Type::Channel(inner) => Ok((*inner).clone()),
                        // Same latitude as `send` for un-inferred operands.
                        Type::Any | Type::Variable(_) => {
                            self.inference_engine
                                .add_constraint(channel_ty, Type::Channel(Box::new(Type::Any)));
                            Ok(Type::Any)
                        }
                        other => Err(Self::type_err(
                            "recv() pattern requires a channel",
                            Some(Type::Channel(Box::new(Type::Any))),
                            Some(other),
                            Some(args[0].as_ref().clone()),
                        )),
                    };
                }
                "spawn" => {
                    if args.len() != 1 {
                        return Err(Self::type_err("spawn() expects exactly 1 argument", None, None, None));
                    }
                    let callable_ty = self.check_expr(&args[0])?;
                    match self.resolve_aliases(&callable_ty) {
                        Type::Function { .. } => {}
                        Type::Any | Type::Variable(_) => {
                            let expected = Type::Function {
                                params: Vec::new(),
                                named_params: Vec::new(),
                                return_type: Box::new(Type::Any),
                            };
                            self.inference_engine.add_constraint(callable_ty, expected);
                        }
                        other => {
                            return Err(Self::type_err(
                                "spawn() expects a function or closure",
                                None,
                                Some(other),
                                Some(args[0].as_ref().clone()),
                            ));
                        }
                    }
                    return Ok(Type::Task(Box::new(Type::Any)));
                }
                _ => {}
            }
        }

        let func_type = self.check_expr(func)?;
        let resolved = self.resolve_aliases(&func_type);

        if let Some((params, named_params, return_type)) = match resolved.clone() {
            Type::Function {
                params,
                named_params,
                return_type,
            } => Some((params, named_params, return_type)),
            Type::Optional(inner) => match *inner {
                Type::Function {
                    params,
                    named_params,
                    return_type,
                } => Some((params, named_params, return_type)),
                _ => None,
            },
            _ => None,
        } {
            let total_count = params.len() + named_params.len();
            if args.len() < params.len() || args.len() > total_count {
                return Err(Self::type_err(
                    &format!("Function expects {}..{} arguments", params.len(), total_count),
                    None,
                    None,
                    None,
                ));
            }

            // Only the positions the callee *annotated* are checked. An
            // unannotated parameter also has a type by now — inference gave it
            // one from the body — but that is a derivation rather than a
            // claim, and rejecting against it rejects on something the program
            // never said.
            let annotated = match func {
                Expr::Var(name) => self.get_function_sig(name).map(|sig| sig.annotated.clone()),
                _ => None,
            };
            // This call's own answer for the callee's type variables.
            //
            // `fn id(x) { return x; }` has the principal type `'a -> 'a` once
            // its own constraints are solved, and every call site constrains
            // that same `'a` — so `id(1)` and `id("s")` in one program fight
            // over it, and what the solver makes of the disagreement is a type
            // nothing can be checked against. That is why `let s: String =
            // id(1);` passed.
            //
            // The map below is *local to this call*: it reads the variables off
            // the arguments and substitutes them into the return type, which is
            // instantiation in effect. Doing it by renaming the signature into
            // fresh variables instead would also work, and would additionally
            // stop the call sites from constraining the original — but that is
            // what makes `id` resolve to a concrete type at all, and without it
            // the strict-Any check reads a generic function as an unannotated
            // one and demands annotations for it. Keeping the constraints where
            // they were leaves that judgement exactly as it was.
            //
            // The map is also why the constraint alone is not enough: checking
            // is one pass, and the solver does not run again until the
            // enclosing function ends — long after the `let` that reads this
            // call has been checked.
            let mut bindings: HashMap<String, Type> = HashMap::new();
            for (index, (param_type, arg)) in params.iter().zip(args.iter()).enumerate() {
                let arg_type = self.check_expr_against(arg, Some(param_type))?;
                bind_instance_variables(param_type, &self.resolve_aliases(&arg_type), &mut bindings);
                let declared = annotated
                    .as_ref()
                    .is_some_and(|mask| mask.get(index).copied().unwrap_or(false));
                if declared {
                    self.check_argument(param_type, &arg_type, index, arg)?;
                } else {
                    self.inference_engine.add_constraint(param_type.clone(), arg_type);
                }
            }

            let supplied_named = args.len() - params.len();
            for (index, decl) in named_params.iter().enumerate() {
                if index < supplied_named {
                    let arg = &args[params.len() + index];
                    let arg_type = self.check_expr_against(arg, Some(&decl.ty))?;
                    // A named parameter's declaration always carries a type or
                    // a default, so unlike a positional one there is nothing
                    // inferred to mistake for a claim.
                    self.check_named_argument(&decl.name, &decl.ty, &arg_type, arg)?;
                    continue;
                }
                let is_optional = matches!(decl.ty, Type::Optional(_)) || decl.has_default;
                if !is_optional {
                    return Err(Self::type_err(
                        &format!("Missing required named argument: {}", decl.name),
                        None,
                        None,
                        None,
                    ));
                }
            }

            return Ok(super::substitute_outside_unions(&return_type, &bindings));
        }

        match resolved {
            Type::Any | Type::Variable(_) => {
                for arg in args {
                    self.check_expr(arg)?;
                }
                Ok(Type::Any)
            }
            Type::Union(variants) => {
                let mut saw_function = false;
                for variant in variants {
                    match variant {
                        Type::Function { .. } => {
                            saw_function = true;
                            break;
                        }
                        Type::Optional(inner) if matches!(*inner, Type::Function { .. }) => {
                            saw_function = true;
                            break;
                        }
                        _ => {}
                    }
                }
                if saw_function {
                    for arg in args {
                        self.check_expr(arg)?;
                    }
                    Ok(Type::Any)
                } else {
                    Err(Self::type_err(
                        "Cannot call non-function type",
                        None,
                        Some(func_type),
                        None,
                    ))
                }
            }
            _ => Err(Self::type_err(
                "Cannot call non-function type",
                None,
                Some(func_type),
                None,
            )),
        }
    }
}

impl TypeChecker {
    /// Check a call against the built-in method's declared signature
    /// (`typ::builtin_method_sig`), if it has one.
    ///
    /// `Ok(None)` means the table says nothing — an unknown method, or a
    /// receiver that is not (yet) a known container — and the caller falls
    /// through to the hand-written arms and then to `Any`. A receiver still
    /// typed as a variable lands here, which is deliberate: constraining a call
    /// on it would decide its type from the method name.
    fn check_declared_builtin_method(
        &mut self,
        receiver_ty: &Type,
        method: &str,
        args: &[Box<Expr>],
    ) -> Result<Option<Type>> {
        let resolved_receiver = self.resolve_aliases(receiver_ty);
        let Some(sig) = crate::typ::builtin_method_signature(&resolved_receiver, method) else {
            // A *known* container with no such method is an error, not a
            // shrug. User and trait methods were already resolved above, so
            // nothing else can answer this call — the VM will say "List has no
            // method 'clear'" when it runs, and there is no reason to wait.
            // (`xs.clear()` type-checked for exactly that long.)
            //
            // Except on a map, where `m.f(x)` need not be a method at all: a
            // map's entries *are* its fields, so `m.score` may hold a function
            // and calling it is an ordinary property call. Nothing in the map's
            // type says which keys it has, so there is no such thing here as a
            // name it cannot answer.
            if let Some(kind) = crate::typ::receiver_kind(&resolved_receiver)
                && kind != crate::typ::BuiltinReceiverKind::Map
            {
                return Err(Self::type_err(
                    &format!("{} has no method '{method}'", receiver_kind_name(kind)),
                    None,
                    Some(resolved_receiver),
                    None,
                ));
            }
            return Ok(None);
        };
        if args.len() < sig.required || args.len() > sig.params.len() {
            let expected = if sig.required == sig.params.len() {
                format!("{}", sig.params.len())
            } else {
                format!("{} to {}", sig.required, sig.params.len())
            };
            return Err(Self::type_err(
                &format!("Method {method} expects {expected} argument(s), got {}", args.len()),
                None,
                None,
                None,
            ));
        }
        let mut callback_result: Option<Type> = None;
        for (index, ((_, param_type), arg)) in sig.params.iter().zip(args.iter()).enumerate() {
            // A callback applied to each element: its first parameter *is* the
            // element type. Handed to the closure before its body is read, so
            // the body is checked against it — `["a"].map(|s| s.bogus())` is a
            // missing method rather than an unknown one.
            let arg_type = match (sig.elementwise_callback == Some(index), arg.as_ref()) {
                (
                    true,
                    Expr::Closure {
                        params,
                        param_types,
                        return_type,
                        body,
                    },
                ) => self.check_closure(
                    params,
                    param_types,
                    return_type.as_deref(),
                    body,
                    core::slice::from_ref(&sig.elem),
                )?,
                _ => self.check_expr(arg)?,
            };
            if sig.elementwise_callback == Some(index)
                && let Type::Function {
                    params, return_type, ..
                } = &self.resolve_aliases(&arg_type)
            {
                if let Some(first) = params.first() {
                    self.inference_engine.add_constraint(first.clone(), sig.elem.clone());
                }
                callback_result = Some(self.resolve_aliases(return_type));
            }
            self.check_argument(param_type, &arg_type, index, arg)?;
        }
        // `map`'s element type is the callback's return type, instantiated
        // here rather than declared in the table — the table cannot name it,
        // and `List<Any>` is what it said until a call site could.
        if callback_result.is_some()
            && let Some(instantiated) =
                crate::typ::builtin_method_signature_with(&resolved_receiver, method, callback_result)
        {
            return Ok(Some(instantiated.return_type));
        }
        Ok(Some(sig.return_type))
    }

    /// Checks one positional argument against the parameter it fills.
    ///
    /// A *concrete* parameter type is checked; an unannotated one (a fresh
    /// type variable, or `Any`) keeps the old behaviour of feeding inference,
    /// because there is nothing to check it against. That distinction is the
    /// whole design: annotate a parameter and the calls to it are checked,
    /// leave it off and they are not.
    fn check_argument(&mut self, param_type: &Type, arg_type: &Type, index: usize, arg: &Expr) -> Result<()> {
        // An argument whose own type is still unresolved says nothing to check
        // against either: feeding inference is the only sound thing to do with
        // it.
        if matches!(self.resolve_aliases(arg_type), Type::Variable(_)) {
            self.inference_engine
                .add_constraint(param_type.clone(), arg_type.clone());
            return Ok(());
        }
        if !self.is_concrete_parameter(param_type) {
            self.inference_engine
                .add_constraint(param_type.clone(), arg_type.clone());
            return Ok(());
        }
        if self.is_assignable(arg_type, param_type) || literal_fits_machine_int(param_type, arg) {
            return Ok(());
        }
        Err(Self::type_err(
            &format!("Argument {} has the wrong type", index + 1),
            Some(param_type.clone()),
            Some(arg_type.clone()),
            Some(arg.clone()),
        ))
    }

    /// [`check_argument`] for a named parameter, which is identified by name
    /// rather than position.
    fn check_named_argument(&mut self, name: &str, param_type: &Type, arg_type: &Type, arg: &Expr) -> Result<()> {
        if matches!(self.resolve_aliases(arg_type), Type::Variable(_)) {
            self.inference_engine
                .add_constraint(param_type.clone(), arg_type.clone());
            return Ok(());
        }
        if !self.is_concrete_parameter(param_type) {
            self.inference_engine
                .add_constraint(param_type.clone(), arg_type.clone());
            return Ok(());
        }
        if self.is_assignable(arg_type, param_type) || literal_fits_machine_int(param_type, arg) {
            return Ok(());
        }
        Err(Self::type_err(
            &format!("Named argument '{name}' has the wrong type"),
            Some(param_type.clone()),
            Some(arg_type.clone()),
            Some(arg.clone()),
        ))
    }

    /// Whether a parameter's type says enough to check an argument against.
    ///
    /// `Any` and type variables do not: the first accepts everything by
    /// definition, and the second is what an unannotated parameter gets, so
    /// rejecting against it would reject on an invented type.
    /// Whether a parameter's type is settled enough to *check* an argument
    /// against, rather than to learn from it.
    ///
    /// "Contains no variable", not "is not a variable". The two differ exactly
    /// where a generic method takes a container: `xs.chain(ys)` has parameter
    /// `List<'T>`, which is not a variable and was therefore checked — so
    /// passing a `List<Int>` reported "expected List<'T0>, got List<Int>"
    /// instead of binding `'T0` to `Int`. `xs.push(y)` took the other path and
    /// worked, because *its* parameter is the bare `'T`.
    ///
    /// What that cost is visible in `bare-metal-x86/program.lk`, which had to
    /// write `let line = [0]; line = [];` — build a list with a placeholder
    /// element so the element type is known, then throw it away — because
    /// `let line = []; line = line.chain(…)` did not type-check.
    fn is_concrete_parameter(&self, param_type: &Type) -> bool {
        let resolved = self.resolve_aliases(param_type);
        !matches!(resolved, Type::Any) && !resolved.contains_variables()
    }
}

/// Whether `arg` is an integer *literal* that fits a machine-integer
/// parameter.
///
/// Machine integers do not convert implicitly — that is the rule that makes
/// `u8 + Int` an error rather than a silent widening — but a literal has no
/// type of its own to preserve. `f(0x3f8)` for `fn f(port: u16)` is the
/// ordinary way to call a driver, and requiring `0x3f8 as u16` there would be
/// ceremony without a reader.
fn literal_fits_machine_int(param_type: &Type, arg: &Expr) -> bool {
    let Type::MachineInt(kind) = param_type else {
        return false;
    };
    let Expr::Literal(crate::val::LiteralVal::Int(value)) = arg else {
        return false;
    };
    kind.accepts_literal(i128::from(*value))
}

/// How a receiver kind is named in a diagnostic — the same words the VM uses
/// when the call reaches it.
fn receiver_kind_name(kind: crate::typ::BuiltinReceiverKind) -> &'static str {
    use crate::typ::BuiltinReceiverKind::*;
    match kind {
        List => "List",
        Bytes => "Bytes",
        Slice => "Slice",
        Map => "Map",
        Set => "Set",
        Str => "String",
    }
}
