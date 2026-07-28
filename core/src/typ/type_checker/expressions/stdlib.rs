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
                if param.optional || param.ty == Type::Any {
                    continue;
                }
                // Checked here rather than handed to the solver. `unify` ends in
                // a rule that accepts any two concrete types that disagree, on
                // the grounds that an *inferred* type may legitimately differ
                // between call sites in a gradually-typed language. That reason
                // does not reach this call: the parameter's type was not
                // inferred, it was declared by whoever wrote the export. Left to
                // the solver, `string.len(5)` passed.
                if !arg_type.contains_variables() && !self.is_assignable(&arg_type, &param.ty) {
                    // The types themselves go in `expected`/`actual`, which
                    // `TypeError`'s Display already renders.
                    return Err(Self::type_err(
                        &format!("Argument '{}' of {module}.{field}", param.name),
                        Some(param.ty.clone()),
                        Some(arg_type),
                        Some(arg.as_ref().clone()),
                    ));
                }
                self.inference_engine.add_constraint(param.ty.clone(), arg_type);
            }
            return Ok(Some(declared.return_type));
        }

        let Some((params, named_params, return_type)) = stdlib_function_signature(&module, &field) else {
            return Ok(None);
        };
        // A parameter this table lists as *named* may still be passed
        // positionally — that is what the export wrapper does at runtime — so
        // the accepted count is a range, not a number. This table is only
        // reached when no signature has been registered, which is the case in
        // `lk-core`'s own tests: the stdlib crate is not linked there.
        if args.len() < params.len() || args.len() > params.len() + named_params.len() {
            return Err(Self::type_err(
                &format!(
                    "Function expects {}",
                    describe_arity(params.len(), params.len() + named_params.len())
                ),
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

        let Some(declared) = crate::typ::stdlib_signature(&format!("{module}.{field}")) else {
            return Ok(None);
        };

        // The rules are the export wrapper's, which is what actually runs: a
        // parameter listed in `named(...)` may be given positionally *or* by
        // name, never both. `math.clamp` used to be the only function checked
        // this way — by a hand-written rule naming it — and every other export
        // fell through to the generic `Type::Function` path, whose parameter
        // list has the named-eligible ones *removed*. So a call that mixed the
        // two spellings, `bytes.slice(b, 0, end: 2)`, was rejected as taking
        // "1 positional arguments" while `bytes.slice(b, 0, 2)` was fine.
        if pos_args.len() > declared.params.len() {
            return Err(Self::type_err(
                &format!(
                    "Function expects {}",
                    describe_arity(declared.required_params(), declared.params.len())
                ),
                None,
                None,
                Some(callee.clone()),
            ));
        }

        let mut filled = vec![false; declared.params.len()];
        for (index, arg) in pos_args.iter().enumerate() {
            filled[index] = true;
            let arg_type = self.check_expr(arg)?;
            self.constrain_stdlib_argument(&declared.params[index], arg_type, arg, &module, &field)?;
        }

        let mut seen: HashSet<&str> = HashSet::with_capacity(named_args.len());
        for (name, expr) in named_args {
            let Some(index) = declared
                .params
                .iter()
                .position(|param| param.named && param.name == *name)
            else {
                return Err(Self::type_err(
                    &format!("Unknown named argument: {}", name),
                    None,
                    None,
                    Some(expr.as_ref().clone()),
                ));
            };
            if !seen.insert(name.as_str()) {
                return Err(Self::type_err(
                    &format!("Duplicate named argument: {}", name),
                    None,
                    None,
                    Some(expr.as_ref().clone()),
                ));
            }
            if filled[index] {
                return Err(Self::type_err(
                    &format!("Argument '{name}' given both positionally and by name"),
                    None,
                    None,
                    Some(expr.as_ref().clone()),
                ));
            }
            filled[index] = true;
            let arg_type = self.check_expr(expr)?;
            self.constrain_stdlib_argument(&declared.params[index], arg_type, expr, &module, &field)?;
        }

        for (param, filled) in declared.params.iter().zip(filled.iter()) {
            if !filled && !param.optional && !param.has_default {
                return Err(Self::type_err(
                    &format!("Missing required named argument: {}", param.name),
                    None,
                    None,
                    Some(callee.clone()),
                ));
            }
        }

        Ok(Some(declared.return_type))
    }

    /// One argument against one declared parameter.
    ///
    /// Shared by the positional and the named paths so that naming an argument
    /// cannot type-check differently from passing it in that position.
    fn constrain_stdlib_argument(
        &mut self,
        param: &crate::typ::ResolvedStdlibParam,
        arg_type: Type,
        arg: &Expr,
        module: &str,
        field: &str,
    ) -> Result<()> {
        // An optional parameter is left unconstrained: the declaration says
        // what it accepts when present, not that the argument *is* one.
        if param.optional || param.ty == Type::Any {
            return Ok(());
        }
        // Checked here rather than handed to the solver — see the positional
        // path for why `unify` is too permissive for a *declared* type.
        if !arg_type.contains_variables() && !self.is_assignable(&arg_type, &param.ty) {
            return Err(Self::type_err(
                &format!("Argument '{}' of {module}.{field}", param.name),
                Some(param.ty.clone()),
                Some(arg_type),
                Some(arg.clone()),
            ));
        }
        self.inference_engine.add_constraint(param.ty.clone(), arg_type);
        Ok(())
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

pub(super) fn segment_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Var(name) => Some(name.as_str()),
        Expr::Literal(value) => value.as_str(),
        _ => None,
    }
}
