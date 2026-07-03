// The file-import resolver (fs/path) is std-gated; under no_std its cache field
// and `Path` import are legitimately unused (M0.7/8).
#[cfg_attr(not(feature = "std"), allow(dead_code, unused_imports))]
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

#[cfg(test)]
pub use test_support::*;

#[cfg(test)]
pub mod test_support {
    use super::*;
    use crate::vm::{ProgramResult, VmContext};
    use anyhow::Result;

    pub fn run_program(program: &Program, ctx: &mut VmContext) -> Result<ProgramResult> {
        program.execute_with_ctx(ctx)
    }

    pub fn run_program_default(program: &Program) -> Result<ProgramResult> {
        let mut ctx = VmContext::new();
        run_program(program, &mut ctx)
    }
}
