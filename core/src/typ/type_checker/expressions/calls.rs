#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::expr::Expr;
use crate::typ::type_checker::TypeChecker;
use crate::val::Type;
use anyhow::Result;

impl TypeChecker {
    /// Check function call type
    pub(super) fn check_function_call(&mut self, func: &Expr, args: &[Box<Expr>]) -> Result<Type> {
        if let Some(return_type) = self.check_stdlib_function_call(func, args)? {
            return Ok(return_type);
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
                        let arg_type = self.check_expr(arg)?;
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
            for (index, (param_type, arg)) in params.iter().zip(args.iter()).enumerate() {
                let arg_type = self.check_expr(arg)?;
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
                    let arg_type = self.check_expr(arg)?;
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

            return Ok(*return_type);
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
    fn is_concrete_parameter(&self, param_type: &Type) -> bool {
        !matches!(self.resolve_aliases(param_type), Type::Any | Type::Variable(_))
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
