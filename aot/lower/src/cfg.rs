use super::*;

pub(crate) fn mark_target(
    t: usize,
    code_len: usize,
    leaders: &mut std::collections::BTreeSet<usize>,
    implicit_ret: &mut bool,
) {
    if t >= code_len {
        *implicit_ret = true;
    } else {
        leaders.insert(t);
    }
}

/// `(body_end, exit)` for block `[start, end)`. A fused compare-and-branch occupies
/// the last two slots (`TestXxx` at `end-2`, consumed `Jmp` at `end-1`).
pub(crate) fn block_span(exits: &[Option<Exit>], consumed: &[bool], start: usize, end: usize) -> (usize, Option<Exit>) {
    if end >= start + 2 && consumed[end - 1] {
        return (end - 2, exits[end - 2]);
    }
    if end > start
        && let Some(exit) = exits[end - 1]
    {
        return (end - 1, Some(exit));
    }
    (end, None)
}

pub(crate) fn exit_successors(exit: Option<Exit>, fallthrough: usize) -> Vec<usize> {
    match exit {
        None => vec![fallthrough],
        Some(Exit::Ret(_)) => vec![],
        Some(Exit::Jump(t)) => vec![t],
        Some(Exit::Cond { then_pc, else_pc, .. }) => vec![then_pc, else_pc],
        Some(Exit::FusedCmp { taken, fallthrough, .. })
        | Some(Exit::FusedCmp2 { taken, fallthrough, .. })
        | Some(Exit::ForLoop { taken, fallthrough, .. })
        | Some(Exit::FusedModZero { taken, fallthrough, .. })
        | Some(Exit::NilBranch { taken, fallthrough, .. }) => {
            vec![taken, fallthrough]
        }
    }
}

