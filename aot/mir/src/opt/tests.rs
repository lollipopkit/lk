use super::*;
use crate::{AbiRef, Block, BlockId, Const, FuncId, MirFunction, Ty};

/// Builds a one-block function with the given instructions, returning `ret`.
fn one_block(insts: Vec<Inst>, ret: Option<ValueId>) -> MirFunction {
    MirFunction {
        id: FuncId(0),
        params: Vec::new(),
        blocks: vec![Block {
            id: BlockId(0),
            params: Vec::new(),
            insts,
            term: Term::Ret(ret),
        }],
        entry: BlockId(0),
        ret: Ty::I64,
    }
}

fn call(dst: u32, module: &'static str, name: &'static str, args: &[u32]) -> Inst {
    Inst::Call {
        dst: Some(ValueId(dst)),
        callee: AbiRef::new(module, name),
        args: args.iter().map(|&a| ValueId(a)).collect(),
    }
}

fn konst(dst: u32, value: i64) -> Inst {
    Inst::Const {
        dst: ValueId(dst),
        value: Const::I64(value),
    }
}

#[test]
fn cse_collapses_identical_pure_calls() {
    // `math.floor(v0)` twice → one call, both readers see the first result.
    let mut func = one_block(
        vec![
            Inst::Const {
                dst: ValueId(0),
                value: Const::F64(1.5),
            },
            call(1, "math", "floor", &[0]),
            call(2, "math", "floor", &[0]),
            Inst::IntBin {
                dst: ValueId(3),
                op: IntBinOp::Add,
                lhs: ValueId(1),
                rhs: ValueId(2),
            },
        ],
        Some(ValueId(3)),
    );
    assert_eq!(cse_pure_calls(&mut func), 1);
    let insts = &func.blocks[0].insts;
    assert_eq!(insts.len(), 3, "the redundant call is gone");
    // The addition now reads the surviving call twice.
    assert!(matches!(
        insts[2],
        Inst::IntBin {
            lhs: ValueId(1),
            rhs: ValueId(1),
            ..
        }
    ));
}

#[test]
fn cse_keeps_calls_with_different_arguments() {
    let mut func = one_block(
        vec![
            konst(0, 1),
            konst(1, 2),
            call(2, "math", "sign_i64", &[0]),
            call(3, "math", "sign_i64", &[1]),
        ],
        Some(ValueId(2)),
    );
    assert_eq!(cse_pure_calls(&mut func), 0);
    assert_eq!(func.blocks[0].insts.len(), 4);
}

#[test]
fn cse_leaves_effectful_calls_alone() {
    // `io.std.flush` is `WritesHost`: two flushes are two flushes.
    let mut func = one_block(
        vec![
            konst(0, 1),
            call(1, "io.std", "flush", &[0]),
            call(2, "io.std", "flush", &[0]),
        ],
        None,
    );
    assert_eq!(cse_pure_calls(&mut func), 0);
    assert_eq!(func.blocks[0].insts.len(), 3);
}

#[test]
fn cse_rewrites_uses_in_later_blocks_and_terminators() {
    let mut func = MirFunction {
        id: FuncId(0),
        params: Vec::new(),
        blocks: vec![
            Block {
                id: BlockId(0),
                params: Vec::new(),
                insts: vec![
                    konst(0, 7),
                    call(1, "math", "sign_i64", &[0]),
                    call(2, "math", "sign_i64", &[0]),
                ],
                // The collapsed value is passed as a block argument.
                term: Term::Br {
                    target: BlockId(1),
                    args: vec![ValueId(2)],
                },
            },
            Block {
                id: BlockId(1),
                params: vec![(ValueId(3), Ty::I64)],
                insts: Vec::new(),
                term: Term::Ret(Some(ValueId(3))),
            },
        ],
        entry: BlockId(0),
        ret: Ty::I64,
    };
    assert_eq!(cse_pure_calls(&mut func), 1);
    assert!(
        matches!(&func.blocks[0].term, Term::Br { args, .. } if args == &[ValueId(1)]),
        "the branch argument follows the rewrite"
    );
}

#[test]
fn dce_removes_dead_pure_data() {
    let mut func = one_block(vec![konst(0, 1), konst(1, 2)], Some(ValueId(0)));
    assert_eq!(eliminate_dead_insts(&mut func), 1);
    assert_eq!(func.blocks[0].insts.len(), 1);
}

