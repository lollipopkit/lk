use crate::val::Val;
use crate::vm::analysis::{
    PerfContainerMoveFact, PerfControlFlowFacts, PerfKeyFact, PerfLocalCopyFact, PerfRegisterCopyFact,
    PerfStringIntKeyFact, PerformanceFacts,
};
use crate::vm::bytecode::Op;

pub(crate) fn annotate_register_liveness(facts: &mut PerformanceFacts, code: &[Op], register_count: u16) {
    let live_out = compute_live_out_sets(code, register_count);
    let mut seen_write = vec![false; register_count as usize];
    let mut last_write_live_after = vec![false; register_count as usize];

    for (pc, op) in code.iter().enumerate().rev() {
        visit_op_written_registers(op, |reg| {
            if reg < register_count {
                let idx = reg as usize;
                if !seen_write[idx] {
                    last_write_live_after[idx] = live_out[pc][idx];
                    seen_write[idx] = true;
                }
            }
        });
    }

    for reg in 0..register_count {
        facts.set_register_live_after(reg, last_write_live_after[reg as usize]);
    }
}

pub(crate) fn annotate_local_copy_facts(facts: &mut PerformanceFacts, code: &[Op], register_count: u16) {
    facts.local_copies.clear();
    for (pc, op) in code.iter().enumerate() {
        let Op::StoreLocal(_, src) = *op else {
            continue;
        };
        if src >= register_count || facts.is_local_slot(src) {
            continue;
        }
        let move_source = register_dead_for_move_take(code[pc + 1..].iter(), src);
        if move_source {
            facts.set_local_copy_fact(pc, PerfLocalCopyFact { move_source });
        }
    }
}

pub(crate) fn annotate_key_facts(facts: &mut PerformanceFacts, code: &[Op], consts: &[Val], register_count: u16) {
    facts.key_ops.clear();
    let mut const_string_regs = vec![None; register_count as usize];
    let mut int_regs = vec![false; register_count as usize];
    let mut string_int_regs = vec![None; register_count as usize];
    for (pc, op) in code.iter().enumerate() {
        if facts.is_branch_target(pc) {
            const_string_regs.fill(None);
            int_regs.fill(false);
            string_int_regs.fill(None);
        }
        let string_int_fact = string_int_key_from_op(op, &const_string_regs, &int_regs);
        if let Some(fact) = string_int_fact {
            set_key_fact(
                facts,
                pc,
                PerfKeyFact {
                    const_key: None,
                    string_int: Some(fact),
                },
            );
        }
        if let Some(key_reg) = map_key_operand(op)
            && let Some(fact) = key_fact_for_reg(key_reg, &const_string_regs, &string_int_regs)
        {
            set_key_fact(facts, pc, fact);
        }
        visit_op_written_registers(op, |reg| {
            if let Some(slot) = const_string_regs.get_mut(reg as usize) {
                *slot = None;
            }
            if let Some(slot) = int_regs.get_mut(reg as usize) {
                *slot = false;
            }
            if let Some(slot) = string_int_regs.get_mut(reg as usize) {
                *slot = None;
            }
        });
        update_key_tracking(op, consts, &mut const_string_regs, &mut int_regs, &mut string_int_regs);
        if let Some(fact) = string_int_fact
            && let Some(slot) = op_written_reg(op).and_then(|reg| string_int_regs.get_mut(reg as usize))
        {
            *slot = Some(fact);
        }
        if op_is_control_boundary(op) {
            const_string_regs.fill(None);
            int_regs.fill(false);
            string_int_regs.fill(None);
        }
    }
}

pub(crate) fn apply_key_facts(code: &mut [Op], facts: &PerformanceFacts) -> bool {
    let mut changed = false;
    for (pc, op) in code.iter_mut().enumerate() {
        let Some(key) = facts
            .key_ops
            .get(pc)
            .and_then(Option::as_ref)
            .and_then(|fact| fact.const_key)
        else {
            continue;
        };
        match *op {
            Op::MapGetDynamic(dst, map, _) => {
                *op = Op::MapGetInterned(dst, map, key);
                changed = true;
            }
            Op::MapHas(dst, map, _) => {
                *op = Op::MapHasK(dst, map, key);
                changed = true;
            }
            Op::MapSet { map, val, .. } => {
                *op = Op::MapSetInterned(map, key, val);
                changed = true;
            }
            Op::MapSetMove { map, val, .. } => {
                *op = Op::MapSetInternedMove(map, key, val);
                changed = true;
            }
            _ => {}
        }
    }
    changed
}

pub(crate) fn annotate_register_copy_facts(facts: &mut PerformanceFacts, code: &[Op], register_count: u16) {
    facts.register_copies.clear();
    let live_out = compute_live_out_sets(code, register_count);
    for (pc, op) in code.iter().enumerate() {
        let Op::Move(dst, src) = *op else {
            continue;
        };
        if src >= register_count || dst == src {
            continue;
        }
        if register_dead_in_live_out(&live_out, pc, src) {
            facts.set_register_copy_fact(pc, PerfRegisterCopyFact { move_source: true });
        }
    }
}

pub(crate) fn annotate_dead_write_facts(facts: &mut PerformanceFacts, code: &[Op], register_count: u16) {
    facts.dead_writes.clear();
    let live_out = compute_live_out_sets(code, register_count);
    for (pc, op) in code.iter().enumerate() {
        let Some(dst) = elidable_dead_write_dst(op) else {
            continue;
        };
        if dst < register_count
            && register_dead_in_live_out(&live_out, pc, dst)
            && !register_may_be_captured_before_kill(&code[pc + 1..], dst)
        {
            facts.set_dead_write_fact(pc);
        }
    }
}

