use crate::{
    llvm::{
        ir_text::{emit_branch_to_next, next_tmp, reg_in_bounds},
        scalar::facts::{NativeScalarFacts, NativeScalarKind},
    },
    vm::Instr,
};

pub(super) fn emit_native_assert_direct_call(
    ir: &mut String,
    instr: Instr,
    pc: usize,
    code_len: usize,
    register_count: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> Option<()> {
    if instr.c() != 1 {
        return None;
    }
    let arg = instr.a().checked_add(1)?;
    if !reg_in_bounds(register_count, arg) {
        return None;
    }
    let kind = facts.register_kind_before(pc, arg).unwrap_or(NativeScalarKind::Bool);
    let value = next_tmp(tmp_index);
    let cond = next_tmp(tmp_index);
    match kind {
        NativeScalarKind::Bool | NativeScalarKind::I64 | NativeScalarKind::MaybeI64 => {
            ir.push_str(&format!("  {value} = load i64, ptr %r{arg}.slot\n"));
            ir.push_str(&format!("  {cond} = icmp ne i64 {value}, 0\n"));
        }
        NativeScalarKind::Nil => {
            ir.push_str("  br label %lk_assert_fail\n");
            return Some(());
        }
        NativeScalarKind::F64 => {
            ir.push_str(&format!("  {value} = load double, ptr %r{arg}.slot\n"));
            ir.push_str(&format!("  {cond} = fcmp one double {value}, 0.0\n"));
        }
        NativeScalarKind::StrPtr | NativeScalarKind::MaybeStrPtr => {
            ir.push_str(&format!("  {value} = load ptr, ptr %r{arg}.slot\n"));
            ir.push_str(&format!("  {cond} = icmp ne ptr {value}, null\n"));
        }
    }
    let ok_label = format!("lk_assert_ok_{pc}");
    let fail_label = format!("lk_assert_fail_{pc}");
    ir.push_str(&format!("  br i1 {cond}, label %{ok_label}, label %{fail_label}\n"));
    ir.push_str(&format!("{fail_label}:\n"));
    ir.push_str(&format!("  call i32 (ptr, ...) @printf(ptr @lk_i64_fmt, i64 {pc})\n"));
    ir.push_str("  br label %lk_assert_fail\n");
    ir.push_str(&format!("{ok_label}:\n"));
    emit_branch_to_next(ir, pc, code_len);
    Some(())
}
