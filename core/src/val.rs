pub mod de;
pub mod position;
pub mod ser;

mod runtime_model;

#[cfg(test)]
mod de_test;
#[cfg(test)]
mod val_test;

// Front-end value/type model (LiteralVal/Type/ShortStr/numeric) lives in the L0
// `lk-values` crate; re-exported here so `crate::val::Type` etc. are unchanged.
pub use lk_values::{
    CONTAINER_TYPE_NAMES, FunctionNamedParamType, IntKind, LiteralVal, NUMBER_TYPE_NAME, NumericClass,
    NumericHierarchy, PRIMITIVE_TYPES, ShortStr, ShortStrOrStr, TYPE_SPELLINGS, Type,
};
pub use runtime_model::*;