pub(crate) fn apply_dead_write_facts(code: &mut [Op], facts: &PerformanceFacts) -> bool {
    let mut changed = false;
    for (pc, op) in code.iter_mut().enumerate() {
        if !facts.is_dead_write(pc) {
            continue;
        }
        if elidable_dead_write_dst(op).is_none() {
            continue;
        }
        *op = Op::Nop;
        changed = true;
    }
    changed
}

fn elidable_dead_write_dst(op: &Op) -> Option<u16> {
    match *op {
        Op::LoadK(dst, _) | Op::Move(dst, _) | Op::LoadLocal(dst, _) => Some(dst),
        _ => None,
    }
}

fn register_may_be_captured_before_kill<'a>(ops: impl IntoIterator<Item = &'a Op>, reg: u16) -> bool {
    for op in ops {
        if matches!(op, Op::MakeClosure { .. }) {
            return true;
        }
        let mut written = false;
        visit_op_written_registers(op, |value| {
            written |= value == reg;
        });
        if written {
            return false;
        }
        if op_is_control_boundary(op) && !matches!(op, Op::Ret { .. }) {
            return true;
        }
    }
    false
}

fn set_key_fact(facts: &mut PerformanceFacts, pc: usize, fact: PerfKeyFact) {
    if facts.key_ops.len() <= pc {
        facts.key_ops.resize_with(pc + 1, Option::default);
    }
    facts.key_ops[pc] = Some(fact);
}

fn key_fact_for_reg(
    reg: u16,
    const_string_regs: &[Option<u16>],
    string_int_regs: &[Option<PerfStringIntKeyFact>],
) -> Option<PerfKeyFact> {
    if let Some(key) = const_string_regs.get(reg as usize).copied().flatten() {
        return Some(PerfKeyFact {
            const_key: Some(key),
            string_int: None,
        });
    }
    string_int_regs
        .get(reg as usize)
        .copied()
        .flatten()
        .map(|string_int| PerfKeyFact {
            const_key: None,
            string_int: Some(string_int),
        })
}

fn update_key_tracking(
    op: &Op,
    consts: &[Val],
    const_string_regs: &mut [Option<u16>],
    int_regs: &mut [bool],
    string_int_regs: &mut [Option<PerfStringIntKeyFact>],
) {
    match *op {
        Op::LoadK(dst, key) => {
            if let Some(slot) = const_string_regs.get_mut(dst as usize)
                && consts.get(key as usize).is_some_and(|value| value.as_str().is_some())
            {
                *slot = Some(key);
            }
            if let Some(slot) = int_regs.get_mut(dst as usize) {
                *slot = consts
                    .get(key as usize)
                    .is_some_and(|value| matches!(value, Val::Int(_)));
            }
        }
        Op::Move(dst, src) | Op::LoadLocal(dst, src) | Op::StoreLocal(dst, src) => {
            copy_tracking(dst, src, const_string_regs, int_regs, string_int_regs);
        }
        _ if op_writes_int(op) => {
            if let Some(dst) = op_written_reg(op)
                && let Some(slot) = int_regs.get_mut(dst as usize)
            {
                *slot = true;
            }
        }
        _ => {}
    }
}

fn copy_tracking(
    dst: u16,
    src: u16,
    const_string_regs: &mut [Option<u16>],
    int_regs: &mut [bool],
    string_int_regs: &mut [Option<PerfStringIntKeyFact>],
) {
    let const_key = const_string_regs.get(src as usize).copied().flatten();
    let is_int = int_regs.get(src as usize).copied().unwrap_or(false);
    let string_int = string_int_regs.get(src as usize).copied().flatten();
    if let Some(slot) = const_string_regs.get_mut(dst as usize) {
        *slot = const_key;
    }
    if let Some(slot) = int_regs.get_mut(dst as usize) {
        *slot = is_int;
    }
    if let Some(slot) = string_int_regs.get_mut(dst as usize) {
        *slot = string_int;
    }
}

fn string_int_key_from_op(
    op: &Op,
    const_string_regs: &[Option<u16>],
    int_regs: &[bool],
) -> Option<PerfStringIntKeyFact> {
    let (_dst, lhs, rhs) = match *op {
        Op::Add(dst, lhs, rhs) | Op::StrConcatKnownCap(dst, lhs, rhs) | Op::StrConcatToStr(dst, lhs, rhs) => {
            (dst, lhs, rhs)
        }
        _ => return None,
    };
    string_int_key_parts(lhs, rhs, const_string_regs, int_regs).or_else(|| {
        if matches!(op, Op::StrConcatToStr(..)) {
            None
        } else {
            string_int_key_parts(rhs, lhs, const_string_regs, int_regs)
        }
    })
}

fn string_int_key_parts(
    prefix_reg: u16,
    suffix_reg: u16,
    const_string_regs: &[Option<u16>],
    int_regs: &[bool],
) -> Option<PerfStringIntKeyFact> {
    let prefix_key = const_string_regs.get(prefix_reg as usize).copied().flatten()?;
    int_regs
        .get(suffix_reg as usize)
        .copied()
        .unwrap_or(false)
        .then_some(PerfStringIntKeyFact { prefix_key, suffix_reg })
}

fn map_key_operand(op: &Op) -> Option<u16> {
    match *op {
        Op::MapGetDynamic(_, _, key) | Op::MapHas(_, _, key) | Op::MapSet { key, .. } | Op::MapSetMove { key, .. } => {
            Some(key)
        }
        _ => None,
    }
}

fn op_written_reg(op: &Op) -> Option<u16> {
    let mut written = None;
    visit_op_written_registers(op, |reg| {
        if written.is_none() {
            written = Some(reg);
        }
    });
    written
}

