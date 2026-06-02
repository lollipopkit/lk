use crate::{
    llvm::{
        ir_text::{native_label, native_relative_target, next_tmp, reg_in_bounds},
        scalar::{
            block_helpers::{local_register_kind_before, mark_static_untaken_return_path},
            facts::{NativeScalarFacts, NativeScalarKind},
        },
        straightline_value::{NativeStraightlineValue, native_static_truthy},
    },
    vm::Instr32,
};

pub(super) fn emit_test_block(
    ir: &mut String,
    skip_static_pcs: &mut [bool],
    static_boundaries: &[bool],
    static_regs: &[Option<NativeStraightlineValue>],
    code: &[Instr32],
    pc: usize,
    instr: Instr32,
    register_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> bool {
    if !reg_in_bounds(register_count, instr.a()) {
        return false;
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

fn test_targets(pc: usize, instr: Instr32, code_len: usize) -> Option<(usize, usize)> {
    let fallthrough = pc + 1;
    let relative = native_relative_target(pc, instr.c() as i8 as i32, code_len)?;
    let truthy_target = if instr.b() != 0 { fallthrough } else { relative };
    let falsy_target = if instr.b() != 0 { relative } else { fallthrough };
    Some((truthy_target, falsy_target))
}
