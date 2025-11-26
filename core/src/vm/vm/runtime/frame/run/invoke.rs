use anyhow::Result;

use crate::val::{RustFunction, RustFunctionNamed, Val};
use crate::vm::context::VmContext;

#[inline]
pub(super) fn invoke_rust_function(ctx: &mut VmContext, func: RustFunction, args: &[Val]) -> Result<Val> {
    let saved_depth = ctx.call_stack_depth();
    let saved_generation = ctx.generation();
    match func(args, ctx) {
        Ok(val) => Ok(val),
        Err(err) => {
            ctx.truncate_call_stack(saved_depth);
            ctx.restore_generation(saved_generation);
            Err(err)
        }
    }
}

#[inline]
pub(super) fn invoke_rust_function_named(
    ctx: &mut VmContext,
    func: RustFunctionNamed,
    positional: &[Val],
    named: &[(String, Val)],
) -> Result<Val> {
    let saved_depth = ctx.call_stack_depth();
    let saved_generation = ctx.generation();
    match func(positional, named, ctx) {
        Ok(val) => Ok(val),
        Err(err) => {
            ctx.truncate_call_stack(saved_depth);
            ctx.restore_generation(saved_generation);
            Err(err)
        }
    }
}