fn op_writes_int(op: &Op) -> bool {
    matches!(
        *op,
        Op::AddInt(..)
            | Op::SubInt(..)
            | Op::MulInt(..)
            | Op::ModInt(..)
            | Op::AddIntImm(..)
            | Op::Floor { .. }
            | Op::Len { .. }
            | Op::ListLen { .. }
            | Op::MapLen { .. }
            | Op::StrLen { .. }
    )
}

pub(crate) fn annotate_container_move_facts(facts: &mut PerformanceFacts, code: &[Op], register_count: u16) {
    facts.container_moves.clear();
    let live_out = compute_live_out_sets(code, register_count);
    for (pc, op) in code.iter().enumerate() {
        match *op {
            Op::ListPush { list, val } if val != list && register_dead_in_live_out(&live_out, pc, val) => {
                facts.set_container_move_fact(
                    pc,
                    PerfContainerMoveFact {
                        move_key: false,
                        move_value: true,
                    },
                );
            }
            Op::MapSetInterned(map, _, val) if val != map && register_dead_in_live_out(&live_out, pc, val) => {
                facts.set_container_move_fact(
                    pc,
                    PerfContainerMoveFact {
                        move_key: false,
                        move_value: true,
                    },
                );
            }
            Op::MapSet { map, key, val }
                if map != key
                    && map != val
                    && key != val
                    && register_dead_in_live_out(&live_out, pc, key)
                    && register_dead_in_live_out(&live_out, pc, val) =>
            {
                facts.set_container_move_fact(
                    pc,
                    PerfContainerMoveFact {
                        move_key: true,
                        move_value: true,
                    },
                );
            }
            _ => {}
        }
    }
}

pub(crate) fn apply_container_move_facts(code: &mut [Op], facts: &PerformanceFacts) -> bool {
    let mut changed = false;
    for (pc, op) in code.iter_mut().enumerate() {
        let Some(fact) = facts.container_move(pc).copied() else {
            continue;
        };
        match *op {
            Op::ListPush { list, val } if fact.move_value => {
                *op = Op::ListPushMove { list, val };
                changed = true;
            }
            Op::MapSetInterned(map, key, val) if fact.move_value => {
                *op = Op::MapSetInternedMove(map, key, val);
                changed = true;
            }
            Op::MapSet { map, key, val } if fact.move_key && fact.move_value => {
                *op = Op::MapSetMove { map, key, val };
                changed = true;
            }
            _ => {}
        }
    }
    changed
}

pub(crate) fn annotate_control_flow_facts(facts: &mut PerformanceFacts, code: &[Op]) {
    let mut branch_targets = vec![false; code.len()];
    let mut block_starts = vec![false; code.len()];
    if !code.is_empty() {
        block_starts[0] = true;
    }

    for (pc, op) in code.iter().enumerate() {
        if let Some(target) = op_branch_target(pc, op)
            && target < code.len()
        {
            branch_targets[target] = true;
            block_starts[target] = true;
        }
        if op_is_control_boundary(op) && pc + 1 < code.len() {
            block_starts[pc + 1] = true;
        }
    }

    let mut block_ids = vec![0; code.len()];
    let mut current = 0u32;
    for pc in 0..code.len() {
        if pc != 0 && block_starts[pc] {
            current = current.saturating_add(1);
        }
        block_ids[pc] = current;
    }

    facts.set_control_flow_facts(PerfControlFlowFacts {
        block_ids,
        branch_targets,
    });
}

fn compute_live_out_sets(code: &[Op], register_count: u16) -> Vec<Vec<bool>> {
    let reg_count = register_count as usize;
    let mut live_in = vec![vec![false; reg_count]; code.len()];
    let mut live_out = vec![vec![false; reg_count]; code.len()];
    let mut reads = vec![Vec::<u16>::new(); code.len()];
    let mut writes = vec![Vec::<u16>::new(); code.len()];
    for (pc, op) in code.iter().enumerate() {
        visit_op_read_registers(op, |reg| {
            if reg < register_count {
                reads[pc].push(reg);
            }
        });
        visit_op_written_registers(op, |reg| {
            if reg < register_count {
                writes[pc].push(reg);
            }
        });
    }

    loop {
        let mut changed = false;
        for pc in (0..code.len()).rev() {
            let mut next_out = vec![false; reg_count];
            for succ in op_successors(code, pc) {
                for (reg, live) in live_in[succ].iter().copied().enumerate() {
                    next_out[reg] |= live;
                }
            }

            let mut next_in = next_out.clone();
            for &reg in &writes[pc] {
                next_in[reg as usize] = false;
            }
            for &reg in &reads[pc] {
                next_in[reg as usize] = true;
            }

            if next_out != live_out[pc] || next_in != live_in[pc] {
                live_out[pc] = next_out;
                live_in[pc] = next_in;
                changed = true;
            }
        }
        if !changed {
            return live_out;
        }
    }
}

fn op_successors(code: &[Op], pc: usize) -> Vec<usize> {
    if matches!(code[pc], Op::Ret { .. }) {
        return Vec::new();
    }
    let next = (pc + 1 < code.len()).then_some(pc + 1);
    let target = op_branch_target(pc, &code[pc]).filter(|target| *target < code.len());
    match code[pc] {
        Op::Jmp(_) | Op::Break(_) | Op::Continue(_) | Op::AddIntImmJmp { .. } => target.into_iter().collect(),
        Op::JmpFalse(_, _)
        | Op::BoolBranch(_, _)
        | Op::JmpIfNil(_, _)
        | Op::JmpIfNotNil(_, _)
        | Op::JmpFalseSet { .. }
        | Op::JmpTrueSet { .. }
        | Op::NullishPick { .. }
        | Op::CmpIntJmp { .. }
        | Op::CmpLtImmJmp { .. }
        | Op::CmpLeImmJmp { .. }
        | Op::CmpEqImmJmp { .. }
        | Op::CmpGtImmJmp { .. }
        | Op::CmpGeImmJmp { .. }
        | Op::CmpNeImmJmp { .. }
        | Op::JmpNilOrFalseJmp { .. }
        | Op::ForRangeLoop { .. }
        | Op::RangeLoopI { .. }
        | Op::ForRangeStep { .. } => next.into_iter().chain(target).collect(),
        _ => next.into_iter().collect(),
    }
}

