use crate::expr::TemplateStringPart;
use crate::typ::type_checker::TypeChecker;
use crate::val::{LiteralVal, Type};
use anyhow::Result;

impl TypeChecker {
    pub(super) fn check_template_string(&mut self, parts: &[TemplateStringPart]) -> Result<Type> {
        for part in parts {
            if let TemplateStringPart::Expr(expr) = part {
                // Checked, and its type deliberately *not* used.
                //
                // Interpolation renders whatever it is given, so an operand
                // whose type is still an unresolved variable must not be pinned
                // to `String` by appearing here. It used to be, and the effect
                // reached a long way: in
                //
                //     fn h(p0) { m["k${p0}"] = 1; return p0; }
                //
                // the map's key type made the whole interpolation a `String`,
                // the constraint travelled back through it onto `p0`, and the
                // function was inferred to *return* a String — so an `Int`
                // caller was rejected for a program that is fine. A fuzz run on
                // a fresh seed produced it at case 651.
                //
                // String `+` still constrains, and that is a different question:
                // `+` is overloaded, so which one it is has to be decided.
                let _ = self.check_expr(expr)?;
            }
        }
        Ok(Type::String)
    }

    pub(super) fn check_literal(&mut self, val: &LiteralVal) -> Result<Type> {
        match val {
            LiteralVal::Nil => Ok(Type::Nil),
            LiteralVal::Bool(_) => Ok(Type::Bool),
            LiteralVal::Int(_) => Ok(Type::Int),
            LiteralVal::Float(_) => Ok(Type::Float),
            LiteralVal::ShortStr(_) => Ok(Type::String),
            value if value.as_str().is_some() => Ok(Type::String),
            LiteralVal::String(_) => Ok(Type::String),
        }
    }

    /// Infer type from a LiteralVal (for use in literal checking).
    pub(in crate::typ::type_checker) fn infer_val_type(&mut self, val: &LiteralVal) -> Result<Type> {
        self.check_literal(val)
    }
}
