// The file-import resolver (fs/path) is std-gated; under no_std its cache field
// and `Path` import are legitimately unused (M0.7/8).
#[cfg_attr(not(feature = "std"), allow(dead_code, unused_imports))]
pub mod defer;
pub mod import;
mod stmt_impl;
pub mod stmt_parser;

#[cfg(test)]
mod attribute_test;
#[cfg(test)]
mod destructuring_test;
#[cfg(test)]
mod function_test;
#[cfg(test)]
mod if_let_test;
#[cfg(test)]
mod import_parse_test;
#[cfg(test)]
mod stmt_recover_test;
#[cfg(test)]
mod stmt_test;

pub use import::*;
pub use stmt_impl::*;
pub use stmt_parser::*;
