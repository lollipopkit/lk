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
//! On the `examples/` corpus: **~1085 dead instructions** removed, **33**
//! redundant `Pure` calls collapsed, and **3** loop-local container handles
//! released per iteration, across 51 programs.
//!
//! The *runtime* effect of the DCE half is nil — compiling
//! `bench/workloads_business_algorithms.lk` with and without this pass
//! produces byte-identical executables (7530368 B) and the same wall time,
//! because Cranelift already eliminates dead pure CLIF instructions itself.
//! DCE here buys readable MIR snapshots and a smaller codegen input, not
//! speed.
//!
//! CSE is the half Cranelift *cannot* do: a call to an opaque `lkrt` symbol
//! is a black box to it, and only the `aot/abi` effect schema knows the call
//! is pure. Scoping it by dominance rather than by block is what makes it
//! worth having — 5 collapses are block-local, 33 with dominance.
//!
//! Scope drop is not an optimization at all but a correctness-of-resources
//! fix; see [`scope_drop_block_locals`].
//!
//! None of this closes the ~17% gap to the retired clang `-O2` path. That gap
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

use crate::{AbiRef, Block, BlockId, FloatBinOp, Inst, IntBinOp, MirFunction, MirModule, Term, ValueId, inst_def};

/// What a run of [`optimize`] changed. Returned for tests and for the
/// `LK_AOT_OPT_STATS` reporting hook; callers may ignore it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct OptStats {
    /// Redundant `Pure` calls collapsed into an earlier identical call.
    pub cse_calls: usize,
    /// Dead pure-data instructions removed.
    pub dce_insts: usize,
    /// Loop-local container handles released at the end of their block.
    pub scope_drops: usize,
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
        // After DCE, so a handle whose only readers just died is visible as
        // loop-local.
        let drops = scope_drop_block_locals(func);
        stats.cse_calls += cse;
        stats.dce_insts += dce;
        stats.scope_drops += drops;
    }
    stats
}

/// Frees block-local container handles at the end of the block that created
/// them, so code allocating a temporary container per iteration — or per call
/// — does not grow the `lkrt` arena without bound.
///
/// # Why this is needed
///
/// Container handles are arena-owned: `lkrt_cleanup()` reclaims them at exit
/// (RFC aot-redesign §3.4). That is fine for a short script and wrong for a
/// loop — `for i in 0..2_000_000 { let tmp = [i, i+1, i+2]; … }` retains every
/// temporary. Measured on that program:
///
/// | | wall | peak RSS |
/// |---|---|---|
/// | without this pass | 0.45 s | 250 MB |
/// | with it | 0.03 s | 2.9 MB |
/// | VM (for reference) | 0.11 s | 8.8 MB |
///
/// The time difference is the arena bookkeeping itself: registering two
/// million live handles costs more than the work the loop is doing. A program
/// whose containers genuinely escape is unaffected (300k escaping lists: 44 MB
/// native vs the VM's 48 MB).
///
/// # Why it is this conservative
///
/// Note what is *not* a condition: being inside a loop. That was the original
/// gate, on the theory that only loops accumulate — but it is a guess about
/// payoff, not a safety property, and it misses the common case of a function
/// that is *called* in a loop. A `try` body is the sharpest example: the
/// lowering makes it its own function, so the loop lives in the caller and the
/// body's own blocks look loop-free. Measured on a 200k-iteration
/// `try { let tmp = [i, i+1]; … }`, gating on loops left every temporary alive
/// (76 MB); without the gate it is flat.
///
/// A wrongly released handle is a use-after-free, so every condition below
/// must hold, and anything unrecognized keeps the old arena behavior:
///
/// - every use is in the defining block, and the terminator does not carry it
///   to another block. This is not the approximation it looks like: SSA passes
///   cross-block values as block arguments, so a handle reaching *any*
///   terminator may survive an iteration and must not be released. Measured on
///   the corpus, of 18 loop allocations 3 are block-local and **0** are
///   cross-block-but-non-escaping — block scope is the ceiling here, not a
///   shortcut ([`count_loop_allocations`] re-measures it);
/// - every use passes it as the **receiver** (parameter 0) of a call that
///   [`lk_aot_abi::Receiver`] contract says does not retain it. A handle passed
///   in any other position may be stored into another container, and a handle
///   read by a non-call instruction is not analyzed at all.
///
/// # Element strings
///
/// Releasing the container alone still left behind the arena *strings* it
/// created: `str.split`/`str.chars` mint one per element through
/// `arena_c_string`, and those live in a separate table that only the exit
/// reclaim drains. On `for i in 0..200_000 { "a-b-c-d-e".split("-") }` that was
/// the whole remaining footprint — the list handles were being freed while a
/// million element strings accumulated.
///
/// So the release comes in two forms, and [`no_element_can_escape`] picks:
/// `rt.handle_release_deep` when no use of the handle could have handed an
/// element back, `rt.handle_release` otherwise. Measured on that program:
///
/// | | peak RSS |
/// |---|---|
/// | without this pass | 89.8 MB |
/// | shallow release only | 60.2 MB |
/// | with the deep release | 4.9 MB |
/// | VM (for reference) | 22.7 MB |
fn scope_drop_block_locals(func: &mut MirFunction) -> usize {
    let mut dropped = 0;
    // Indexed rather than iterated: the body needs `&func` (for the cross-block
    // escape check) before taking `&mut func.blocks[bi]`.
    #[allow(clippy::needless_range_loop)]
    for bi in 0..func.blocks.len() {
        let candidates = block_local_handles(func, bi);
        if candidates.is_empty() {
            continue;
        }
        let deep: Vec<bool> = candidates
            .iter()
            .map(|&handle| no_element_can_escape(&func.blocks[bi], handle))
            .collect();
        let block = &mut func.blocks[bi];
        for (handle, deep) in candidates.into_iter().zip(deep) {
            let release = if deep { "handle_release_deep" } else { "handle_release" };
            block.insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("rt", release),
                args: vec![handle],
            });
            dropped += 1;
        }
    }
    dropped
}

