use crate::compat::collections::HashSet;
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::expr::Expr;
use crate::typ::type_checker::TypeChecker;
use crate::val::{FunctionNamedParamType, Type};
use anyhow::Result;

impl TypeChecker {
    pub(super) fn check_stdlib_function_call(&mut self, func: &Expr, args: &[Box<Expr>]) -> Result<Option<Type>> {
        let Some(path) = access_segments(func) else {
            return Ok(None);
        };
        let Some((module, field)) = canonical_stdlib_path(&path) else {
            return Ok(None);
        };

        if module == "math" && field == "clamp" {
            self.check_math_clamp_args(args, &[])?;
            return Ok(Some(Type::Int));
        }

        if let Some(declared) = crate::typ::stdlib_signature(&format!("{module}.{field}")) {
            let required = declared.required_params();
            if args.len() < required || args.len() > declared.params.len() {
                return Err(Self::type_err(
                    &format!("Function expects {}", describe_arity(required, declared.params.len())),
                    None,
                    None,
                    Some(func.clone()),
                ));
            }
            for (param, arg) in declared.params.iter().zip(args.iter()) {
                let arg_type = self.check_expr(arg)?;
                // An optional parameter is left unconstrained: the declaration
                // says what it accepts when present, not that the argument in
                // that position *is* one — several exports let a named
                // parameter be passed positionally too.
                if !param.optional && param.ty != Type::Any {
                    self.inference_engine.add_constraint(param.ty.clone(), arg_type);
                }
            }
            return Ok(Some(declared.return_type));
        }

        let Some((params, named_params, return_type)) = stdlib_function_signature(&module, &field) else {
            return Ok(None);
        };
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
        Ok(Some(return_type))
    }

    pub(super) fn check_stdlib_named_function_call(
        &mut self,
        callee: &Expr,
        pos_args: &[Box<Expr>],
        named_args: &[(String, Box<Expr>)],
    ) -> Result<Option<Type>> {
        let Some(path) = access_segments(callee) else {
            return Ok(None);
        };
        let Some((module, field)) = canonical_stdlib_path(&path) else {
            return Ok(None);
        };

        if module == "math" && field == "clamp" {
            self.check_math_clamp_args(pos_args, named_args)?;
            return Ok(Some(Type::Int));
        }

        Ok(None)
    }

    pub(super) fn stdlib_access_function_type(&self, expr: &Expr, field: &Expr) -> Option<Type> {
        let mut path = access_segments(expr)?;
        path.push(segment_name(field)?);
        let (module, field) = canonical_stdlib_path(&path)?;
        self.stdlib_function_type(&module, &field)
    }

    fn stdlib_function_type(&self, module: &str, field: &str) -> Option<Type> {
        if let Some(declared) = crate::typ::stdlib_signature(&format!("{module}.{field}")) {
            return Some(Type::Function {
                params: declared
                    .params
                    .iter()
                    .filter(|param| !param.named)
                    .map(|param| param.ty.clone())
                    .collect(),
                named_params: declared.named_params(),
                return_type: Box::new(declared.return_type),
            });
        }
        let (params, named_params, return_type) = stdlib_function_signature(module, field)?;
        Some(Type::Function {
            params,
            named_params,
            return_type: Box::new(return_type),
        })
    }

    fn check_math_clamp_args(&mut self, pos_args: &[Box<Expr>], named_args: &[(String, Box<Expr>)]) -> Result<()> {
        if pos_args.is_empty() || pos_args.len() > 3 {
            return Err(Self::type_err(
                "clamp() expects 1..3 positional arguments",
                None,
                None,
                None,
            ));
        }
        for arg in pos_args {
            let arg_type = self.check_expr(arg)?;
            self.inference_engine.add_constraint(Type::Int, arg_type);
        }

        let mut seen = HashSet::with_capacity(named_args.len());
        for (name, expr) in named_args {
            if name != "min" && name != "max" {
                return Err(Self::type_err(
                    &format!("Unknown named argument: {}", name),
                    None,
                    None,
                    Some(expr.as_ref().clone()),
                ));
            }
            if !seen.insert(name.as_str()) {
                return Err(Self::type_err(
                    &format!("Duplicate named argument: {}", name),
                    None,
                    None,
                    Some(expr.as_ref().clone()),
                ));
            }
            let arg_type = self.check_expr(expr)?;
            self.inference_engine.add_constraint(Type::Int, arg_type);
        }
        Ok(())
    }
}

