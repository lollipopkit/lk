mod type_checker;
mod type_system;

#[cfg(test)]
mod type_system_test;

// NumericClass/NumericHierarchy live with `Type` in `crate::val`; re-exported
// here so `crate::typ::Numeric*` call sites stay stable. Breaks the val -> typ
// dependency (a step toward extracting values into an L0 crate).
pub use crate::val::{NumericClass, NumericHierarchy};
pub use type_checker::*;
pub use type_system::*;
