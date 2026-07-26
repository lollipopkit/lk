//! Non-terminator control-flow opcodes (terminators live in `function.rs`).

use super::LowerCtx;
use crate::*;

pub(super) fn lower(
    ctx: &mut LowerCtx<'_>,
    _block: usize,
    insts: &mut Vec<Inst>,
    instr: &Instr,
    pc: usize,
) -> Result<(), Unsupported> {
    let ssa = &mut *ctx.ssa;
    let globals = &mut *ctx.globals;
    let func = ctx.func;
    match instr.opcode() {
        Opcode::Raise => {
            // `bx` = the raised message string constant. The raise unwinds to
            // the nearest native `try` frame (`try$call` — plan G); with no
            // handler it aborts, exactly the VM's uncaught raise (the
            // differential harness treats VM exit-1 and a native SIGABRT as
            // matching failures).
            let message = func
                .consts
                .strings
                .get(instr.bx() as usize)
                .ok_or(Unsupported::BadConst { pc })?
                .clone();
            let msg = materialize_key(ssa, insts, globals, &message);
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("rt", "raise_msg"),
                args: vec![msg],
            });
        }
        op => return Err(Unsupported::Opcode { pc, op }),
    }
    Ok(())
}
