//! Protected regions (`try { … } catch e { … }`), turned into a call.
//!
//! Cranelift cannot emit `setjmp`. A call that returns twice has no place in
//! its SSA or its register allocator, so the shape the VM uses — open a
//! handler, run the body *here*, and longjmp back into the middle of this
//! function — cannot be lowered as written. What can be lowered is a call: the
//! body becomes a function of its own, `lkrt`'s trampoline does the `setjmp`
//! in a C frame that outlives it, and the caller branches on whether the body
//! returned or raised.
//!
//! So this module answers one question: *can this region be outlined, and if
//! so, into what function?* The rejections are as important as the acceptances
//! — each one is a shape whose meaning would change if it were called instead
//! of inlined, and every one of them names itself rather than falling out as
//! "opcode TryBegin is not natively lowerable yet".

use lk_core::vm::{FunctionData, Instr, Opcode};

use crate::Unsupported;

/// A `try` region found in a function's bytecode.
///
/// Only the `TryBegin`'s own pc for now: it is what a rejection has to name.
/// The body's bounds, the handler and the caught register are what *outlining*
/// needs, and they arrive with it — carrying them before then would be fields
/// nothing reads.
pub(crate) struct TryRegionShape {
    pub(crate) begin_pc: usize,
}

/// Finds every `try` region in a function, in `TryBegin` order.
///
/// Returns `Err` for a region that cannot be outlined, because falling back
/// silently is what made this feature invisible for so long: a program with a
/// `try` in it simply ran three times slower, with nothing said.
pub(crate) fn scan(func: &FunctionData, instrs: &[Instr]) -> Result<Vec<TryRegionShape>, Unsupported> {
    let mut regions = Vec::new();
    for (pc, instr) in instrs.iter().enumerate() {
        if instr.opcode() != Opcode::TryBegin {
            continue;
        }
        regions.push(shape_at(func, instrs, pc)?);
    }
    Ok(regions)
}

fn shape_at(func: &FunctionData, instrs: &[Instr], begin_pc: usize) -> Result<TryRegionShape, Unsupported> {
    let code_len = instrs.len();

    // The body runs from the next instruction to the matching `TryEnd`.
    // Nested regions are counted rather than assumed away: an inner `try`
    // inside the body has its own `TryEnd`, and taking the first one would cut
    // the outer body in half.
    let mut depth = 0usize;
    let mut body_end = None;
    for (pc, instr) in instrs.iter().enumerate().skip(begin_pc + 1) {
        match instr.opcode() {
            Opcode::TryBegin => depth += 1,
            Opcode::TryEnd => {
                if depth == 0 {
                    body_end = Some(pc);
                    break;
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    let body_end = body_end.ok_or(Unsupported::TryRegion {
        pc: begin_pc,
        reason: "no matching TryEnd",
    })?;

    for (pc, instr) in instrs.iter().enumerate().take(body_end).skip(begin_pc + 1) {
        let op = instr.opcode();
        // A `return` inside the body returns from the *enclosing* function.
        // Outlined, it would return from the body instead — a different
        // program. Propagating it needs the call to carry "and then return",
        // which is a protocol this does not have yet.
        if matches!(op, Opcode::Return | Opcode::Return0 | Opcode::Return1) {
            return Err(Unsupported::TryRegion {
                pc,
                reason: "the body returns from the enclosing function",
            });
        }
        // A nested region would need its own outlining pass inside the
        // synthesized function. That is the same work again, and it is the
        // next round's.
        if matches!(op, Opcode::TryBegin) {
            return Err(Unsupported::TryRegion {
                pc,
                reason: "a try inside a try",
            });
        }
    }

    // Jumps must stay inside the body: a `break` out of a loop that encloses
    // the `try` leaves the region, and an outlined body has nowhere to leave
    // to.
    let mut consumed = vec![false; code_len];
    for pc in begin_pc + 1..body_end {
        let exit = crate::cfg::exit_of(pc, instrs, code_len, &mut consumed, &func.performance)?;
        for target in crate::cfg::exit_successors(exit, pc + 1) {
            if target <= begin_pc || target > body_end {
                return Err(Unsupported::TryRegion {
                    pc,
                    reason: "the body jumps out of the region",
                });
            }
        }
    }

    Ok(TryRegionShape { begin_pc })
}