#[test]
fn dce_cascades_through_a_dead_chain() {
    // v1 = v0 + v0; v2 = zext(cmp) … nothing reads them → both go, in one call.
    let mut func = one_block(
        vec![
            konst(0, 1),
            Inst::IntBin {
                dst: ValueId(1),
                op: IntBinOp::Add,
                lhs: ValueId(0),
                rhs: ValueId(0),
            },
            Inst::IntToFloat {
                dst: ValueId(2),
                src: ValueId(1),
            },
            konst(3, 9),
        ],
        Some(ValueId(3)),
    );
    assert_eq!(eliminate_dead_insts(&mut func), 3);
    assert_eq!(func.blocks[0].insts.len(), 1);
}

#[test]
fn dce_keeps_a_dead_division() {
    // `x / 0` aborts like the VM — the result being unread does not make the
    // check removable.
    let mut func = one_block(
        vec![
            konst(0, 1),
            konst(1, 0),
            Inst::IntBin {
                dst: ValueId(2),
                op: IntBinOp::Div,
                lhs: ValueId(0),
                rhs: ValueId(1),
            },
        ],
        Some(ValueId(0)),
    );
    assert_eq!(eliminate_dead_insts(&mut func), 0);
    assert_eq!(func.blocks[0].insts.len(), 3);
}

#[test]
fn dce_keeps_a_dead_maybe_unwrap_and_dead_call() {
    // Both abort on their failure path; an unread result does not license
    // dropping the abort.
    let mut func = one_block(
        vec![
            konst(0, 0),
            Inst::UnwrapMaybeI64 {
                dst: ValueId(1),
                src: ValueId(0),
            },
            call(2, "math", "floor", &[0]),
        ],
        Some(ValueId(0)),
    );
    assert_eq!(eliminate_dead_insts(&mut func), 0);
    assert_eq!(func.blocks[0].insts.len(), 3);
}

#[test]
fn optimized_module_still_validates() {
    let mut module = MirModule {
        abi_version: lk_aot_abi::ABI_VERSION,
        functions: vec![one_block(
            vec![
                Inst::Const {
                    dst: ValueId(0),
                    value: Const::F64(2.5),
                },
                call(1, "math", "floor", &[0]),
                call(2, "math", "floor", &[0]),
                Inst::IntBin {
                    dst: ValueId(3),
                    op: IntBinOp::Add,
                    lhs: ValueId(1),
                    rhs: ValueId(2),
                },
                konst(4, 99),
            ],
            Some(ValueId(3)),
        )],
        entry: FuncId(0),
        globals: Vec::new(),
        mutable_globals: Vec::new(),
        vm_functions: Vec::new(),
    };
    crate::validate(&module).expect("valid before");
    let stats = optimize(&mut module);
    assert_eq!(stats.cse_calls, 1);
    assert_eq!(stats.dce_insts, 1, "the unused constant goes");
    crate::validate(&module).expect("still valid after");
}

/// A two-block loop: `bb0` jumps to `bb1`, which branches back to itself.
/// `insts` go in the loop body (`bb1`).
fn loop_func(insts: Vec<Inst>, cond: ValueId, extra_args: Vec<ValueId>) -> MirFunction {
    MirFunction {
        id: FuncId(0),
        params: Vec::new(),
        blocks: vec![
            Block {
                id: BlockId(0),
                params: Vec::new(),
                insts: vec![Inst::Const {
                    dst: cond,
                    value: Const::Bool(true),
                }],
                term: Term::Br {
                    target: BlockId(1),
                    args: Vec::new(),
                },
            },
            Block {
                id: BlockId(1),
                params: Vec::new(),
                insts,
                term: Term::CondBr {
                    cond,
                    then_blk: BlockId(1),
                    then_args: extra_args,
                    else_blk: BlockId(2),
                    else_args: Vec::new(),
                },
            },
            Block {
                id: BlockId(2),
                params: Vec::new(),
                insts: Vec::new(),
                term: Term::Ret(None),
            },
        ],
        entry: BlockId(0),
        ret: Ty::I64,
    }
}

