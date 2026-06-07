use crate::{
    llvm::{
        ir_text::{native_label, native_relative_target, next_tmp, reg_in_bounds},
        scalar::{
            block_helpers::{local_register_kind_before, mark_static_untaken_return_path, three_regs_in_bounds},
            facts::{NativeScalarFacts, NativeScalarKind},
        },
        straightline_value::{NativeStraightlineValue, native_static_truthy},
    },
    vm::{Instr, Opcode, analysis::PerfForLoopFact},
};

pub(super) fn emit_test_block(
    ir: &mut String,
    skip_static_pcs: &mut [bool],
    static_boundaries: &[bool],
    static_regs: &[Option<NativeStraightlineValue>],
    code: &[Instr],
    pc: usize,
    instr: Instr,
    register_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> bool {
    if !reg_in_bounds(register_count, instr.a()) {
        return false;
    }
    if matches!(instr.opcode(), Opcode::BrNil | Opcode::BrNotNil) {
        return emit_nil_branch_block(
            ir,
            skip_static_pcs,
            static_boundaries,
            static_regs,
            code,
            pc,
            instr,
            facts,
            tmp_index,
        );
    }
    if let Some(value) = static_regs
        .get(instr.a() as usize)
        .and_then(|value| value.as_ref())
        .and_then(native_static_truthy)
    {
        let Some((truthy_target, falsy_target)) = test_targets(pc, instr, code.len()) else {
            return false;
        };
        let target = if value { truthy_target } else { falsy_target };
        let untaken = if value { falsy_target } else { truthy_target };
        mark_static_untaken_return_path(skip_static_pcs, static_boundaries, code, untaken);
        ir.push_str(&format!("  br label {}\n", native_label(target, code.len())));
        return true;
    }
    let Some(kind) = facts
        .register_kind_before(pc, instr.a())
        .or_else(|| local_register_kind_before(code, pc, instr.a()))
    else {
        return false;
    };
    let Some((truthy_target, falsy_target)) = test_targets(pc, instr, code.len()) else {
        return false;
    };
    match kind {
        NativeScalarKind::Bool => {
            let value = next_tmp(tmp_index);
            let cond = next_tmp(tmp_index);
            ir.push_str(&format!("  {value} = load i64, ptr %r{}.slot\n", instr.a()));
            ir.push_str(&format!("  {cond} = icmp ne i64 {value}, 0\n"));
            ir.push_str(&format!(
                "  br i1 {cond}, label {}, label {}\n",
                native_label(truthy_target, code.len()),
                native_label(falsy_target, code.len())
            ));
        }
        NativeScalarKind::Nil => {
            ir.push_str(&format!("  br label {}\n", native_label(falsy_target, code.len())));
        }
        NativeScalarKind::I64
        | NativeScalarKind::F64
        | NativeScalarKind::StrPtr
        | NativeScalarKind::MaybeI64
        | NativeScalarKind::MaybeStrPtr => {
            ir.push_str(&format!("  br label {}\n", native_label(truthy_target, code.len())));
        }
    }
    true
}

pub(super) fn emit_compare_test_block(
    ir: &mut String,
    _skip_static_pcs: &mut [bool],
    _static_boundaries: &[bool],
    _static_regs: &[Option<NativeStraightlineValue>],
    code: &[Instr],
    pc: usize,
    instr: Instr,
    register_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> bool {
    if !reg_in_bounds(register_count, instr.a()) || !reg_in_bounds(register_count, instr.b()) {
        return false;
    }
    let Some((taken, fallthrough)) = compare_test_targets(pc, instr, code) else {
        return false;
    };
    let Some(lhs_kind) = facts
        .register_kind_before(pc, instr.a())
        .or_else(|| local_register_kind_before(code, pc, instr.a()))
    else {
        return false;
    };
    let Some(rhs_kind) = facts
        .register_kind_before(pc, instr.b())
        .or_else(|| local_register_kind_before(code, pc, instr.b()))
    else {
        return false;
    };
    let cond = next_tmp(tmp_index);
    if lhs_kind == NativeScalarKind::I64 && rhs_kind == NativeScalarKind::I64 {
        let Some(pred) = compare_test_i64_pred(instr.opcode()) else {
            return false;
        };
        let lhs = next_tmp(tmp_index);
        let rhs = next_tmp(tmp_index);
        ir.push_str(&format!("  {lhs} = load i64, ptr %r{}.slot\n", instr.a()));
        ir.push_str(&format!("  {rhs} = load i64, ptr %r{}.slot\n", instr.b()));
        ir.push_str(&format!("  {cond} = icmp {pred} i64 {lhs}, {rhs}\n"));
    } else if lhs_kind == NativeScalarKind::F64 && rhs_kind == NativeScalarKind::F64 {
        let Some(pred) = compare_test_f64_pred(instr.opcode()) else {
            return false;
        };
        let lhs = next_tmp(tmp_index);
        let rhs = next_tmp(tmp_index);
        ir.push_str(&format!("  {lhs} = load double, ptr %r{}.slot\n", instr.a()));
        ir.push_str(&format!("  {rhs} = load double, ptr %r{}.slot\n", instr.b()));
        ir.push_str(&format!("  {cond} = fcmp {pred} double {lhs}, {rhs}\n"));
    } else if lhs_kind == NativeScalarKind::Bool
        && rhs_kind == NativeScalarKind::Bool
        && matches!(instr.opcode(), Opcode::TestEqInt | Opcode::TestNeInt)
    {
        let lhs = next_tmp(tmp_index);
        let rhs = next_tmp(tmp_index);
        let pred = if instr.opcode() == Opcode::TestEqInt {
            "eq"
        } else {
            "ne"
        };
        ir.push_str(&format!("  {lhs} = load i64, ptr %r{}.slot\n", instr.a()));
        ir.push_str(&format!("  {rhs} = load i64, ptr %r{}.slot\n", instr.b()));
        ir.push_str(&format!("  {cond} = icmp {pred} i64 {lhs}, {rhs}\n"));
    } else {
        return false;
    }
    let branch_cond = if instr.c() != 0 {
        cond
    } else {
        let inverted = next_tmp(tmp_index);
        ir.push_str(&format!("  {inverted} = xor i1 {cond}, true\n"));
        inverted
    };
    ir.push_str(&format!(
        "  br i1 {branch_cond}, label {}, label {}\n",
        native_label(taken, code.len()),
        native_label(fallthrough, code.len())
    ));
    true
}

fn compare_test_targets(pc: usize, _instr: Instr, code: &[Instr]) -> Option<(usize, usize)> {
    let jmp = code.get(pc + 1).copied()?;
    if jmp.opcode() != Opcode::Jmp {
        return None;
    }
    Some((native_relative_target(pc + 1, jmp.sj_arg(), code.len())?, pc + 2))
}

fn compare_test_i64_pred(opcode: Opcode) -> Option<&'static str> {
    Some(match opcode {
        Opcode::TestEqInt => "eq",
        Opcode::TestNeInt => "ne",
        Opcode::TestLtInt => "slt",
        Opcode::TestLeInt => "sle",
        Opcode::TestGtInt => "sgt",
        Opcode::TestGeInt => "sge",
        _ => return None,
    })
}

fn compare_test_f64_pred(opcode: Opcode) -> Option<&'static str> {
    Some(match opcode {
        Opcode::TestEqInt => "oeq",
        Opcode::TestNeInt => "une",
        Opcode::TestLtInt => "olt",
        Opcode::TestLeInt => "ole",
        Opcode::TestGtInt => "ogt",
        Opcode::TestGeInt => "oge",
        _ => return None,
    })
}

