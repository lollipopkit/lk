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
