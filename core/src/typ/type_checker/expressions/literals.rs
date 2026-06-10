use crate::expr::TemplateStringPart;
use crate::typ::type_checker::TypeChecker;
use crate::val::{LiteralVal, Type};
use anyhow::Result;

impl TypeChecker {
    pub(super) fn check_template_string(&mut self, parts: &[TemplateStringPart]) -> Result<Type> {
        for part in parts {
            if let TemplateStringPart::Expr(expr) = part {
                let expr_type = self.check_expr(expr)?;
                self.coerce_to_string(&expr_type);
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
