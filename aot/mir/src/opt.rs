//! MIR-level optimization passes.
//!
//! These exist because the backend cannot do this work for us. The retired
//! string-IR path emitted LLVM `declare`s carrying function attributes, so
//! `opt` could CSE and hoist `Pure` runtime calls on its own; Cranelift has no
//! equivalent for calls to opaque external symbols. Redundancy elimination
//! therefore has to happen where the effect metadata lives — on MIR, using the
//! `aot/abi` schema as the source of truth.
//!
//! Being MIR-level is also the point architecturally: these passes are
//! independent of which backend consumes the MIR, which is exactly the
//! separation the redesign RFC's `codegen` boundary was drawn for.
//!
//! # Measured effect (do not overstate this)
//!
//! On the `examples/` corpus: **1085 dead instructions** removed and **5**
//! redundant `Pure` calls collapsed across 51 programs. The *runtime* effect
//! of the DCE half is nil — compiling `bench/workloads_business_algorithms.lk`
//! with and without this pass produces byte-identical executables (7530368 B)
//! and the same wall time, because Cranelift already eliminates dead pure
//! CLIF instructions itself.
//!
//! So DCE here buys readable MIR snapshots and a smaller codegen input, not
//! speed. CSE is the half that earns its place: Cranelift *cannot* do it,
//! since a call to an opaque `lkrt` symbol is a black box to it — only the
//! `aot/abi` effect schema knows the call is pure. The corpus just does not
//! happen to repeat many pure calls; user code that calls `s.len()` twice in
//! one expression does.
//!
//! Neither pass closes the ~17% gap to the retired clang `-O2` path. That gap
//! is in instruction selection and register allocation, not in redundancy.
//!
//! # Soundness rules
//!
//! Two distinct properties matter here, and conflating them is how an
//! optimizer miscompiles:
//!
//! - **CSE** needs "same arguments ⇒ same result, no observable effect in
//!   between". [`AbiEffect::Pure`] gives exactly that. A `Pure` call that
//!   *aborts* (`socket.addr` on a bad port) is still fine to collapse: the
//!   first call already aborted, so the second is unreachable.
//! - **DCE** needs "removing the call changes nothing observable". That is
//!   strictly stronger — an unused aborting call is observable by *not*
//!   happening. So this pass never removes a call, only pure data
//!   instructions, and never removes `Div`/`Mod` (divide-by-zero aborts) or a
//!   `Maybe` unwrap (absent aborts).
//!
//! # Why there is no LICM
//!
//! Hoisting loop-invariant `Pure` calls was the third pass considered here.
//! It is not implemented because the candidate set is empty: over all 51
//! `examples/` programs, [`count_licm_candidates`] reports **0** invariant
//! `Pure` calls inside a loop. The reason is structural rather than
//! incidental — the `Pure` calls the lowering emits inside loops (`dyn.from_*`
//! boxing, `str.char_len`, comparisons) take the loop variable as an argument,
//! so they are invariant by definition almost never.
//!
//! Hoisting would also need a stronger property than `Pure`: a hoisted call
//! runs even when the loop body does not, so a `Pure`-but-aborting callee
//! (`socket.addr` with a bad port) could abort a program that previously
//! completed. That would need a `total`/`no-abort` bit in the ABI schema.
//!
//! Re-measure with `LK_AOT_OPT_STATS=1` before building it; if the number is
//! still 0, the pass would be dead weight.

use std::collections::HashMap;

use crate::{Block, FloatBinOp, Inst, IntBinOp, MirFunction, MirModule, Term, ValueId, inst_def};

/// What a run of [`optimize`] changed. Returned for tests and for the
/// `LK_AOT_OPT_STATS` reporting hook; callers may ignore it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct OptStats {
    /// Redundant `Pure` calls collapsed into an earlier identical call.
    pub cse_calls: usize,
    /// Dead pure-data instructions removed.
    pub dce_insts: usize,
}

impl OptStats {
    /// Whether this run changed anything.
    pub fn is_empty(self) -> bool {
        self.cse_calls == 0 && self.dce_insts == 0
    }
}

/// Runs the MIR optimization pipeline over every function in `module`.
///
/// The result stays valid MIR: [`crate::validate`] should pass afterwards just
/// as it did before (the caller asserts exactly that).
pub fn optimize(module: &mut MirModule) -> OptStats {
    let mut stats = OptStats::default();
    for func in &mut module.functions {
        let cse = cse_pure_calls(func);
        let dce = eliminate_dead_insts(func);
        stats.cse_calls += cse;
        stats.dce_insts += dce;
    }
    stats
}