fn emit_nil_branch_block(
    ir: &mut String,
    skip_static_pcs: &mut [bool],
    static_boundaries: &[bool],
    static_regs: &[Option<NativeStraightlineValue>],
    code: &[Instr],
    pc: usize,
    instr: Instr,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> bool {
    let fallthrough = pc + 1;
    let Some(taken) = native_relative_target(pc, instr.sbx() as i32, code.len()) else {
        return false;
    };
    if let Some(value) = static_regs.get(instr.a() as usize).and_then(|value| value.as_ref())
        && !matches!(
            value,
            NativeStraightlineValue::MaybeI64 { .. }
                | NativeStraightlineValue::MaybeF64 { .. }
                | NativeStraightlineValue::MaybeBool { .. }
                | NativeStraightlineValue::MaybeStrPtr { .. }
        )
    {
        let is_nil = matches!(value, NativeStraightlineValue::Nil);
        let branch_taken =
            (instr.opcode() == Opcode::BrNil && is_nil) || (instr.opcode() == Opcode::BrNotNil && !is_nil);
        let target = if branch_taken { taken } else { fallthrough };
        let untaken = if branch_taken { fallthrough } else { taken };
        mark_static_untaken_return_path(skip_static_pcs, static_boundaries, code, untaken);
        ir.push_str(&format!("  br label {}\n", native_label(target, code.len())));
        return true;
    }
    let Some(kind) = facts
        .register_kind_before(pc, instr.a())
        .or_else(|| local_register_kind_before(code, pc, instr.a()))
    else {
        return false;
    };
    match kind {
        NativeScalarKind::Nil => {
            let target = if instr.opcode() == Opcode::BrNil {
                taken
            } else {
                fallthrough
            };
            ir.push_str(&format!("  br label {}\n", native_label(target, code.len())));
        }
        NativeScalarKind::MaybeI64 | NativeScalarKind::MaybeStrPtr => {
            let present = next_tmp(tmp_index);
            let cond = next_tmp(tmp_index);
            ir.push_str(&format!("  {present} = load i64, ptr %r{}.present.slot\n", instr.a()));
            if instr.opcode() == Opcode::BrNil {
                ir.push_str(&format!("  {cond} = icmp eq i64 {present}, 0\n"));
            } else {
                ir.push_str(&format!("  {cond} = icmp ne i64 {present}, 0\n"));
            }
            ir.push_str(&format!(
                "  br i1 {cond}, label {}, label {}\n",
                native_label(taken, code.len()),
                native_label(fallthrough, code.len())
            ));
        }
        NativeScalarKind::Bool | NativeScalarKind::I64 | NativeScalarKind::F64 | NativeScalarKind::StrPtr => {
            let target = if instr.opcode() == Opcode::BrNotNil {
                taken
            } else {
                fallthrough
            };
            ir.push_str(&format!("  br label {}\n", native_label(target, code.len())));
        }
    }
    true
}

pub(super) fn emit_for_loop_i_block(
    ir: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    pc: usize,
    instr: Instr,
    register_count: usize,
    fact: PerfForLoopFact,
    tmp_index: &mut usize,
) -> bool {
    if !three_regs_in_bounds(register_count, instr) {
        return false;
    }
    let Some(target) = native_relative_target(pc, fact.jump_offset, code.len()) else {
        return false;
    };
    let index = next_tmp(tmp_index);
    let end = next_tmp(tmp_index);
    let step = next_tmp(tmp_index);
    let next = next_tmp(tmp_index);
    let cond = next_tmp(tmp_index);
    ir.push_str(&format!("  {index} = load i64, ptr %r{}.slot\n", instr.a()));
    ir.push_str(&format!("  {end} = load i64, ptr %r{}.slot\n", instr.b()));
    ir.push_str(&format!("  {step} = load i64, ptr %r{}.slot\n", instr.c()));
    ir.push_str(&format!("  {next} = add i64 {index}, {step}\n"));
    ir.push_str(&format!("  store i64 {next}, ptr %r{}.slot\n", instr.a()));
    ir.push_str(&format!("  store i64 1, ptr %r{}.present.slot\n", instr.a()));
    let pred = match (fact.positive_step, fact.inclusive) {
        (true, true) => "sle",
        (true, false) => "slt",
        (false, true) => "sge",
        (false, false) => "sgt",
    };
    ir.push_str(&format!("  {cond} = icmp {pred} i64 {next}, {end}\n"));
    ir.push_str(&format!(
        "  br i1 {cond}, label {}, label {}\n",
        native_label(target, code.len()),
        native_label(pc + 1, code.len())
    ));
    static_regs[instr.a() as usize] = None;
    true
}

fn test_targets(pc: usize, instr: Instr, code_len: usize) -> Option<(usize, usize)> {
    let fallthrough = pc + 1;
    let relative = match instr.opcode() {
        Opcode::Test => native_relative_target(pc, instr.c() as i8 as i32, code_len)?,
        Opcode::BrFalse | Opcode::BrTrue => native_relative_target(pc, instr.sbx() as i32, code_len)?,
        _ => return None,
    };
    let truthy_target = if matches!(instr.opcode(), Opcode::Test if instr.b() == 0) || instr.opcode() == Opcode::BrTrue
    {
        relative
    } else {
        fallthrough
    };
    let falsy_target = if matches!(instr.opcode(), Opcode::Test if instr.b() != 0) || instr.opcode() == Opcode::BrFalse
    {
        relative
    } else {
        fallthrough
    };
    Some((truthy_target, falsy_target))
}
