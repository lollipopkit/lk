mod numeric;
mod type_checker;
mod type_system;

#[cfg(test)]
mod type_system_test;

pub use numeric::{NumericClass, NumericHierarchy};
pub use type_checker::*;
pub use type_system::*;
