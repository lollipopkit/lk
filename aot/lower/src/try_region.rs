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
//!
//! # Nothing is rejected here any more, and what it took
//!
//! The three `try`/`catch` files that sat in `AOT_COVERAGE_ALLOW` all lower
//! natively now, and each came off by a different fix. The reason to write that
//! down is the two that did *not* work, because both looked right:
//!
//! - **A placeholder on the edge that has no definition.** In the VM that
//!   register holds whatever it held before the region, which is a real value a
//!   program could read.
//!
//! - **Reading a container back out of a cell as the register's own type.** The
//!   type is not a guess — it comes from the SSA, and the type checker refuses
//!   `let a = 0; try { a = "s"; }` — and it makes `try_catch.lk` compile. It
//!   also makes it print `Assertion failed` where the VM prints `try/catch: ok`.
//!   A container does not need a cell: the parent and the body hold the *same
//!   handle*, so a mutation is already visible, and the `dyn.from_list` /
//!   `dyn.as_list` round trip is what loses it.
//!
//! What did work, in order:
//!
//! 1. **Cells are discovered, not predicted.** A region carries nothing back to
//!    begin with; every register its body rebound and did not carry back is
//!    poisoned at the region's exit, and a later read of one fails naming
//!    itself. That error is how the fixpoint already finds a cell, so the set
//!    ends up containing exactly the registers something reads. `Ssa::poisoned`
//!    exists because `current_def = None` is not an absence — the read falls
//!    through to the predecessors and finds the stale value.
//!
//! 2. **A body may be handed a container.** The trampoline marshals inputs as
//!    machine words, and a handle *is* a machine word; declaring them all `I64`
//!    rejected a body that merely looked at a list the parent owned.
//!
//!    Same for a **`Bool`** (2026-07-30): 0/1 is a machine word too, and its
//!    absence from `crosses_as_word` meant a `try` inside *any* function taking
//!    a bool dropped the module to the VM — while the identical function with an
//!    `Int` parameter lowered. That is what the note on file as "the `try`
//!    expression's value cannot lower" actually was.
//!
//!    **`F64` needed one more step.** The trampoline's signature is all
//!    `long long` (`lkrt/src/try_trampoline.c`), so a float arrives in an
//!    *integer* register. Adding `F64` to the list and declaring the body's
//!    parameter `F64` made Cranelift read a float register instead: it compiled
//!    and **segfaulted**. The body now declares the parameter `I64` and reads
//!    the float back out of those bits (`Inst::BitsToFloat`) before its first
//!    instruction. The differential case does arithmetic on it, because a
//!    bit-cast in the wrong direction still runs and answers *something*.
//!
//! 3. **What a body rebound is reported by the body.** Reading the `a` field as
//!    "the register this instruction writes" is not true of every opcode —
//!    `log.push(2)` is `ListPush a=log`, where `a` is the receiver. The body
//!    already compares the SSA's `current_def` before and after each
//!    instruction to notice a cell changing; widening that to every register
//!    answers the question without a table of operand roles.
//!
//! 4. **A retriable discovery made in the final pass has somewhere to go.** The
//!    fixpoint converges, `refine_signatures` runs once, and the final pass
//!    lowers against refined signatures — where a function that was clean every
//!    pass can fail, with nothing after it to retry.
//!
//! 5. **An already-boxed value comes back as itself.** `Dyn` needs no unboxing:
//!    what the cell holds *is* the register's value. Unlike (the refuted)
//!    container case, nothing is reinterpreted.
//!
//! The through-line: every one of these replaced an inference about what a
//! value *must be* with a question put to the SSA — and the two that were
//! refuted were the two that inferred. Compiling was never the test; agreeing
//! with the VM was, and the refutations were found by running the program.
//!
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
    /// The body `return`s from the **enclosing** function.
    ///
    /// Outlined, such a `return` would return from the body instead — a
    /// different program — so it used to be a rejection. It is a third outcome
    /// now: two more output cells (a flag and the value), set by the body and
    /// checked by the caller on the ok edge.
    pub(crate) body_returns: bool,
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
/// `performance` is carried over **rebased**, and only the two tables this
/// pipeline actually reads: `for_loops` (`cfg::exit_of`) and `key_ops`
/// (`inst::container`). Both are keyed by pc, and the body's pcs are the
/// parent's shifted by `body_start`, so the rebase is a slice — a fact read at
/// the wrong pc is worse than a missing one, and slicing cannot produce one.
///
/// Dropping them wholesale is what it used to do, and the cost was concrete: a
/// `for` loop *requires* its fact, so a region with an ordinary `for i in 0..n`
/// in it rejected. That was the single most common blocker left in a generated
/// corpus of `try` programs. Neither fact names a pc — `PerfForLoopFact`'s jump
/// is an offset — so nothing inside them needs adjusting.
///
/// The rest of the tables stay default. They are the VM executor's, and an
/// outlined body is never executed by the VM: it exists only in this crate's
/// own function table.
pub(crate) fn outline(parent: &FunctionData, region: &TryRegionShape) -> FunctionData {
    let mut code: Vec<u32> = parent.code[region.body_start..region.body_end].to_vec();
    code.push(Instr::abc(Opcode::Return0, 0, 0, 0).raw());
    fn rebase<T: Clone>(table: &[Option<T>], start: usize, end: usize) -> Vec<Option<T>> {
        (start..end).map(|pc| table.get(pc).cloned().flatten()).collect()
    }
    let (start, end) = (region.body_start, region.body_end);
    let performance = lk_core::vm::analysis::PerformanceFacts {
        for_loops: rebase(&parent.performance.for_loops, start, end),
        key_ops: rebase(&parent.performance.key_ops, start, end),
        ..Default::default()
    };
    FunctionData {
        consts: parent.consts.clone(),
        code,
        performance,
        register_count: parent.register_count,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        // Named even when the parent is not — the top-level entry has no name,
        // so its outlined bodies had none either and a blocker in one arrived
        // bare. `try@12` is not a made-up id like `fn41`: the pc is where the
        // region begins, which is the one thing a reader can look up.
        debug_name: Some(match parent.debug_name.as_ref() {
            Some(name) => format!("{name}$try{}", region.begin_pc),
            None => format!("try@{}", region.begin_pc),
        }),
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
    let mut regions: Vec<TryRegionShape> = Vec::new();
    // Only this function's *own* regions. A `try` written inside another one's
    // body belongs to the body — which becomes a function of its own, scanned
    // in turn — so outlining it here as well would give one `TryBegin` two
    // owners and consume the same instructions twice.
    let mut inner_until = 0usize;
    for (pc, instr) in instrs.iter().enumerate() {
        if instr.opcode() != Opcode::TryBegin || pc < inner_until {
            continue;
        }
        let shape = shape_at(func, instrs, pc)?;
        inner_until = shape.body_end;
        regions.push(shape);
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
    // fall through at all.
    //
    // A body that *always* returns has none, and then there is no ok edge to
    // give the region: `after_end` is the handler itself, and the `TryEnd`
    // block has no successor for the CFG to record. The return channel
    // (`body_returns`) answers a body that returns on *some* path, which is the
    // general shape; this degenerate one — the whole body is a `return` — stays
    // a rejection, because the fix for it is a region with no ok edge rather
    // than one more cell.
    let after_end = body_end + 1;
    let has_fallthrough = after_end < code_len && instrs[after_end].opcode() == Opcode::Jmp;
    let fallthrough = if has_fallthrough {
        crate::cfg::rel(after_end, instrs[after_end].sj_arg(), code_len)
            .ok_or(Unsupported::BadTarget { pc: after_end })?
    } else {
        after_end
    };

    let mut body_returns = false;
    for instr in instrs.iter().take(body_end).skip(begin_pc + 1) {
        let op = instr.opcode();
        // A `return` inside the body returns from the *enclosing* function —
        // recorded, and answered by the return channel (`body_returns`).
        if matches!(op, Opcode::Return | Opcode::Return0 | Opcode::Return1) {
            body_returns = true;
        }
    }

    // Jumps must stay inside the body. The shape that reaches here is a
    // `break` or `continue` whose loop *encloses* the `try`: outlined, the body
    // is a function of its own and has no loop to leave.
    //
    // `return` is not in this set — it is answered by the return channel
    // (`body_returns`) — and a loop written *inside* the `try` is not either,
    // since its jumps stay in the body. Naming the shape rather than the
    // mechanism matters here because the message is what a reader gets from
    // `lk compile object:`, where there is no fallback to hide it; the
    // rearrangement that lowers is in the sentence.
    let mut consumed = vec![false; code_len];
    for pc in begin_pc + 1..body_end {
        let exit = crate::cfg::exit_of(pc, instrs, code_len, &mut consumed, &func.performance)?;
        for target in crate::cfg::exit_successors(exit, pc + 1) {
            if target <= begin_pc || target > body_end {
                return Err(Unsupported::TryRegion {
                    pc,
                    reason: "a `break` or `continue` here belongs to a loop outside the `try`, and the body \
                             becomes a function of its own with no loop to leave — writing \
                             the loop inside the `try` lowers",
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
        body_returns,
    })
}
