use crate::val::Type;

/// Coarse-grained numeric hierarchy used by the type system and compiler.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NumericClass {
    Int,
    Float,
    Boxed,
}

pub struct NumericHierarchy;

impl NumericHierarchy {
    /// Classify a type into the numeric hierarchy if possible.
    pub fn classify(ty: &Type) -> Option<NumericClass> {
        match ty {
            Type::Int => Some(NumericClass::Int),
            Type::Float => Some(NumericClass::Float),
            Type::Any => Some(NumericClass::Boxed),
            Type::Boxed(inner) => NumericHierarchy::classify(inner).or(Some(NumericClass::Boxed)),
            Type::Union(items) => {
                let mut result: Option<NumericClass> = None;
                for item in items {
                    if matches!(item, Type::Nil) {
                        continue;
                    }
                    if let Some(class) = NumericHierarchy::classify(item) {
                        result = Some(result.map(|r| r.max(class)).unwrap_or(class));
                    } else {
                        return None;
                    }
                }
                result
            }
            Type::Optional(inner) => NumericHierarchy::classify(inner),
            _ => None,
        }
    }

    /// Combine two numeric classes and return the resulting class after promotion.
    pub fn result(lhs: NumericClass, rhs: NumericClass) -> NumericClass {
        lhs.max(rhs)
    }

    /// Convert a numeric class back to a type.
    pub fn to_type(class: NumericClass) -> Type {
        match class {
            NumericClass::Int => Type::Int,
            NumericClass::Float => Type::Float,
            NumericClass::Boxed => Type::Boxed(Box::new(Type::Any)),
        }
    }

    /// Canonical "expected numeric type" hint for diagnostics.
    pub fn expected_type() -> Type {
        Type::Union(vec![Type::Int, Type::Float, Type::Boxed(Box::new(Type::Any))])
    }
}