/// Whether no *element* of `handle` can have escaped — the extra condition for
/// releasing the arena strings a container minted itself
/// (`rt.handle_release_deep`).
///
/// The caller has already established that every use of `handle` is a
/// non-retaining receiver use in this block. What is left is whether any of
/// those calls could have *handed an element back*, and the ABI signature
/// answers that: a call returning `I64`/`F64`/`Nil`/`Bool` cannot return a
/// pointer, so with a receiver that does not retain, no element can outlive the
/// call. `parts.len()` qualifies; `parts[0]` — `StrPtr` — does not.
///
/// Conservative on purpose: an entry may return a `Ptr` that is not an element
/// (a fresh container, say) and still be refused. It only costs the element
/// strings, which then wait for the arena's exit reclaim as before.
fn no_element_can_escape(block: &Block, handle: ValueId) -> bool {
    block.insts.iter().all(|inst| {
        let Inst::Call { callee, args, .. } = inst else {
            return true;
        };
        if args.first() != Some(&handle) {
            return true;
        }
        !matches!(
            lk_aot_abi::find(callee.module, callee.name).map(|abi| abi.result),
            // An unknown entry is treated as pointer-returning, same
            // conservative default as `receiver_of`.
            None | Some(lk_aot_abi::AbiType::Ptr)
                | Some(lk_aot_abi::AbiType::StrPtr)
                | Some(lk_aot_abi::AbiType::DynVal)
        )
    })
}

fn term_targets(term: &Term) -> Vec<BlockId> {
    match term {
        Term::Br { target, .. } => vec![*target],
        Term::CondBr { then_blk, else_blk, .. } => vec![*then_blk, *else_blk],
        Term::Ret(_) | Term::Abort => Vec::new(),
    }
}

