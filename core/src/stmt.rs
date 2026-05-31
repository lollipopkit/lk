pub mod import;
mod stmt_impl;
pub mod stmt_parser;

#[cfg(test)]
mod destructuring_test;
#[cfg(test)]
mod function_test;
#[cfg(test)]
mod if_let_test;
#[cfg(test)]
mod stmt_recover_test;
#[cfg(test)]
mod stmt_test;

pub use import::*;
pub use stmt_impl::*;
pub use stmt_parser::*;

#[cfg(test)]
pub use test_support::*;

#[cfg(test)]
pub mod test_support {
    use super::*;
    use crate::vm::{Program32Result, VmContext};
    use anyhow::Result;

    pub fn run_program(program: &Program, ctx: &mut VmContext) -> Result<Program32Result> {
        program.execute32_with_ctx(ctx)
    }

    pub fn run_program_default(program: &Program) -> Result<Program32Result> {
        let mut ctx = VmContext::new();
        run_program(program, &mut ctx)
    }
}
