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
mod rust_function_test;
#[cfg(test)]
mod stmt_recover_test;
#[cfg(test)]
mod stmt_test;
#[cfg(test)]
mod while_let_test;

pub use import::*;
pub use stmt_impl::*;
pub use stmt_parser::*;

#[cfg(test)]
pub mod test_support {
    use super::*;
    use crate::{
        val::Val,
        vm::{Vm, VmContext, compile_program},
    };
    use anyhow::Result;

    pub fn run_program(program: &Program, ctx: &mut VmContext) -> Result<Val> {
        let function = compile_program(program);
        let mut vm = Vm::new();
        vm.exec_with(&function, ctx, None)
    }

    pub fn run_program_default(program: &Program) -> Result<Val> {
        let mut ctx = VmContext::new();
        run_program(program, &mut ctx)
    }
}

#[cfg(test)]
pub use test_support::*;
