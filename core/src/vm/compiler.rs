mod builder;
mod const_eval;
mod driver;
mod expr;
mod expr_call;
mod expr_list;
mod expr_map;
mod expr_select;
mod free_vars;
mod map_facts;
mod param_infer;
mod peephole;
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
