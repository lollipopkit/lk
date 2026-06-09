use crate::expr::Expr;
use crate::typ::type_checker::TypeChecker;
use crate::val::Type;
use anyhow::Result;

impl TypeChecker {
    /// Check function call type
    pub(super) fn check_function_call(&mut self, func: &Expr, args: &[Box<Expr>]) -> Result<Type> {
        if let Some(Type::Function {
            params,
            named_params,
            return_type,
        }) = self.stdlib_call_function_type(func)
        {
            if !named_params.is_empty() || params.len() != args.len() {
                return Err(Self::type_err(
                    &format!("Function expects {} arguments", params.len()),
                    None,
                    None,
                    Some(func.clone()),
                ));
            }
            for (param_type, arg) in params.iter().zip(args.iter()) {
                let arg_type = self.check_expr(arg)?;
                self.inference_engine.add_constraint(param_type.clone(), arg_type);
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
                    for (param_type, arg) in remaining_params.iter().zip(args.iter()) {
                        let arg_type = self.check_expr(arg)?;
                        self.inference_engine.add_constraint(param_type.clone(), arg_type);
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

            for (param_type, arg) in params.iter().zip(args.iter()) {
                let arg_type = self.check_expr(arg)?;
                self.inference_engine.add_constraint(param_type.clone(), arg_type);
            }

            let supplied_named = args.len() - params.len();
            for (index, decl) in named_params.iter().enumerate() {
                if index < supplied_named {
                    let arg_type = self.check_expr(&args[params.len() + index])?;
                    self.inference_engine.add_constraint(decl.ty.clone(), arg_type);
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