fn describe_arity(required: usize, max: usize) -> String {
    if required == max {
        format!("exactly {required} argument{}", if required == 1 { "" } else { "s" })
    } else {
        format!("{required} to {max} arguments")
    }
}

/// What the checker knows about the standard library when no standard library
/// is linked in: `core`'s own tests, and targets that ship a different module
/// set (`stdlib/bare`, `stdlib/web`).
///
/// The declarations in `stdlib/crates` are the real source — they cover all 23
/// modules and reach the checker through `register_stdlib_signatures`, which
/// takes priority over everything here. This table only has to keep `core`
/// standing on its own.
fn stdlib_function_signature(module: &str, field: &str) -> Option<(Vec<Type>, Vec<FunctionNamedParamType>, Type)> {
    let any = || Type::Any;
    let unary_any = || vec![Type::Any];
    let binary_any = || vec![Type::Any, Type::Any];
    let no_named = || Vec::new();

    match (module, field) {
        ("os", "arch" | "hostname" | "os") => Some((Vec::new(), no_named(), Type::String)),
        ("os", "clock") => Some((Vec::new(), no_named(), Type::Float)),
        ("os", "epoch" | "time") => Some((Vec::new(), no_named(), Type::Int)),

        ("env", "get") => Some((unary_any(), no_named(), Type::Any)),
        ("env", "get_or") => Some((binary_any(), no_named(), Type::String)),
        ("env", "has") => Some((unary_any(), no_named(), Type::Bool)),

        ("math", "abs") => Some((unary_any(), no_named(), Type::Any)),
        ("math", "max" | "min") => Some((binary_any(), no_named(), Type::Any)),
        ("math", "clamp") => Some((
            vec![any()],
            vec![
                FunctionNamedParamType {
                    name: "min".to_string(),
                    ty: Type::Optional(Box::new(Type::Int)),
                    has_default: true,
                },
                FunctionNamedParamType {
                    name: "max".to_string(),
                    ty: Type::Optional(Box::new(Type::Int)),
                    has_default: true,
                },
            ],
            Type::Int,
        )),
        ("math", "ceil" | "floor" | "round" | "to_int" | "trunc") => Some((unary_any(), no_named(), Type::Int)),
        (
            "math",
            "acos" | "asin" | "atan" | "cbrt" | "cos" | "cosh" | "exp" | "fract" | "log" | "log10" | "log2" | "sin"
            | "sinh" | "sqrt" | "tan" | "tanh" | "to_float",
        ) => Some((unary_any(), no_named(), Type::Float)),
        ("math", "atan2" | "hypot" | "pow") => Some((binary_any(), no_named(), Type::Float)),
        ("math", "is_inf" | "is_nan") => Some((unary_any(), no_named(), Type::Bool)),
        ("math", "random") => Some((Vec::new(), no_named(), Type::Float)),

        _ => None,
    }
}

fn canonical_stdlib_path(path: &[&str]) -> Option<(String, String)> {
    match path {
        ["os", "env", field] => Some(("env".to_string(), (*field).to_string())),
        [.., field] if path.len() >= 2 => Some((path[..path.len() - 1].join("."), (*field).to_string())),
        _ => None,
    }
}

fn access_segments(expr: &Expr) -> Option<Vec<&str>> {
    match expr {
        Expr::Var(name) => Some(vec![name.as_str()]),
        Expr::Access(base, field) => {
            let mut path = access_segments(base)?;
            path.push(segment_name(field)?);
            Some(path)
        }
        _ => None,
    }
}

fn segment_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Var(name) => Some(name.as_str()),
        Expr::Literal(value) => value.as_str(),
        _ => None,
    }
}