fn register_dead_in_live_out(live_out: &[Vec<bool>], pc: usize, reg: u16) -> bool {
    live_out
        .get(pc)
        .and_then(|regs| regs.get(reg as usize))
        .is_none_or(|live| !*live)
}

pub(crate) fn registers_dead_after_ops<'a>(ops: impl IntoIterator<Item = &'a Op>, regs: &[u16]) -> bool {
    if regs.is_empty() {
        return true;
    }
    if regs.len() <= u128::BITS as usize {
        return registers_dead_after_ops_small(ops, regs);
    }
    let mut resolved = vec![false; regs.len()];
    let mut unresolved = regs.len();
    for op in ops {
        let mut read_before_write = false;
        visit_op_read_registers(op, |read| {
            if regs
                .iter()
                .copied()
                .enumerate()
                .any(|(idx, reg)| !resolved[idx] && reg == read)
            {
                read_before_write = true;
            }
        });
        if read_before_write {
            return false;
        }
        visit_op_written_registers(op, |written| {
            for (idx, reg) in regs.iter().copied().enumerate() {
                if !resolved[idx] && reg == written {
                    resolved[idx] = true;
                    unresolved -= 1;
                }
            }
        });
        if unresolved == 0 {
            return true;
        }
    }
    true
}

fn registers_dead_after_ops_small<'a>(ops: impl IntoIterator<Item = &'a Op>, regs: &[u16]) -> bool {
    let all_resolved = if regs.len() == u128::BITS as usize {
        u128::MAX
    } else {
        (1u128 << regs.len()) - 1
    };
    let mut resolved = 0u128;
    for op in ops {
        let mut read_before_write = false;
        visit_op_read_registers(op, |read| {
            for (idx, reg) in regs.iter().copied().enumerate() {
                let bit = 1u128 << idx;
                if (resolved & bit) == 0 && reg == read {
                    read_before_write = true;
                }
            }
        });
        if read_before_write {
            return false;
        }
        visit_op_written_registers(op, |written| {
            for (idx, reg) in regs.iter().copied().enumerate() {
                let bit = 1u128 << idx;
                if (resolved & bit) == 0 && reg == written {
                    resolved |= bit;
                }
            }
        });
        if resolved == all_resolved {
            return true;
        }
    }
    true
}

pub(crate) fn register_dead_after_ops<'a>(ops: impl IntoIterator<Item = &'a Op>, reg: u16) -> bool {
    for op in ops {
        let mut read = false;
        visit_op_read_registers(op, |value| {
            read |= value == reg;
        });
        if read {
            return false;
        }
        let mut written = false;
        visit_op_written_registers(op, |value| {
            written |= value == reg;
        });
        if written {
            return true;
        }
    }
    true
}

pub(crate) fn register_dead_for_move_take<'a>(ops: impl IntoIterator<Item = &'a Op>, reg: u16) -> bool {
    for op in ops {
        let mut read = false;
        visit_op_read_registers(op, |value| {
            read |= value == reg;
        });
        if read {
            return false;
        }
        let mut written = false;
        visit_op_written_registers(op, |value| {
            written |= value == reg;
        });
        if written {
            return true;
        }
        if matches!(op, Op::Ret { .. }) {
            return true;
        }
        if op_is_control_boundary(op) {
            return false;
        }
    }
    true
}

pub(crate) fn op_is_control_boundary(op: &Op) -> bool {
    matches!(
        op,
        Op::Jmp(_)
            | Op::JmpFalse(_, _)
            | Op::BoolBranch(_, _)
            | Op::JmpIfNil(_, _)
            | Op::JmpIfNotNil(_, _)
            | Op::JmpFalseSet { .. }
            | Op::JmpTrueSet { .. }
            | Op::NullishPick { .. }
            | Op::Break(_)
            | Op::Continue(_)
            | Op::AddIntImmJmp { .. }
            | Op::CmpIntJmp { .. }
            | Op::CmpLtImmJmp { .. }
            | Op::CmpLeImmJmp { .. }
            | Op::CmpEqImmJmp { .. }
            | Op::CmpGtImmJmp { .. }
            | Op::CmpGeImmJmp { .. }
            | Op::CmpNeImmJmp { .. }
            | Op::JmpNilOrFalseJmp { .. }
            | Op::ForRangeLoop { .. }
            | Op::RangeLoopI { .. }
            | Op::ForRangeStep { .. }
            | Op::Ret { .. }
    )
}

