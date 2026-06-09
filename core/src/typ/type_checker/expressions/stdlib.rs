use crate::expr::Expr;
use crate::typ::type_checker::TypeChecker;
use crate::val::Type;

impl TypeChecker {
    pub(super) fn stdlib_call_function_type(&self, func: &Expr) -> Option<Type> {
        let Expr::Access(expr, field) = func else {
            return None;
        };
        self.stdlib_access_function_type(expr, field)
    }

    pub(super) fn stdlib_access_function_type(&self, expr: &Expr, field: &Expr) -> Option<Type> {
        let Expr::Var(module) = expr else {
            return None;
        };
        let field_name = match field {
            Expr::Literal(value) => value.as_str()?,
            Expr::Var(name) => name.as_str(),
            _ => return None,
        };
        let (params, return_type) = stdlib_function_signature(module, field_name)?;
        Some(Type::Function {
            params,
            named_params: Vec::new(),
            return_type: Box::new(return_type),
        })
    }
}

fn stdlib_function_signature(module: &str, field: &str) -> Option<(Vec<Type>, Type)> {
    let any = || Type::Any;
    let unary_any = || vec![Type::Any];
    let binary_any = || vec![Type::Any, Type::Any];

    match (module, field) {
        ("os", "arch" | "hostname" | "os") => Some((Vec::new(), Type::String)),
        ("os", "clock") => Some((Vec::new(), Type::Float)),
        ("os", "epoch" | "time") => Some((Vec::new(), Type::Int)),

        ("env", "get") => Some((unary_any(), Type::Any)),
        ("env", "get_or") => Some((binary_any(), Type::String)),
        ("env", "has") => Some((unary_any(), Type::Bool)),

        ("math", "abs" | "max" | "min") => Some((unary_any(), Type::Any)),
        ("math", "clamp") => Some((vec![any(), any(), any()], Type::Int)),
        ("math", "ceil" | "floor" | "round" | "to_int" | "trunc") => Some((unary_any(), Type::Int)),
        (
            "math",
            "acos" | "asin" | "atan" | "cbrt" | "cos" | "cosh" | "exp" | "fract" | "log" | "log10" | "log2" | "sin"
            | "sinh" | "sqrt" | "tan" | "tanh" | "to_float",
        ) => Some((unary_any(), Type::Float)),
        ("math", "atan2" | "hypot" | "pow") => Some((binary_any(), Type::Float)),
        ("math", "is_inf" | "is_nan") => Some((unary_any(), Type::Bool)),
        ("math", "random") => Some((Vec::new(), Type::Float)),

        _ => None,
    }
}
