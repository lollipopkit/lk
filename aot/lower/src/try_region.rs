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
pub(crate) struct TryRegionShape {
    /// The `TryBegin` itself: what a rejection names, and where the parent's
    /// block ends.
    pub(crate) begin_pc: usize,
    /// The body, `[start, end)` — everything between `TryBegin` and `TryEnd`.
    pub(crate) body_start: usize,
    pub(crate) body_end: usize,
    /// Where the handler begins, and where control resumes after the region.
    pub(crate) handler: usize,
    pub(crate) fallthrough: usize,
    /// The register the handler reads the caught value from.
    pub(crate) catch_reg: u8,
}

/// Every register the body might write.
///
/// Over-approximated on purpose: it is `a` for every instruction in the body,
/// whether or not that opcode writes a register at all. `a` is the destination
/// by convention throughout this instruction set, so this misses nothing; what
/// it adds are registers an instruction only *read*, and the cost of that is a
/// region rejected that could have been lowered. The cost of the opposite
/// mistake is a program that computes a different answer, which is why the
/// approximation goes this way.
pub(crate) fn written_registers(instrs: &[Instr], start: usize, end: usize) -> Vec<u8> {
    let mut written: Vec<u8> = instrs[start..end].iter().map(|instr| instr.a()).collect();
    written.sort_unstable();
    written.dedup();
    written
}

/// Builds the function a region's body becomes.
///
/// The body's instructions verbatim, with a `Return0` appended: it produces no
/// value, and the only thing the caller wants back is whether it finished.
/// Register numbering is left alone — the body uses the enclosing function's
/// registers, so the synthesized function simply declares as many.
///
/// `performance` is *not* carried over. Those facts are keyed by pc in the
/// parent, and the body's pcs are rebased here; a fact read at the wrong pc is
/// worse than a missing one. The shapes that need a fact (a `for` loop, which
/// requires one) therefore reject rather than lower wrongly.
pub(crate) fn outline(parent: &FunctionData, region: &TryRegionShape) -> FunctionData {
    let mut code: Vec<u32> = parent.code[region.body_start..region.body_end].to_vec();
    code.push(Instr::abc(Opcode::Return0, 0, 0, 0).raw());
    FunctionData {
        consts: parent.consts.clone(),
        code,
        performance: Default::default(),
        register_count: parent.register_count,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        debug_name: parent
            .debug_name
            .as_ref()
            .map(|name| format!("{name}$try{}", region.begin_pc)),
        export_name: None,
        extern_name: None,
    }
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

    // `TryBegin catch_reg, →handler`, the offset relative to the next pc.
    let begin = instrs[begin_pc];
    let handler =
        crate::cfg::rel(begin_pc, i32::from(begin.sbx()), code_len).ok_or(Unsupported::BadTarget { pc: begin_pc })?;
    // What follows `TryEnd` is the jump over the handler, when the body can
    // fall through at all. A body that always returns has none — but such a
    // body is rejected below, so this is the ordinary case.
    let after_end = body_end + 1;
    let fallthrough = if after_end < code_len && instrs[after_end].opcode() == Opcode::Jmp {
        crate::cfg::rel(after_end, instrs[after_end].sj_arg(), code_len)
            .ok_or(Unsupported::BadTarget { pc: after_end })?
    } else {
        after_end
    };

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

    Ok(TryRegionShape {
        begin_pc,
        body_start: begin_pc + 1,
        body_end,
        handler,
        fallthrough,
        catch_reg: begin.a(),
    })
}