pub(crate) fn op_branch_target(pc: usize, op: &Op) -> Option<usize> {
    let ofs = match op {
        Op::Jmp(ofs)
        | Op::JmpFalse(_, ofs)
        | Op::JmpIfNil(_, ofs)
        | Op::JmpIfNotNil(_, ofs)
        | Op::BoolBranch(_, ofs)
        | Op::Break(ofs)
        | Op::Continue(ofs)
        | Op::AddIntImmJmp { ofs, .. }
        | Op::CmpIntJmp { ofs, .. }
        | Op::CmpLtImmJmp { ofs, .. }
        | Op::CmpLeImmJmp { ofs, .. }
        | Op::CmpEqImmJmp { ofs, .. }
        | Op::CmpGtImmJmp { ofs, .. }
        | Op::CmpGeImmJmp { ofs, .. }
        | Op::CmpNeImmJmp { ofs, .. }
        | Op::JmpNilOrFalseJmp { ofs, .. }
        | Op::RangeLoopI { ofs, .. }
        | Op::ForRangeLoop { ofs, .. } => *ofs,
        Op::JmpFalseSet { ofs, .. } | Op::JmpTrueSet { ofs, .. } | Op::NullishPick { ofs, .. } => *ofs,
        Op::ForRangeStep { back_ofs, .. } => *back_ofs,
        _ => return None,
    };
    let target = pc as isize + ofs as isize;
    (target >= 0).then_some(target as usize)
}

pub(crate) fn has_branch_target_to(code: &[Op], target: usize) -> bool {
    code.iter()
        .enumerate()
        .any(|(pc, op)| op_branch_target(pc, op) == Some(target))
}

fn visit_register_span(mut f: impl FnMut(u16), base: u16, len: u16) {
    for offset in 0..len {
        f(base.saturating_add(offset));
    }
}

pub(crate) fn op_reads_register(op: &Op, reg: u16) -> bool {
    let mut reads = false;
    visit_op_read_registers(op, |value| {
        reads |= value == reg;
    });
    reads
}

pub(crate) fn visit_op_read_registers(op: &Op, mut visit: impl FnMut(u16)) {
    match *op {
        Op::Move(_, src)
        | Op::Not(_, src)
        | Op::ToStr(_, src)
        | Op::ToBool(_, src)
        | Op::StoreLocal(_, src)
        | Op::DefineGlobal(_, src)
        | Op::LoadLocal(_, src)
        | Op::JmpIfNil(src, _)
        | Op::JmpIfNotNil(src, _)
        | Op::JmpFalse(src, _)
        | Op::BoolBranch(src, _)
        | Op::CmpLtImmJmp { r: src, .. }
        | Op::JmpNilOrFalseJmp { r: src, .. }
        | Op::CmpLeImmJmp { r: src, .. }
        | Op::CmpEqImmJmp { r: src, .. }
        | Op::CmpGtImmJmp { r: src, .. }
        | Op::CmpGeImmJmp { r: src, .. }
        | Op::CmpNeImmJmp { r: src, .. }
        | Op::AddIntImm(_, src, _)
        | Op::CmpEqImm(_, src, _)
        | Op::CmpNeImm(_, src, _)
        | Op::CmpLtImm(_, src, _)
        | Op::CmpLeImm(_, src, _)
        | Op::CmpGtImm(_, src, _)
        | Op::CmpGeImm(_, src, _)
        | Op::AccessK(_, src, _)
        | Op::IndexK(_, src, _)
        | Op::ListIndexI(_, src, _)
        | Op::StrIndexI(_, src, _)
        | Op::Len { src, .. }
        | Op::ListLen { src, .. }
        | Op::MapLen { src, .. }
        | Op::StrLen { src, .. }
        | Op::Floor { src, .. }
        | Op::FloorDivImm { src, .. }
        | Op::StartsWithK(_, src, _)
        | Op::ContainsK(_, src, _)
        | Op::MapGetInterned(_, src, _)
        | Op::MapHasK(_, src, _)
        | Op::PatternMatch { src, .. }
        | Op::PatternMatchOrFail { src, .. }
        | Op::ToIter { src, .. } => visit(src),
        Op::Ret { base, retc } => visit_register_span(&mut visit, base, retc as u16),
        Op::Add(_, a, b)
        | Op::StrConcatKnownCap(_, a, b)
        | Op::StrConcatToStr(_, a, b)
        | Op::Sub(_, a, b)
        | Op::Mul(_, a, b)
        | Op::Div(_, a, b)
        | Op::Mod(_, a, b)
        | Op::AddInt(_, a, b)
        | Op::AddFloat(_, a, b)
        | Op::SubInt(_, a, b)
        | Op::SubFloat(_, a, b)
        | Op::MulInt(_, a, b)
        | Op::MulFloat(_, a, b)
        | Op::DivFloat(_, a, b)
        | Op::ModInt(_, a, b)
        | Op::ModFloat(_, a, b)
        | Op::CmpEq(_, a, b)
        | Op::CmpNe(_, a, b)
        | Op::CmpLt(_, a, b)
        | Op::CmpLe(_, a, b)
        | Op::CmpGt(_, a, b)
        | Op::CmpGe(_, a, b)
        | Op::In(_, a, b)
        | Op::Access(_, a, b)
        | Op::MapHas(_, a, b)
        | Op::MapGetDynamic(_, a, b)
        | Op::Index { base: a, idx: b, .. }
        | Op::ListSlice { src: a, start: b, .. } => {
            visit(a);
            visit(b);
        }
        Op::CmpI { a, b, .. } | Op::CmpIntJmp { a, b, .. } => {
            visit(a);
            visit(b);
        }
        Op::NullishPick { l, .. } | Op::JmpFalseSet { r: l, .. } | Op::JmpTrueSet { r: l, .. } => visit(l),
        Op::AddRangeCountImm { idx, limit, step, .. } => {
            visit(idx);
            visit(limit);
            visit(step);
        }
        Op::ForRangePrep {
            idx,
            limit,
            step,
            explicit,
            ..
        } => {
            visit(idx);
            visit(limit);
            if explicit {
                visit(step);
            }
        }
        Op::ForRangeLoop { idx, limit, step, .. } | Op::RangeLoopI { idx, limit, step, .. } => {
            visit(idx);
            visit(limit);
            visit(step);
        }
        Op::ForRangeStep { idx, step, .. } => {
            visit(idx);
            visit(step);
        }
        Op::ListSetI { list, val, .. } => {
            visit(list);
            visit(val);
        }
        Op::BuildList { base, len, .. } | Op::BuildMap { base, len, .. } => {
            visit_register_span(&mut visit, base, len.saturating_mul(2));
        }
        Op::ListPush { list, val }
        | Op::ListPushMove { list, val }
        | Op::MapSetInterned(list, _, val)
        | Op::MapSetInternedMove(list, _, val) => {
            visit(list);
            visit(val);
        }
        Op::MapSet { map, key, val } | Op::MapSetMove { map, key, val } => {
            visit(map);
            visit(key);
            visit(val);
        }
        Op::ListFoldAdd { acc, list } => {
            visit(acc);
            visit(list);
        }
        Op::MapValuesFoldAdd { acc, map } => {
            visit(acc);
            visit(map);
        }
        Op::Call { f, base, argc, .. }
        | Op::CallExact { f, base, argc, .. }
        | Op::CallClosureExact { f, base, argc, .. }
        | Op::CallNativeFast { f, base, argc, .. } => {
            visit(f);
            visit_register_span(&mut visit, base, argc as u16);
        }
        Op::CallMethod0 { receiver, .. } => visit(receiver),
        Op::CallNamed {
            f,
            base_pos,
            posc,
            base_named,
            namedc,
            ..
        }
        | Op::CallNamedFallback {
            f,
            base_pos,
            posc,
            base_named,
            namedc,
            ..
        } => {
            visit(f);
            visit_register_span(&mut visit, base_pos, posc as u16);
            visit_register_span(&mut visit, base_named, (namedc as u16).saturating_mul(2));
        }
        _ => {}
    }
}

