pub mod de;
pub mod position;
pub mod ser;

mod runtime_model;
// A value's type identity — the declaring module plus the name — is a property
// of the value, not of the executor. It lived under `vm/` and was the reason
// `val` named `vm` for anything other than a callable payload.
mod type_info;

#[cfg(test)]
mod de_test;
#[cfg(test)]
mod val_test;

// Front-end value/type model (LiteralVal/Type/ShortStr/numeric) lives in the L0
// `lk-values` crate; re-exported here so `crate::val::Type` etc. are unchanged.
pub use lk_values::{
    CONTAINER_TYPE_NAMES, FunctionNamedParamType, IntKind, LiteralVal, NUMBER_TYPE_NAME, NoTraits, NumericClass,
    NumericHierarchy, PRIMITIVE_TYPES, ShortStr, ShortStrOrStr, TYPE_SPELLINGS, TraitOracle, Type,
};
pub use runtime_model::*;
pub use type_info::*;
