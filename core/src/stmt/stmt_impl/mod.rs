#![allow(
    clippy::collapsible_if,
    clippy::collapsible_else_if,
    clippy::get_first,
    clippy::useless_conversion
)]

mod ast;
mod display;
mod flow;
mod type_check;

pub use ast::{Attribute, ForPattern, NamedParamDecl, Program, Stmt};