pub(crate) fn op_writes_register(op: &Op, reg: u16) -> bool {
    let mut writes = false;
    visit_op_written_registers(op, |value| {
        writes |= value == reg;
    });
    writes
}

pub(crate) fn visit_op_written_registers(op: &Op, mut f: impl FnMut(u16)) {
    match *op {
        Op::Nop => {}
        Op::LoadK(dst, _)
        | Op::Move(dst, _)
        | Op::Not(dst, _)
        | Op::ToStr(dst, _)
        | Op::ToBool(dst, _)
        | Op::Add(dst, _, _)
        | Op::StrConcatKnownCap(dst, _, _)
        | Op::StrConcatToStr(dst, _, _)
        | Op::Sub(dst, _, _)
        | Op::Mul(dst, _, _)
        | Op::Div(dst, _, _)
        | Op::Mod(dst, _, _)
        | Op::AddInt(dst, _, _)
        | Op::AddFloat(dst, _, _)
        | Op::AddIntImm(dst, _, _)
        | Op::SubInt(dst, _, _)
        | Op::SubFloat(dst, _, _)
        | Op::MulInt(dst, _, _)
        | Op::MulFloat(dst, _, _)
        | Op::DivFloat(dst, _, _)
        | Op::ModInt(dst, _, _)
        | Op::ModFloat(dst, _, _)
        | Op::CmpEq(dst, _, _)
        | Op::CmpNe(dst, _, _)
        | Op::CmpLt(dst, _, _)
        | Op::CmpLe(dst, _, _)
        | Op::CmpGt(dst, _, _)
        | Op::CmpGe(dst, _, _)
        | Op::CmpEqImm(dst, _, _)
        | Op::CmpNeImm(dst, _, _)
        | Op::CmpLtImm(dst, _, _)
        | Op::CmpLeImm(dst, _, _)
        | Op::CmpGtImm(dst, _, _)
        | Op::CmpGeImm(dst, _, _)
        | Op::In(dst, _, _)
        | Op::LoadLocal(dst, _)
        | Op::LoadGlobal(dst, _)
        | Op::Access(dst, _, _)
        | Op::AccessK(dst, _, _)
        | Op::IndexK(dst, _, _)
        | Op::ListIndexI(dst, _, _)
        | Op::StrIndexI(dst, _, _)
        | Op::StartsWithK(dst, _, _)
        | Op::ContainsK(dst, _, _)
        | Op::MapHas(dst, _, _)
        | Op::MapGetInterned(dst, _, _)
        | Op::MapGetDynamic(dst, _, _)
        | Op::MapHasK(dst, _, _)
        | Op::MakeClosure { dst, .. }
        | Op::PatternMatch { dst, .. }
        | Op::ToIter { dst, .. }
        | Op::BuildList { dst, .. }
        | Op::BuildMap { dst, .. }
        | Op::ListSlice { dst, .. }
        | Op::NullishPick { dst, .. }
        | Op::JmpFalseSet { dst, .. }
        | Op::JmpTrueSet { dst, .. }
        | Op::Len { dst, .. }
        | Op::ListLen { dst, .. }
        | Op::MapLen { dst, .. }
        | Op::StrLen { dst, .. }
        | Op::Floor { dst, .. }
        | Op::FloorDivImm { dst, .. }
        | Op::LoadCapture { dst, .. }
        | Op::CmpI { dst, .. }
        | Op::ListSetI { dst, .. }
        | Op::CallMethod0 { dst, .. }
        | Op::CallGlobalMethod0 { dst, .. } => f(dst),
        Op::StoreLocal(idx, _) | Op::AddIntImmJmp { r: idx, .. } | Op::AddRangeCountImm { target: idx, .. } => f(idx),
        Op::ForRangePrep {
            step, explicit: false, ..
        } => f(step),
        Op::ForRangeLoop {
            idx, write_idx: true, ..
        }
        | Op::RangeLoopI {
            idx, write_idx: true, ..
        }
        | Op::ForRangeStep { idx, .. } => f(idx),
        Op::ListPush { list, .. }
        | Op::ListPushMove { list, .. }
        | Op::MapSetInterned(list, _, _)
        | Op::MapSetInternedMove(list, _, _)
        | Op::ListFoldAdd { acc: list, .. }
        | Op::MapValuesFoldAdd { acc: list, .. } => f(list),
        Op::MapSet { map, .. } | Op::MapSetMove { map, .. } => f(map),
        Op::Call { base, retc, .. }
        | Op::CallExact { base, retc, .. }
        | Op::CallClosureExact { base, retc, .. }
        | Op::CallNativeFast { base, retc, .. } => visit_register_span(&mut f, base, retc as u16),
        Op::CallNamed { base_pos, retc, .. } | Op::CallNamedFallback { base_pos, retc, .. } => {
            visit_register_span(&mut f, base_pos, retc as u16);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use crate::val::Val;
    use crate::vm::analysis::{PerfStringIntKeyFact, PerfValueKind, PerformanceFacts};
    use crate::vm::bytecode::{IntCmpKind, Op};

    use super::{
        annotate_control_flow_facts, annotate_dead_write_facts, annotate_key_facts, annotate_local_copy_facts,
        annotate_register_copy_facts, annotate_register_liveness, apply_dead_write_facts, apply_key_facts,
        has_branch_target_to, op_branch_target, register_dead_for_move_take,
    };

    #[test]
    fn liveness_marks_last_write_live_when_read_before_overwrite() {
        let mut facts = PerformanceFacts::default();
        for reg in 0..3 {
            facts.set_register_kind(reg, PerfValueKind::Int);
        }
        let code = vec![
            Op::LoadK(0, 0),
            Op::LoadK(1, 1),
            Op::AddInt(2, 0, 1),
            Op::Ret { base: 2, retc: 1 },
        ];

        annotate_register_liveness(&mut facts, &code, 3);

        assert!(facts.register(0).is_some_and(|fact| fact.live_after));
        assert!(facts.register(1).is_some_and(|fact| fact.live_after));
        assert!(facts.register(2).is_some_and(|fact| fact.live_after));
    }

    #[test]
    fn liveness_marks_unused_last_write_dead() {
        let mut facts = PerformanceFacts::default();
        for reg in 0..2 {
            facts.set_register_kind(reg, PerfValueKind::Int);
        }
        let code = vec![Op::LoadK(0, 0), Op::LoadK(1, 1), Op::Ret { base: 1, retc: 1 }];

        annotate_register_liveness(&mut facts, &code, 2);

        assert!(!facts.register(0).is_some_and(|fact| fact.live_after));
        assert!(facts.register(1).is_some_and(|fact| fact.live_after));
    }

    #[test]
    fn liveness_handles_in_place_read_write() {
        let mut facts = PerformanceFacts::default();
        facts.set_register_kind(0, PerfValueKind::Int);
        facts.set_register_kind(1, PerfValueKind::Int);
        let code = vec![
            Op::LoadK(0, 0),
            Op::LoadK(1, 1),
            Op::AddInt(0, 0, 1),
            Op::Ret { base: 0, retc: 1 },
        ];

        annotate_register_liveness(&mut facts, &code, 2);

        assert!(facts.register(0).is_some_and(|fact| fact.live_after));
        assert!(facts.register(1).is_some_and(|fact| fact.live_after));
    }

    #[test]
    fn liveness_treats_cmp_int_jump_as_read_only() {
        let mut facts = PerformanceFacts::default();
        facts.set_register_kind(0, PerfValueKind::Int);
        facts.set_register_kind(1, PerfValueKind::Int);
        let code = vec![
            Op::LoadK(0, 0),
            Op::LoadK(1, 1),
            Op::CmpIntJmp {
                kind: IntCmpKind::Lt,
                a: 0,
                b: 1,
                ofs: 1,
            },
            Op::Ret { base: 0, retc: 1 },
        ];

        annotate_register_liveness(&mut facts, &code, 2);

        assert!(facts.register(0).is_some_and(|fact| fact.live_after));
        assert!(facts.register(1).is_some_and(|fact| fact.live_after));
    }

    #[test]
    fn register_dead_for_move_take_accepts_frame_exit_when_unread() {
        let code = vec![Op::Ret { base: 1, retc: 1 }];

        assert!(register_dead_for_move_take(code.iter(), 0));
        assert!(!register_dead_for_move_take(code.iter(), 1));
    }

    #[test]
    fn control_flow_facts_mark_branch_targets_and_blocks() {
        let mut facts = PerformanceFacts::default();
        let code = vec![
            Op::LoadK(0, 0),
            Op::CmpEqImmJmp { r: 0, imm: 1, ofs: 3 },
            Op::LoadK(1, 1),
            Op::Jmp(2),
            Op::LoadK(1, 2),
            Op::Ret { base: 1, retc: 1 },
        ];

        annotate_control_flow_facts(&mut facts, &code);

        assert_eq!(op_branch_target(1, &code[1]), Some(4));
        assert_eq!(op_branch_target(3, &code[3]), Some(5));
        assert!(has_branch_target_to(&code, 4));
        assert!(facts.is_branch_target(4));
        assert!(facts.is_branch_target(5));
        assert!(facts.same_block(0, 1));
        assert!(!facts.same_block(1, 2));
        assert!(!facts.same_block(2, 4));
        assert!(!facts.same_block(4, 5));
    }

    #[test]
    fn local_copy_facts_mark_dead_non_local_store_source_movable() {
        let mut facts = PerformanceFacts::default();
        facts.mark_local_slot(1);
        let code = vec![Op::LoadK(0, 0), Op::StoreLocal(1, 0), Op::Ret { base: 1, retc: 1 }];

        annotate_local_copy_facts(&mut facts, &code, 2);

        assert!(facts.local_copy(1).is_some_and(|fact| fact.move_source));
    }

    #[test]
    fn local_copy_facts_do_not_move_local_slot_source() {
        let mut facts = PerformanceFacts::default();
        facts.mark_local_slot(0);
        facts.mark_local_slot(1);
        let code = vec![Op::StoreLocal(1, 0), Op::Ret { base: 1, retc: 1 }];

        annotate_local_copy_facts(&mut facts, &code, 2);

        assert!(facts.local_copy(0).is_none());
    }

    #[test]
    fn key_facts_lower_const_string_key_register_to_interned_map_get() {
        let consts = vec![Val::from_str("answer")];
        let mut facts = PerformanceFacts::default();
        let mut code = vec![
            Op::LoadK(1, 0),
            Op::MapGetDynamic(2, 0, 1),
            Op::Ret { base: 2, retc: 1 },
        ];

        annotate_control_flow_facts(&mut facts, &code);
        annotate_key_facts(&mut facts, &code, &consts, 3);

        assert_eq!(
            facts
                .key_ops
                .get(1)
                .and_then(Option::as_ref)
                .and_then(|fact| fact.const_key),
            Some(0)
        );
        assert!(apply_key_facts(&mut code, &facts));
        assert!(matches!(code[1], Op::MapGetInterned(2, 0, 0)));
    }

    #[test]
    fn key_facts_track_string_int_key_registers() {
        let consts = vec![Val::from_str("sku-"), Val::Int(42)];
        let mut facts = PerformanceFacts::default();
        let code = vec![
            Op::LoadK(0, 0),
            Op::LoadK(1, 1),
            Op::StrConcatToStr(2, 0, 1),
            Op::MapGetDynamic(3, 4, 2),
            Op::Ret { base: 3, retc: 1 },
        ];

        annotate_control_flow_facts(&mut facts, &code);
        annotate_key_facts(&mut facts, &code, &consts, 5);

        let expected = PerfStringIntKeyFact {
            prefix_key: 0,
            suffix_reg: 1,
        };
        assert_eq!(
            facts
                .key_ops
                .get(2)
                .and_then(Option::as_ref)
                .and_then(|fact| fact.string_int),
            Some(expected)
        );
        assert_eq!(
            facts
                .key_ops
                .get(3)
                .and_then(Option::as_ref)
                .and_then(|fact| fact.string_int),
            Some(expected)
        );
    }

    #[test]
    fn key_facts_track_string_int_key_when_concat_overwrites_prefix() {
        let consts = vec![Val::from_str("sku-"), Val::Int(42)];
        let mut facts = PerformanceFacts::default();
        let code = vec![
            Op::LoadK(0, 0),
            Op::LoadK(1, 1),
            Op::StrConcatToStr(0, 0, 1),
            Op::MapGetDynamic(2, 3, 0),
            Op::Ret { base: 2, retc: 1 },
        ];

        annotate_control_flow_facts(&mut facts, &code);
        annotate_key_facts(&mut facts, &code, &consts, 4);

        let expected = PerfStringIntKeyFact {
            prefix_key: 0,
            suffix_reg: 1,
        };
        assert_eq!(
            facts
                .key_ops
                .get(3)
                .and_then(Option::as_ref)
                .and_then(|fact| fact.string_int),
            Some(expected)
        );
    }

    #[test]
    fn dead_write_facts_replace_dead_const_load_with_stable_noop_move() {
        let mut facts = PerformanceFacts::default();
        let mut code = vec![
            Op::LoadK(1, 0),
            Op::MapGetInterned(2, 0, 0),
            Op::Ret { base: 2, retc: 1 },
        ];

        annotate_dead_write_facts(&mut facts, &code, 3);

        assert!(facts.is_dead_write(0));
        assert!(apply_dead_write_facts(&mut code, &facts));
        assert!(matches!(code[0], Op::Nop));
        assert!(matches!(code[1], Op::MapGetInterned(2, 0, 0)));
    }

    #[test]
    fn dead_write_facts_keep_live_const_load() {
        let mut facts = PerformanceFacts::default();
        let mut code = vec![Op::LoadK(1, 0), Op::Ret { base: 1, retc: 1 }];

        annotate_dead_write_facts(&mut facts, &code, 2);

        assert!(!facts.is_dead_write(0));
        assert!(!apply_dead_write_facts(&mut code, &facts));
        assert!(matches!(code[0], Op::LoadK(1, 0)));
    }

    #[test]
    fn register_copy_facts_use_cfg_liveness_across_loop_back_edge() {
        let mut facts = PerformanceFacts::default();
        facts.mark_local_slot(0);
        let code = vec![
            Op::BuildList {
                dst: 0,
                base: 1,
                len: 0,
            },
            Op::Move(2, 0),
            Op::CallNativeFast {
                f: 3,
                base: 2,
                argc: 1,
                retc: 1,
            },
            Op::Len { dst: 4, src: 2 },
            Op::ForRangeStep {
                idx: 5,
                step: 6,
                back_ofs: -4,
            },
            Op::Ret { base: 4, retc: 1 },
        ];

        annotate_register_copy_facts(&mut facts, &code, 7);

        assert!(facts.register_copy(1).is_some_and(|fact| fact.move_source));
    }

    #[test]
    fn registers_dead_after_ops_scans_multiple_registers_once() {
        let code = vec![Op::LoadK(0, 0), Op::LoadK(1, 1), Op::Ret { base: 2, retc: 1 }];

        assert!(super::registers_dead_after_ops(code.iter(), &[0, 1]));
        assert!(!super::registers_dead_after_ops(code.iter(), &[0, 2]));
    }
}