/// Block-local common-subexpression elimination over `Pure` ABI calls.
///
/// Scope is deliberately one block: a cross-block version needs dominance
/// information to be sound, and the redundancy the lowering actually produces
/// (repeated `dyn.from_*` boxing of the same value, repeated `str.char_len` in
/// one expression) is block-local anyway.
fn cse_pure_calls(func: &mut MirFunction) -> usize {
    // Rewrites apply function-wide even though the *candidate* table is
    // per-block: a collapsed value may well be read from a later block.
    let mut rewrite: HashMap<ValueId, ValueId> = HashMap::new();
    let mut collapsed = 0;

    for block in &mut func.blocks {
        let mut seen: HashMap<(&'static str, &'static str, Vec<ValueId>), ValueId> = HashMap::new();
        let mut keep = Vec::with_capacity(block.insts.len());
        for mut inst in std::mem::take(&mut block.insts) {
            // Resolve operands first so an earlier collapse is visible to this
            // instruction's identity.
            for use_ in uses_mut(&mut inst) {
                *use_ = resolve(&rewrite, *use_);
            }
            if let Inst::Call {
                dst: Some(dst),
                callee,
                args,
            } = &inst
                && lk_aot_abi::find(callee.module, callee.name).is_some_and(|f| f.effect == lk_aot_abi::AbiEffect::Pure)
            {
                let key = (callee.module, callee.name, args.clone());
                match seen.get(&key) {
                    Some(&existing) => {
                        rewrite.insert(*dst, existing);
                        collapsed += 1;
                        continue; // drop the redundant call
                    }
                    None => {
                        seen.insert(key, *dst);
                    }
                }
            }
            keep.push(inst);
        }
        block.insts = keep;
        for use_ in term_uses_mut(&mut block.term) {
            *use_ = resolve(&rewrite, *use_);
        }
    }
    collapsed
}

/// Follows a rewrite chain to its root (`a → b → c` resolves to `c`).
fn resolve(rewrite: &HashMap<ValueId, ValueId>, mut v: ValueId) -> ValueId {
    // The chain is short by construction (a collapsed value is never a CSE
    // table entry), but the bound keeps a malformed map from looping.
    for _ in 0..64 {
        match rewrite.get(&v) {
            Some(&next) if next != v => v = next,
            _ => return v,
        }
    }
    v
}

/// Removes pure data instructions whose result is never read.
///
/// Iterates to a fixpoint: removing one instruction can make its operands
/// dead in turn (a boxing chain collapses in one sweep per level).
fn eliminate_dead_insts(func: &mut MirFunction) -> usize {
    let mut removed = 0;
    loop {
        let live = live_values(&func.blocks);
        let mut round = 0;
        for block in &mut func.blocks {
            block.insts.retain(|inst| {
                let dead = inst_def(inst).is_some_and(|dst| !live.contains(&dst));
                if dead && is_removable(inst) {
                    round += 1;
                    false
                } else {
                    true
                }
            });
        }
        removed += round;
        if round == 0 {
            return removed;
        }
    }
}

/// Every value read by an instruction or terminator anywhere in the function.
fn live_values(blocks: &[Block]) -> std::collections::HashSet<ValueId> {
    let mut live = std::collections::HashSet::new();
    for block in blocks {
        for inst in &block.insts {
            let mut inst = inst.clone();
            for use_ in uses_mut(&mut inst) {
                live.insert(*use_);
            }
        }
        let mut term = block.term.clone();
        for use_ in term_uses_mut(&mut term) {
            live.insert(*use_);
        }
    }
    live
}

/// Whether an instruction with an unread result can be dropped outright.
///
/// The `false` arms are the load-bearing ones: every instruction that can
/// abort, print, call, or write state stays even when its result is dead,
/// because the *effect* is the point, not the value.
fn is_removable(inst: &Inst) -> bool {
    match inst {
        // Divide-by-zero aborts (matching the VM), so a dead division is still
        // a program-visible check.
        Inst::IntBin { op, .. } => !matches!(op, IntBinOp::Div | IntBinOp::Mod),
        Inst::FloatBin { op, .. } => !matches!(op, FloatBinOp::Div | FloatBinOp::Mod),
        Inst::Const { .. }
        | Inst::Cmp { .. }
        | Inst::IntToFloat { .. }
        | Inst::ZextBool { .. }
        | Inst::Not { .. }
        | Inst::BoolAnd { .. }
        | Inst::MaybePresent { .. }
        | Inst::MaybeValue { .. }
        | Inst::MaybeWrap { .. }
        | Inst::Select { .. }
        // Container reads with `Maybe` semantics never abort (a missing
        // element is `present = 0`), so a dead read is genuinely dead.
        | Inst::ListGetMaybe { .. }
        | Inst::ListGetMaybeF64 { .. }
        | Inst::ListGetMaybeStr { .. }
        | Inst::MapGetMaybe { .. }
        | Inst::MapGetMaybeI64Key { .. }
        | Inst::MapGetMaybeStrF64 { .. }
        | Inst::MapGetMaybeI64F64 { .. }
        | Inst::GlobalGet { .. } => true,
        // A `Maybe` unwrap aborts when the element was absent — dropping it
        // would turn the VM's halt into a silent continue.
        Inst::UnwrapMaybeI64 { .. } | Inst::UnwrapMaybeF64 { .. } | Inst::UnwrapMaybeStr { .. } => false,
        // Calls stay even when `Pure`: an unused aborting call (`socket.addr`
        // with a bad port) is observable precisely by aborting.
        Inst::Call { .. }
        | Inst::CallFn { .. }
        | Inst::TryCall { .. }
        | Inst::TraitDispatch { .. }
        | Inst::CallVm { .. }
        | Inst::PrintStr { .. }
        | Inst::GlobalSet { .. } => false,
    }
}

