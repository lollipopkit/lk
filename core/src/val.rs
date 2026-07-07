pub mod de;

mod runtime_model;

#[cfg(test)]
mod de_test;
#[cfg(test)]
mod val_test;

// Front-end value/type model (LiteralVal/Type/ShortStr/numeric) lives in the L0
// `lk-values` crate; re-exported here so `crate::val::Type` etc. are unchanged.
pub use lk_values::{
    FunctionNamedParamType, LiteralVal, NumericClass, NumericHierarchy, ShortStr, ShortStrOrStr, Type,
};
pub use runtime_model::*;