/// Container handles constructed in block `bi` whose every use is a
/// non-retaining receiver use inside that same block (see
/// [`scope_drop_block_locals`] for why each condition is required).
fn block_local_handles(func: &MirFunction, bi: usize) -> Vec<ValueId> {
    let block = &func.blocks[bi];
    let mut created: Vec<ValueId> = Vec::new();
    for inst in &block.insts {
        // `constructs_handle` is the whole condition: the ABI schema's
        // `Constructs` annotation is the audited contract that a call returns a
        // fresh arena handle, and every constructor registers it the same way
        // (`lkrt::state::arena_handle`), so `rt.handle_release` — an
        // address-keyed lookup of the drop function stored at registration —
        // releases any of them. Filtering by module name on top of it only hid
        // constructors from the pass: `str.split`/`str.chars` are annotated
        // `Constructs` too, so `for l in lines { let parts = l.split(","); }`
        // retained every temporary list, which is the exact shape this pass
        // exists for.
        if let Inst::Call {
            dst: Some(dst), callee, ..
        } = inst
            && constructs_handle(callee)
        {
            created.push(*dst);
        }
    }
    created.retain(|&handle| {
        // Carried out of the block by the terminator (including block args)?
        let mut term = block.term.clone();
        if term_uses_mut(&mut term).into_iter().any(|u| *u == handle) {
            return false;
        }
        // Read by any other block? (Block params rebind, so a same-id read
        // elsewhere would still be this value.)
        for (other, b) in func.blocks.iter().enumerate() {
            if other == bi {
                continue;
            }
            for inst in &b.insts {
                let mut inst = inst.clone();
                if uses_mut(&mut inst).into_iter().any(|u| *u == handle) {
                    return false;
                }
            }
            let mut t = b.term.clone();
            if term_uses_mut(&mut t).into_iter().any(|u| *u == handle) {
                return false;
            }
        }
        // Every in-block use must be a non-retaining receiver use.
        block.insts.iter().all(|inst| {
            let mut probe = inst.clone();
            let mentions = uses_mut(&mut probe).into_iter().any(|u| *u == handle);
            if !mentions {
                return true;
            }
            match inst {
                Inst::Call { callee, args, .. } => {
                    args.first() == Some(&handle)
                        && args[1..].iter().all(|a| *a != handle)
                        && !receiver_of(callee).retains()
                }
                // Any other instruction reading the handle (a return value, a
                // carrier, a bridge call) is not analyzed — keep the handle.
                _ => false,
            }
        })
    });
    created
}

/// The schema's handle-ownership contract for a callee, defaulting to the
/// conservative [`lk_aot_abi::Receiver::Retained`] for a callee the schema
/// does not know (which cannot happen for a validated module, but the pass
/// must not assume that to stay memory-safe).
fn receiver_of(callee: &AbiRef) -> lk_aot_abi::Receiver {
    lk_aot_abi::find(callee.module, callee.name).map_or(lk_aot_abi::Receiver::Retained, |abi| abi.receiver)
}

/// Whether a call allocates and returns a fresh arena container handle —
/// per the audited schema annotation, never a name pattern. `dyn.as_list`
/// also returns a `Ptr` but hands back an *existing* handle, which is exactly
/// the distinction a name match would get wrong.
fn constructs_handle(callee: &AbiRef) -> bool {
    receiver_of(callee).constructs()
}

/// Common-subexpression elimination over `Pure` ABI calls, scoped by
/// **dominance**.
///
/// A redundant call may be collapsed into an earlier one only if that earlier
/// call is guaranteed to have executed — i.e. its block dominates this one.
/// Walking the dominator tree with a scoped table gives exactly that: every
/// candidate visible at a block was defined on a path that must have run.
/// (A block-scoped table would miss the majority of the redundancy: measured
/// on the corpus, 5 collapses are block-local and 52 more span blocks.)
fn cse_pure_calls(func: &mut MirFunction) -> usize {
    let Some(idom) = immediate_dominators(func) else {
        return 0;
    };
    let mut rewrite: HashMap<ValueId, ValueId> = HashMap::new();
    let mut collapsed = 0;
    // Children in the dominator tree, walked depth-first so a block's table
    // holds exactly its dominators' candidates.
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); func.blocks.len()];
    let entry = func.blocks.iter().position(|b| b.id == func.entry).unwrap_or(0);
    for (bi, parent) in idom.iter().enumerate() {
        if let Some(parent) = parent
            && *parent != bi
        {
            children[*parent].push(bi);
        }
    }

    // (block, candidates introduced there) — popped when the subtree is done.
    let mut table: HashMap<(&'static str, &'static str, Vec<ValueId>), ValueId> = HashMap::new();
    let mut stack: Vec<(usize, bool)> = vec![(entry, false)];
    let mut scopes: Vec<Vec<(&'static str, &'static str, Vec<ValueId>)>> = Vec::new();
    while let Some((bi, exiting)) = stack.pop() {
        if exiting {
            for key in scopes.pop().unwrap_or_default() {
                table.remove(&key);
            }
            continue;
        }
        let mut introduced = Vec::new();
        let block = &mut func.blocks[bi];
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
                match table.get(&key) {
                    Some(&existing) => {
                        rewrite.insert(*dst, existing);
                        collapsed += 1;
                        continue; // drop the redundant call
                    }
                    None => {
                        table.insert(key.clone(), *dst);
                        introduced.push(key);
                    }
                }
            }
            keep.push(inst);
        }
        block.insts = keep;
        scopes.push(introduced);
        stack.push((bi, true));
        for &child in &children[bi] {
            stack.push((child, false));
        }
    }

    // Terminators (and any block the dominator walk did not reach) still need
    // the rewrite applied.
    for block in &mut func.blocks {
        for inst in &mut block.insts {
            for use_ in uses_mut(inst) {
                *use_ = resolve(&rewrite, *use_);
            }
        }
        for use_ in term_uses_mut(&mut block.term) {
            *use_ = resolve(&rewrite, *use_);
        }
    }
    collapsed
}