/// Mutable access to every value an instruction *reads* (never its `dst`).
fn uses_mut(inst: &mut Inst) -> Vec<&mut ValueId> {
    match inst {
        Inst::Const { .. } | Inst::GlobalGet { .. } => vec![],
        Inst::IntBin { lhs, rhs, .. }
        | Inst::FloatBin { lhs, rhs, .. }
        | Inst::Cmp { lhs, rhs, .. }
        | Inst::BoolAnd { lhs, rhs, .. } => vec![lhs, rhs],
        Inst::IntToFloat { src, .. }
        | Inst::ZextBool { src, .. }
        | Inst::Not { src, .. }
        | Inst::MaybePresent { src, .. }
        | Inst::MaybeValue { src, .. }
        | Inst::MaybeWrap { src, .. }
        | Inst::UnwrapMaybeI64 { src, .. }
        | Inst::UnwrapMaybeF64 { src, .. }
        | Inst::UnwrapMaybeStr { src, .. }
        | Inst::GlobalSet { src, .. } => vec![src],
        Inst::Call { args, .. }
        | Inst::CallFn { args, .. }
        | Inst::TryCall { args, .. }
        | Inst::CallVm { args, .. } => args.iter_mut().collect(),
        Inst::TraitDispatch { self_arg, .. } => vec![self_arg],
        Inst::ListGetMaybe { handle, index, .. }
        | Inst::ListGetMaybeF64 { handle, index, .. }
        | Inst::ListGetMaybeStr { handle, index, .. } => vec![handle, index],
        Inst::MapGetMaybe { handle, key, .. }
        | Inst::MapGetMaybeI64Key { handle, key, .. }
        | Inst::MapGetMaybeStrF64 { handle, key, .. }
        | Inst::MapGetMaybeI64F64 { handle, key, .. } => vec![handle, key],
        Inst::PrintStr { value, .. } => vec![value],
        Inst::Select {
            cond, then_v, else_v, ..
        } => vec![cond, then_v, else_v],
    }
}

/// Mutable access to every value a terminator reads (including block args).
fn term_uses_mut(term: &mut Term) -> Vec<&mut ValueId> {
    match term {
        Term::Ret(Some(v)) => vec![v],
        Term::Ret(None) | Term::Abort => vec![],
        Term::Br { args, .. } => args.iter_mut().collect(),
        Term::CondBr {
            cond,
            then_args,
            else_args,
            ..
        } => {
            let mut out = vec![cond];
            out.extend(then_args.iter_mut());
            out.extend(else_args.iter_mut());
            out
        }
    }
}

#[cfg(test)]
mod tests;

/// Counts loop-invariant `Pure` calls — the candidate set a LICM pass would
/// hoist. Approximate on purpose (loop body ≈ the block-id span between a
/// back-edge target and its source, which the lowering's leader-ordered
/// blocks make sound in practice): it exists to answer "is there anything to
/// hoist at all", not to drive a transformation.
#[doc(hidden)]
pub fn count_licm_candidates(module: &MirModule) -> usize {
    let mut candidates = 0;
    for func in &module.functions {
        // Back edges: a branch whose target block id does not exceed its own.
        let mut loops: Vec<(usize, usize)> = Vec::new();
        for (bi, block) in func.blocks.iter().enumerate() {
            let mut targets = Vec::new();
            match &block.term {
                Term::Br { target, .. } => targets.push(*target),
                Term::CondBr { then_blk, else_blk, .. } => {
                    targets.push(*then_blk);
                    targets.push(*else_blk);
                }
                _ => {}
            }
            for target in targets {
                if let Some(hi) = func.blocks.iter().position(|b| b.id == target)
                    && hi <= bi
                {
                    loops.push((hi, bi));
                }
            }
        }
        for (header, latch) in loops {
            // Values defined inside the loop body are not invariant.
            let mut inside = std::collections::HashSet::new();
            for block in &func.blocks[header..=latch] {
                inside.extend(block.params.iter().map(|(v, _)| *v));
                for inst in &block.insts {
                    if let Some(dst) = inst_def(inst) {
                        inside.insert(dst);
                    }
                }
            }
            for block in &func.blocks[header..=latch] {
                for inst in &block.insts {
                    let Inst::Call {
                        dst: Some(_),
                        callee,
                        args,
                    } = inst
                    else {
                        continue;
                    };
                    if !lk_aot_abi::find(callee.module, callee.name)
                        .is_some_and(|f| f.effect == lk_aot_abi::AbiEffect::Pure)
                    {
                        continue;
                    }
                    if args.iter().all(|a| !inside.contains(a)) {
                        candidates += 1;
                    }
                }
            }
        }
    }
    candidates
}