pub(crate) fn exit_of(
    pc: usize,
    instrs: &[Instr],
    code_len: usize,
    consumed: &mut [bool],
    facts: &lk_core::vm::analysis::PerformanceFacts,
) -> Result<Option<Exit>, Unsupported> {
    if consumed[pc] {
        return Ok(None);
    }
    let instr = instrs[pc];
    if instr.opcode().is_compare_test() {
        let jmp = instrs.get(pc + 1).copied().ok_or(Unsupported::BadTarget { pc })?;
        if jmp.opcode() != Opcode::Jmp {
            return Err(Unsupported::Opcode { pc, op: instr.opcode() });
        }
        if instr.opcode() == Opcode::TestEqIntI2 {
            // `r_a == (c >> 4) && r_b == (c & 0xf)`: true falls through, false
            // takes the trailing `Jmp` (the VM's false-branch application).
            let packed = instr.c();
            let taken = rel(pc + 1, jmp.sj_arg(), code_len).ok_or(Unsupported::BadTarget { pc })?;
            consumed[pc + 1] = true;
            return Ok(Some(Exit::FusedCmp2 {
                reg_a: instr.a(),
                imm_a: i64::from(packed >> 4),
                reg_b: instr.b(),
                imm_b: i64::from(packed & 0x0f),
                taken,
                fallthrough: pc + 2,
            }));
        }
        let op = test_cmp_op(instr.opcode()).ok_or(Unsupported::Opcode { pc, op: instr.opcode() })?;
        let immediate = instr.opcode().is_int_immediate_compare_test();
        let rhs = if immediate {
            FusedRhs::Imm(instr.sc() as i64)
        } else {
            FusedRhs::Reg(instr.b())
        };
        let jump_when = if immediate { instr.b() != 0 } else { instr.c() != 0 };
        let taken = rel(pc + 1, jmp.sj_arg(), code_len).ok_or(Unsupported::BadTarget { pc })?;
        consumed[pc + 1] = true;
        return Ok(Some(Exit::FusedCmp {
            reg_a: instr.a(),
            rhs,
            op,
            jump_when,
            taken,
            fallthrough: pc + 2,
        }));
    }
    match instr.opcode() {
        Opcode::Return | Opcode::Return1 => Ok(Some(Exit::Ret(Some(instr.a())))),
        Opcode::Return0 => Ok(Some(Exit::Ret(None))),
        Opcode::Jmp => {
            let t = rel(pc, instr.sj_arg(), code_len).ok_or(Unsupported::BadTarget { pc })?;
            Ok(Some(Exit::Jump(t)))
        }
        // Fused compare-and-branch against an immediate (a single instruction, no
        // trailing `Jmp`): `if (r_a <op> imm) goto target else fall through`. The VM
        // requires an `Int` operand; the immediate is an unsigned byte. These are
        // what `if (i == k)` / `!=` inside loops lower to (enabling break/continue/
        // early-return/else-if shapes).
        Opcode::ForLoopI => {
            let Some(fact) = facts.for_loop(pc) else {
                return Err(Unsupported::Opcode { pc, op: instr.opcode() });
            };
            let taken = rel(pc, fact.jump_offset, code_len).ok_or(Unsupported::BadTarget { pc })?;
            Ok(Some(Exit::ForLoop {
                index_reg: instr.a(),
                end_reg: instr.b(),
                step_reg: instr.c(),
                inclusive: fact.inclusive,
                positive_step: fact.positive_step,
                taken,
                fallthrough: pc + 1,
            }))
        }
        Opcode::BrEqIntI4 | Opcode::BrNeIntI4 => {
            let taken = rel(pc, instr.branch_i4_offset() as i32, code_len).ok_or(Unsupported::BadTarget { pc })?;
            let op = if instr.opcode() == Opcode::BrEqIntI4 {
                CmpOp::Eq
            } else {
                CmpOp::Ne
            };
            Ok(Some(Exit::FusedCmp {
                reg_a: instr.a(),
                rhs: FusedRhs::Imm(i64::from(instr.branch_i4_immediate())),
                op,
                jump_when: true,
                taken,
                fallthrough: pc + 1,
            }))
        }
        // `if (r_a == 0)` / `!= 0` fused branch (immediate zero).
        Opcode::BrEqZeroInt | Opcode::BrNeZeroInt => {
            let taken = rel(pc, instr.sbx() as i32, code_len).ok_or(Unsupported::BadTarget { pc })?;
            let op = if instr.opcode() == Opcode::BrEqZeroInt {
                CmpOp::Eq
            } else {
                CmpOp::Ne
            };
            Ok(Some(Exit::FusedCmp {
                reg_a: instr.a(),
                rhs: FusedRhs::Imm(0),
                op,
                jump_when: true,
                taken,
                fallthrough: pc + 1,
            }))
        }
        // `if (x == nil)` / `!= nil` fused branch (offset in `sbx`).
        Opcode::BrNil | Opcode::BrNotNil => {
            let taken = rel(pc, instr.sbx() as i32, code_len).ok_or(Unsupported::BadTarget { pc })?;
            Ok(Some(Exit::NilBranch {
                reg_a: instr.a(),
                jump_when_nil: instr.opcode() == Opcode::BrNil,
                taken,
                fallthrough: pc + 1,
            }))
        }
        // `if (r_a % k == 0)` / `!= 0` fused divisibility branch.
        Opcode::BrModEqZeroIntI4 | Opcode::BrModNeZeroIntI4 => {
            let taken = rel(pc, instr.branch_i4_offset() as i32, code_len).ok_or(Unsupported::BadTarget { pc })?;
            let op = if instr.opcode() == Opcode::BrModEqZeroIntI4 {
                CmpOp::Eq
            } else {
                CmpOp::Ne
            };
            Ok(Some(Exit::FusedModZero {
                reg_a: instr.a(),
                divisor: i64::from(instr.branch_i4_immediate()),
                op,
                taken,
                fallthrough: pc + 1,
            }))
        }
        Opcode::Test | Opcode::BrFalse | Opcode::BrTrue => {
            let relative = match instr.opcode() {
                Opcode::Test => rel(pc, instr.c() as i8 as i32, code_len),
                _ => rel(pc, instr.sbx() as i32, code_len),
            }
            .ok_or(Unsupported::BadTarget { pc })?;
            let fallthrough = pc + 1;
            let then_pc =
                if matches!(instr.opcode(), Opcode::Test if instr.b() == 0) || instr.opcode() == Opcode::BrTrue {
                    relative
                } else {
                    fallthrough
                };
            let else_pc =
                if matches!(instr.opcode(), Opcode::Test if instr.b() != 0) || instr.opcode() == Opcode::BrFalse {
                    relative
                } else {
                    fallthrough
                };
            Ok(Some(Exit::Cond {
                cond: instr.a(),
                then_pc,
                else_pc,
            }))
        }
        _ => Ok(None),
    }
}

pub(crate) fn rel(pc: usize, offset: i32, code_len: usize) -> Option<usize> {
    let target = pc as i64 + 1 + offset as i64;
    if target < 0 || target as usize > code_len {
        None
    } else {
        Some(target as usize)
    }
}
