/// Cross-file signatures. `std` only: it reads the imported file, and a target
/// without a filesystem has no file imports to resolve.
#[cfg(feature = "std")]
mod imports;
mod type_checker;
mod type_system;

#[cfg(test)]
mod function_infer_test;
#[cfg(test)]
mod or_pattern_binding_test;
#[cfg(test)]
mod type_checker_test;
#[cfg(test)]
mod type_system_test;

// NumericClass/NumericHierarchy live with `Type` in `crate::val`; re-exported
// here so `crate::typ::Numeric*` call sites stay stable. Breaks the val -> typ
// dependency (a step toward extracting values into an L0 crate).
pub use crate::val::{NumericClass, NumericHierarchy};
#[cfg(feature = "std")]
pub use imports::seed_imported_signatures;
pub use type_checker::*;
pub use type_system::*;