fn released_handles(func: &MirFunction) -> Vec<ValueId> {
    func.blocks
        .iter()
        .flat_map(|b| b.insts.iter())
        .filter_map(|inst| match inst {
            Inst::Call { callee, args, .. } if callee.module == "rt" && callee.name == "handle_release" => {
                args.first().copied()
            }
            _ => None,
        })
        .collect()
}

#[test]
fn scope_drop_releases_a_loop_local_container() {
    // `let tmp = []; tmp.push(1); tmp.len()` inside a loop body: the handle
    // never leaves the block, so it is released at the end of each iteration.
    let mut func = loop_func(
        vec![
            call(10, "list_h", "i64_new", &[]),
            konst(11, 1),
            Inst::Call {
                dst: None,
                callee: AbiRef::new("list_h", "i64_push"),
                args: vec![ValueId(10), ValueId(11)],
            },
            call(12, "list_h", "i64_len", &[10]),
        ],
        ValueId(0),
        Vec::new(),
    );
    assert_eq!(scope_drop_block_locals(&mut func), 1);
    assert_eq!(released_handles(&func), vec![ValueId(10)]);
    // The release is the last instruction, i.e. after every use.
    let body = &func.blocks[1].insts;
    assert!(matches!(
        body.last(),
        Some(Inst::Call { callee, .. }) if callee.name == "handle_release"
    ));
}

#[test]
fn scope_drop_skips_a_handle_escaping_through_the_terminator() {
    // The handle is passed as a block argument to the next iteration — it
    // outlives this block, so releasing it would be a use-after-free.
    let mut func = loop_func(
        vec![call(10, "list_h", "i64_new", &[]), call(12, "list_h", "i64_len", &[10])],
        ValueId(0),
        vec![ValueId(10)],
    );
    assert_eq!(scope_drop_block_locals(&mut func), 0);
    assert!(released_handles(&func).is_empty());
}

#[test]
fn scope_drop_skips_a_handle_stored_into_another_container() {
    // `outer.push(tmp)` — the handle is a *value* argument, so the callee may
    // retain it.
    let mut func = loop_func(
        vec![
            call(9, "list_h", "dyn_new", &[]),
            call(10, "list_h", "dyn_new", &[]),
            Inst::Call {
                dst: None,
                callee: AbiRef::new("list_h", "dyn_push"),
                args: vec![ValueId(9), ValueId(10)],
            },
        ],
        ValueId(0),
        vec![ValueId(9)],
    );
    assert_eq!(scope_drop_block_locals(&mut func), 0, "the pushed handle must survive");
    assert!(released_handles(&func).is_empty());
}

#[test]
fn scope_drop_skips_a_handle_boxed_into_a_dyn() {
    // `dyn.from_list(tmp)` retains the handle inside the boxed value, which
    // the schema's `Receiver::Retained` records — even though `tmp` is the receiver.
    let mut func = loop_func(
        vec![call(10, "list_h", "i64_new", &[]), call(11, "dyn", "from_list", &[10])],
        ValueId(0),
        vec![ValueId(11)],
    );
    assert_eq!(scope_drop_block_locals(&mut func), 0);
    assert!(released_handles(&func).is_empty());
}

#[test]
fn scope_drop_also_releases_outside_loops() {
    // Straight-line code is released too: the payoff is not "this block
    // repeats" but "this *function* may be called repeatedly" — a `try` body
    // is its own function whose caller holds the loop.
    let mut func = one_block(
        vec![call(10, "list_h", "i64_new", &[]), call(11, "list_h", "i64_len", &[10])],
        Some(ValueId(11)),
    );
    assert_eq!(scope_drop_block_locals(&mut func), 1);
    assert_eq!(released_handles(&func), vec![ValueId(10)]);
}

#[test]
fn scope_drop_skips_a_handle_read_by_a_later_block() {
    let mut func = loop_func(vec![call(10, "list_h", "i64_new", &[])], ValueId(0), Vec::new());
    // Block 2 reads the handle even though the terminator does not pass it.
    func.blocks[2].insts.push(call(13, "list_h", "i64_len", &[10]));
    assert_eq!(scope_drop_block_locals(&mut func), 0);
    assert!(released_handles(&func).is_empty());
}