/// Immediate dominators by block index (Cooper–Harvey–Kennedy iteration).
/// `None` for a block the entry cannot reach; `None` overall if the function
/// has no entry block, in which case callers skip the pass rather than guess.
fn immediate_dominators(func: &MirFunction) -> Option<Vec<Option<usize>>> {
    let n = func.blocks.len();
    let entry = func.blocks.iter().position(|b| b.id == func.entry)?;
    let index_of = |id: BlockId| func.blocks.iter().position(|b| b.id == id);

    // Reverse postorder over the CFG, plus predecessors.
    let mut order = Vec::with_capacity(n);
    let mut visited = vec![false; n];
    let mut stack = vec![(entry, false)];
    while let Some((bi, done)) = stack.pop() {
        if done {
            order.push(bi);
            continue;
        }
        if visited[bi] {
            continue;
        }
        visited[bi] = true;
        stack.push((bi, true));
        for target in term_targets(&func.blocks[bi].term) {
            if let Some(ti) = index_of(target)
                && !visited[ti]
            {
                stack.push((ti, false));
            }
        }
    }
    order.reverse(); // now reverse postorder
    let mut rpo_num = vec![usize::MAX; n];
    for (rank, &bi) in order.iter().enumerate() {
        rpo_num[bi] = rank;
    }
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (bi, block) in func.blocks.iter().enumerate() {
        for target in term_targets(&block.term) {
            if let Some(ti) = index_of(target) {
                preds[ti].push(bi);
            }
        }
    }

    let mut idom: Vec<Option<usize>> = vec![None; n];
    idom[entry] = Some(entry);
    let intersect = |idom: &[Option<usize>], mut a: usize, mut b: usize| -> usize {
        while a != b {
            while rpo_num[a] > rpo_num[b] {
                a = idom[a].expect("processed predecessor has an idom");
            }
            while rpo_num[b] > rpo_num[a] {
                b = idom[b].expect("processed predecessor has an idom");
            }
        }
        a
    };
    let mut changed = true;
    while changed {
        changed = false;
        for &bi in &order {
            if bi == entry {
                continue;
            }
            let mut new_idom: Option<usize> = None;
            for &p in &preds[bi] {
                if idom[p].is_none() {
                    continue; // not yet processed on this round
                }
                new_idom = Some(match new_idom {
                    None => p,
                    Some(current) => intersect(&idom, p, current),
                });
            }
            if new_idom.is_some() && idom[bi] != new_idom {
                idom[bi] = new_idom;
                changed = true;
            }
        }
    }
    // The entry dominates itself; represent that as "no parent" so the caller
    // does not build a self-edge in the tree.
    idom[entry] = None;
    Some(idom)
}

/// Maps a value to its CSE replacement, if any.
///
/// The map is flat by construction — a collapsed value is never itself a CSE
/// table entry, so a replacement is never replaced again — and the assertion
/// below states that invariant rather than papering over a violation of it.
/// (An earlier version chased a chain with an iteration cap, which would have
/// returned a *wrong* value silently had the invariant ever broken.)
fn resolve(rewrite: &HashMap<ValueId, ValueId>, v: ValueId) -> ValueId {
    match rewrite.get(&v) {
        Some(&replacement) => {
            debug_assert!(
                !rewrite.contains_key(&replacement),
                "CSE replacement {replacement:?} was itself replaced — the rewrite map must stay flat"
            );
            replacement
        }
        None => v,
    }
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
        | Inst::IntTruncate { .. }
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
        | Inst::IntTruncate { src, .. }
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

mod metrics;
pub use metrics::{count_cross_block_cse_candidates, count_licm_candidates, count_loop_allocations};

#[cfg(test)]
mod tests;
