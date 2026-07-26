//! Measurement-only counters over MIR.
//!
//! Deliberately separate from the passes in the parent module: nothing here
//! mutates a `MirModule`. These exist so decisions about *whether* to build a
//! pass stay evidence-based — each one answers "how much would this be worth?"
//! and is re-runnable with `LK_AOT_OPT_STATS=1`. Two of them have already
//! talked us out of writing a pass (see the parent module's notes on LICM and
//! on cross-block scope drop).

use std::collections::HashMap;

use super::{constructs_handle, immediate_dominators, term_targets, term_uses_mut, uses_mut};
use crate::{BlockId, Inst, MirFunction, MirModule, ValueId, inst_def};

/// Blocks belonging to some natural loop.
///
/// Uses the textbook definition rather than block ordering: an edge `b → h` is
/// a back edge when `h` dominates `b`, and the loop body is `h` plus every
/// block that reaches `b` without going through `h`. An ordering-based
/// approximation would silently mean something different if the lowering ever
/// changed how it numbers blocks.
fn loop_blocks(func: &MirFunction, idom: &[Option<usize>]) -> Vec<bool> {
    let n = func.blocks.len();
    let mut looped = vec![false; n];
    let index_of = |id: BlockId| func.blocks.iter().position(|b| b.id == id);

    // `a` dominates `b` iff `a` is `b` or an ancestor of `b` in the dom tree.
    let dominates = |a: usize, b: usize| {
        let mut cur = Some(b);
        while let Some(c) = cur {
            if c == a {
                return true;
            }
            cur = idom[c];
        }
        false
    };

    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (bi, block) in func.blocks.iter().enumerate() {
        for target in term_targets(&block.term) {
            if let Some(ti) = index_of(target) {
                preds[ti].push(bi);
            }
        }
    }

    for (bi, block) in func.blocks.iter().enumerate() {
        for target in term_targets(&block.term) {
            let Some(header) = index_of(target) else { continue };
            if !dominates(header, bi) {
                continue; // not a back edge
            }
            // Natural loop body: walk predecessors back from the latch,
            // stopping at the header.
            looped[header] = true;
            looped[bi] = true;
            let mut work = vec![bi];
            let mut seen = vec![false; n];
            seen[header] = true;
            seen[bi] = true;
            while let Some(cur) = work.pop() {
                for &p in &preds[cur] {
                    if !seen[p] {
                        seen[p] = true;
                        looped[p] = true;
                        work.push(p);
                    }
                }
            }
        }
    }
    looped
}

/// Counts loop-invariant `Pure` calls — the candidate set a LICM pass would
/// hoist.
///
/// Loop membership comes from [`loop_blocks`], i.e. the natural loops of the
/// dominator-tree back edges, not from any block-id span or lowering order. A
/// call counts when it is `Pure` and none of its arguments are defined
/// anywhere in the loop body. It exists to answer "is there anything to hoist
/// at all", not to drive a transformation.
#[doc(hidden)]
pub fn count_licm_candidates(module: &MirModule) -> usize {
    let mut candidates = 0;
    for func in &module.functions {
        let Some(idom) = immediate_dominators(func) else {
            continue;
        };
        let looped = loop_blocks(func, &idom);
        // Values defined anywhere in a loop body are not invariant.
        let mut inside = std::collections::HashSet::new();
        for (bi, block) in func.blocks.iter().enumerate() {
            if !looped[bi] {
                continue;
            }
            inside.extend(block.params.iter().map(|(v, _)| *v));
            for inst in &block.insts {
                if let Some(dst) = inst_def(inst) {
                    inside.insert(dst);
                }
            }
        }
        for (bi, block) in func.blocks.iter().enumerate() {
            if !looped[bi] {
                continue;
            }
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
    candidates
}

/// Counts container handles constructed **inside a loop body** — the working
/// set a scope-drop pass would target. Like [`count_licm_candidates`] this is
/// a measurement hook, not a transformation: it answers "how much garbage does
/// a long-running loop accumulate in the lkrt arena".
///
/// Returns `(loop_constructions, block_local, cross_block_but_not_escaping)`.
/// Anything reaching a terminator is excluded from the last two — it may
/// survive an iteration.
///
/// Both are *liveness* categories only: unlike
/// [`super::scope_drop_block_locals`], neither applies the pass's eligibility
/// filters (the `list_h`/`map_h`/`set` construction allowlist and the
/// non-retaining-receiver-use requirement). So `block_local` is an **upper
/// bound** on what the pass releases inside loops, not the set it releases —
/// expect it to exceed the `scope_drops` figure printed beside it, and note
/// that `scope_drops` also counts releases outside any loop, which this
/// counter never sees. The third is likewise an upper bound on what a
/// cross-block liveness analysis could additionally reach.
#[doc(hidden)]
pub fn count_loop_allocations(module: &MirModule) -> (usize, usize, usize) {
    let mut in_loop = 0;
    let mut block_local = 0;
    let mut cross_block = 0;
    for func in &module.functions {
        let Some(idom) = immediate_dominators(func) else {
            continue;
        };
        let looped = loop_blocks(func, &idom);
        for (bi, block) in func.blocks.iter().enumerate() {
            if !looped[bi] {
                continue;
            }
            for inst in &block.insts {
                let Inst::Call {
                    dst: Some(dst), callee, ..
                } = inst
                else {
                    continue;
                };
                if !constructs_handle(callee) {
                    continue;
                }
                in_loop += 1;
                // A handle reaching *any* terminator can cross an iteration
                // (SSA passes cross-block values as block arguments), so that
                // is the category no liveness analysis can release.
                let escapes_via_term = func.blocks.iter().any(|b| {
                    let mut t = b.term.clone();
                    term_uses_mut(&mut t).into_iter().any(|u| *u == *dst)
                });
                let used_elsewhere = func.blocks.iter().enumerate().any(|(other, b)| {
                    other != bi
                        && b.insts.iter().any(|i| {
                            let mut i = i.clone();
                            uses_mut(&mut i).into_iter().any(|u| *u == *dst)
                        })
                });
                if !escapes_via_term && !used_elsewhere {
                    block_local += 1;
                } else if !escapes_via_term {
                    cross_block += 1;
                }
            }
        }
    }
    (in_loop, block_local, cross_block)
}

/// Counts `Pure` calls that a *cross-block* CSE could additionally collapse:
/// an identical `(callee, args)` pair appearing in two different blocks. Like
/// the other counters this only measures, so the decision to extend CSE past
/// block scope stays evidence-based.
#[doc(hidden)]
pub fn count_cross_block_cse_candidates(module: &MirModule) -> usize {
    let mut extra = 0;
    for func in &module.functions {
        let mut seen: HashMap<(&'static str, &'static str, Vec<ValueId>), usize> = HashMap::new();
        for (bi, block) in func.blocks.iter().enumerate() {
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
                let key = (callee.module, callee.name, args.clone());
                match seen.get(&key) {
                    Some(&first) if first != bi => extra += 1,
                    Some(_) => {}
                    None => {
                        seen.insert(key, bi);
                    }
                }
            }
        }
    }
    extra
}
