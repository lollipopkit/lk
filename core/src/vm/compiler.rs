mod builder;
mod const_eval;
mod driver;
mod expr;
mod free_vars;
mod ssa;
mod stmt;

pub(crate) use builder::FunctionBuilder;
pub use driver::{Compiler, compile_program};
pub use ssa::{
    BlockId, ParamId, SsaBlock, SsaFunction, SsaLoweringError, SsaRvalue, SsaStatement, SsaTerminator, ValueId,
    lower_expr_to_ssa,
};

#[cfg(test)]
mod tests;
