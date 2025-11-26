#![allow(
    clippy::collapsible_if,
    clippy::collapsible_else_if,
    clippy::get_first,
    clippy::useless_conversion
)]

mod ast;
mod display;
mod type_check;

pub use ast::{ForPattern, NamedParamDecl, Program, Stmt};