#[test]
fn cse_collapses_across_blocks_when_the_first_call_dominates() {
    // bb0 computes `math.sign_i64(v0)` and unconditionally falls into bb1,
    // which computes it again. bb0 dominates bb1, so the second call is
    // genuinely redundant.
    let mut func = MirFunction {
        id: FuncId(0),
        params: Vec::new(),
        blocks: vec![
            Block {
                id: BlockId(0),
                params: Vec::new(),
                insts: vec![konst(0, 7), call(1, "math", "sign_i64", &[0])],
                term: Term::Br {
                    target: BlockId(1),
                    args: Vec::new(),
                },
            },
            Block {
                id: BlockId(1),
                params: Vec::new(),
                insts: vec![call(2, "math", "sign_i64", &[0])],
                term: Term::Ret(Some(ValueId(2))),
            },
        ],
        entry: BlockId(0),
        ret: Ty::I64,
    };
    assert_eq!(cse_pure_calls(&mut func), 1);
    assert!(func.blocks[1].insts.is_empty(), "the dominated repeat is gone");
    assert!(
        matches!(&func.blocks[1].term, Term::Ret(Some(v)) if *v == ValueId(1)),
        "the return reads the dominating call's result"
    );
}

#[test]
fn cse_does_not_collapse_across_sibling_branches() {
    // bb1 and bb2 are two arms of a conditional: neither dominates the other,
    // so reusing bb1's result inside bb2 would read a value that never ran.
    let mut func = MirFunction {
        id: FuncId(0),
        params: Vec::new(),
        blocks: vec![
            Block {
                id: BlockId(0),
                params: Vec::new(),
                insts: vec![
                    konst(0, 7),
                    Inst::Const {
                        dst: ValueId(9),
                        value: Const::Bool(true),
                    },
                ],
                term: Term::CondBr {
                    cond: ValueId(9),
                    then_blk: BlockId(1),
                    then_args: Vec::new(),
                    else_blk: BlockId(2),
                    else_args: Vec::new(),
                },
            },
            Block {
                id: BlockId(1),
                params: Vec::new(),
                insts: vec![call(1, "math", "sign_i64", &[0])],
                term: Term::Ret(Some(ValueId(1))),
            },
            Block {
                id: BlockId(2),
                params: Vec::new(),
                insts: vec![call(2, "math", "sign_i64", &[0])],
                term: Term::Ret(Some(ValueId(2))),
            },
        ],
        entry: BlockId(0),
        ret: Ty::I64,
    };
    assert_eq!(cse_pure_calls(&mut func), 0, "sibling arms must keep their own call");
    assert_eq!(func.blocks[1].insts.len(), 1);
    assert_eq!(func.blocks[2].insts.len(), 1);
}

#[test]
fn cse_does_not_hoist_out_of_a_loop_body_into_a_later_block() {
    // The call lives in a loop body (bb1); bb2 runs after the loop. bb1 does
    // not dominate bb2 (the loop may run zero times), so bb2 keeps its call.
    let mut func = MirFunction {
        id: FuncId(0),
        params: Vec::new(),
        blocks: vec![
            Block {
                id: BlockId(0),
                params: Vec::new(),
                insts: vec![
                    konst(0, 7),
                    Inst::Const {
                        dst: ValueId(9),
                        value: Const::Bool(true),
                    },
                ],
                term: Term::CondBr {
                    cond: ValueId(9),
                    then_blk: BlockId(1),
                    then_args: Vec::new(),
                    else_blk: BlockId(2),
                    else_args: Vec::new(),
                },
            },
            Block {
                id: BlockId(1),
                params: Vec::new(),
                insts: vec![call(1, "math", "sign_i64", &[0])],
                term: Term::CondBr {
                    cond: ValueId(9),
                    then_blk: BlockId(1),
                    then_args: Vec::new(),
                    else_blk: BlockId(2),
                    else_args: Vec::new(),
                },
            },
            Block {
                id: BlockId(2),
                params: Vec::new(),
                insts: vec![call(2, "math", "sign_i64", &[0])],
                term: Term::Ret(Some(ValueId(2))),
            },
        ],
        entry: BlockId(0),
        ret: Ty::I64,
    };
    assert_eq!(cse_pure_calls(&mut func), 0);
    assert_eq!(func.blocks[2].insts.len(), 1, "the post-loop call must stay");
}
