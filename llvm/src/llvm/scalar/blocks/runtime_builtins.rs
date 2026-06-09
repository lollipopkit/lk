use crate::{
    llvm::{
        ir_text::next_tmp,
        scalar::facts::{NativeScalarFacts, NativeScalarKind},
        straightline_value::NativeBuiltin,
    },
    vm::Instr,
};

pub(in crate::llvm) fn emit_runtime_builtin_call(
    ir: &mut String,
    builtin: NativeBuiltin,
    instr: Instr,
    register_count: usize,
    facts: &NativeScalarFacts,
    pc: usize,
    tmp_index: &mut usize,
) -> bool {
    match builtin {
        NativeBuiltin::Assert => emit_assert_call(ir, instr, register_count, facts, pc, tmp_index),
        NativeBuiltin::AssertEq | NativeBuiltin::AssertNe => {
            emit_assert_cmp_call(ir, builtin, instr, register_count, facts, pc, tmp_index)
        }
        NativeBuiltin::Panic => {
            ir.push_str("  call void @abort()\n");
            ir.push_str("  unreachable\n");
            true
        }
        NativeBuiltin::Print | NativeBuiltin::Println => {
            let is_newline = builtin == NativeBuiltin::Println;
            let arg_reg = instr.b() as usize + 1;
            if instr.c() == 0 {
                if is_newline {
                    ir.push_str("  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr @lk_nil_text)\n");
                } else {
                    ir.push_str("  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr @lk_empty_text)\n");
                }
                return true;
            }
            if instr.c() != 1 || arg_reg >= register_count {
                return false;
            }
            let kind = facts
                .register_kind_before(pc, arg_reg as u8)
                .unwrap_or(NativeScalarKind::I64);
            match kind {
                NativeScalarKind::I64 | NativeScalarKind::MaybeI64 => {
                    let value = next_tmp(tmp_index);
                    ir.push_str(&format!("  {value} = load i64, ptr %r{arg_reg}.slot\n"));
                    ir.push_str(&format!(
                        "  call i32 (ptr, ...) @printf(ptr @lk_i64_fmt, i64 {value})\n"
                    ));
                }
                NativeScalarKind::F64 => {
                    let value = next_tmp(tmp_index);
                    ir.push_str(&format!("  {value} = load double, ptr %r{arg_reg}.slot\n"));
                    ir.push_str(&format!(
                        "  call i32 (ptr, ...) @printf(ptr @lk_f64_fmt, double {value})\n"
                    ));
                }
                NativeScalarKind::Bool => {
                    let value = next_tmp(tmp_index);
                    let cond = next_tmp(tmp_index);
                    let text = next_tmp(tmp_index);
                    ir.push_str(&format!("  {value} = load i64, ptr %r{arg_reg}.slot\n"));
                    ir.push_str(&format!("  {cond} = icmp ne i64 {value}, 0\n"));
                    ir.push_str(&format!(
                        "  {text} = select i1 {cond}, ptr @lk_bool_true, ptr @lk_bool_false\n"
                    ));
                    ir.push_str(&format!("  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {text})\n"));
                }
                NativeScalarKind::Nil => {
                    ir.push_str("  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr @lk_nil_text)\n");
                }
                NativeScalarKind::StrPtr => {
                    let value = next_tmp(tmp_index);
                    ir.push_str(&format!("  {value} = load ptr, ptr %r{arg_reg}.slot\n"));
                    ir.push_str(&format!(
                        "  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {value})\n"
                    ));
                }
                NativeScalarKind::MaybeStrPtr => {
                    let present = next_tmp(tmp_index);
                    let cond = next_tmp(tmp_index);
                    let value = next_tmp(tmp_index);
                    let text = next_tmp(tmp_index);
                    ir.push_str(&format!("  {present} = load i64, ptr %r{arg_reg}.present.slot\n"));
                    ir.push_str(&format!("  {value} = load ptr, ptr %r{arg_reg}.slot\n"));
                    ir.push_str(&format!("  {cond} = icmp ne i64 {present}, 0\n"));
                    ir.push_str(&format!("  {text} = select i1 {cond}, ptr {value}, ptr @lk_nil_text\n"));
                    ir.push_str(&format!("  call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {text})\n"));
                }
            }
            true
        }
        _ => false,
    }
}

fn emit_assert_call(
    ir: &mut String,
    instr: Instr,
    register_count: usize,
    facts: &NativeScalarFacts,
    pc: usize,
    tmp_index: &mut usize,
) -> bool {
    if instr.c() != 1 && instr.c() != 2 {
        return false;
    }
    let arg_reg = instr.b() as usize + 1;
    if arg_reg >= register_count {
        return false;
    }
    let Some(cond) = emit_truthy_condition(ir, arg_reg, facts.register_kind_before(pc, arg_reg as u8), tmp_index)
    else {
        return false;
    };
    emit_assert_branch(ir, pc, &cond);
    true
}

fn emit_assert_cmp_call(
    ir: &mut String,
    builtin: NativeBuiltin,
    instr: Instr,
    register_count: usize,
    facts: &NativeScalarFacts,
    pc: usize,
    tmp_index: &mut usize,
) -> bool {
    if instr.c() != 2 && instr.c() != 3 {
        return false;
    }
    let lhs = instr.b() as usize + 1;
    let rhs = lhs + 1;
    if lhs >= register_count || rhs >= register_count {
        return false;
    }
    let lhs_kind = facts
        .register_kind_before(pc, lhs as u8)
        .unwrap_or(NativeScalarKind::I64);
    let rhs_kind = facts
        .register_kind_before(pc, rhs as u8)
        .unwrap_or(NativeScalarKind::I64);
    let Some(mut cond) = emit_scalar_equality_condition(ir, lhs, lhs_kind, rhs, rhs_kind, tmp_index) else {
        return false;
    };
    if builtin == NativeBuiltin::AssertNe {
        let inverted = next_tmp(tmp_index);
        ir.push_str(&format!("  {inverted} = xor i1 {cond}, true\n"));
        cond = inverted;
    }
    emit_assert_branch(ir, pc, &cond);
    true
}

fn emit_truthy_condition(
    ir: &mut String,
    reg: usize,
    kind: Option<NativeScalarKind>,
    tmp_index: &mut usize,
) -> Option<String> {
    match kind.unwrap_or(NativeScalarKind::Bool) {
        NativeScalarKind::Bool => {
            let value = next_tmp(tmp_index);
            let cond = next_tmp(tmp_index);
            ir.push_str(&format!("  {value} = load i64, ptr %r{reg}.slot\n"));
            ir.push_str(&format!("  {cond} = icmp ne i64 {value}, 0\n"));
            Some(cond)
        }
        NativeScalarKind::Nil => Some("false".to_string()),
        NativeScalarKind::MaybeI64 | NativeScalarKind::MaybeStrPtr => {
            let present = next_tmp(tmp_index);
            let cond = next_tmp(tmp_index);
            ir.push_str(&format!("  {present} = load i64, ptr %r{reg}.present.slot\n"));
            ir.push_str(&format!("  {cond} = icmp ne i64 {present}, 0\n"));
            Some(cond)
        }
        NativeScalarKind::I64 | NativeScalarKind::F64 | NativeScalarKind::StrPtr => Some("true".to_string()),
    }
}

fn emit_scalar_equality_condition(
    ir: &mut String,
    lhs: usize,
    lhs_kind: NativeScalarKind,
    rhs: usize,
    rhs_kind: NativeScalarKind,
    tmp_index: &mut usize,
) -> Option<String> {
    match (lhs_kind, rhs_kind) {
        (NativeScalarKind::Nil, NativeScalarKind::Nil) => Some("true".to_string()),
        (NativeScalarKind::Nil, NativeScalarKind::MaybeI64 | NativeScalarKind::MaybeStrPtr) => {
            emit_maybe_nil_condition(ir, rhs, tmp_index)
        }
        (NativeScalarKind::MaybeI64 | NativeScalarKind::MaybeStrPtr, NativeScalarKind::Nil) => {
            emit_maybe_nil_condition(ir, lhs, tmp_index)
        }
        (NativeScalarKind::I64 | NativeScalarKind::Bool, NativeScalarKind::I64 | NativeScalarKind::Bool) => {
            emit_i64_compare(ir, lhs, rhs, tmp_index)
        }
        (NativeScalarKind::MaybeI64, NativeScalarKind::I64 | NativeScalarKind::Bool) => {
            emit_maybe_i64_compare(ir, lhs, rhs, tmp_index)
        }
        (NativeScalarKind::I64 | NativeScalarKind::Bool, NativeScalarKind::MaybeI64) => {
            emit_maybe_i64_compare(ir, rhs, lhs, tmp_index)
        }
        (NativeScalarKind::MaybeI64, NativeScalarKind::MaybeI64) => {
            emit_maybe_i64_maybe_compare(ir, lhs, rhs, tmp_index)
        }
        (NativeScalarKind::F64, NativeScalarKind::F64) => emit_f64_compare(ir, lhs, rhs, tmp_index),
        (NativeScalarKind::StrPtr, NativeScalarKind::StrPtr) => emit_str_compare(ir, lhs, rhs, tmp_index),
        (NativeScalarKind::MaybeStrPtr, NativeScalarKind::StrPtr) => emit_maybe_str_compare(ir, lhs, rhs, tmp_index),
        (NativeScalarKind::StrPtr, NativeScalarKind::MaybeStrPtr) => emit_maybe_str_compare(ir, rhs, lhs, tmp_index),
        (NativeScalarKind::MaybeStrPtr, NativeScalarKind::MaybeStrPtr) => {
            emit_maybe_str_maybe_compare(ir, lhs, rhs, tmp_index)
        }
        _ => None,
    }
}

fn emit_i64_compare(ir: &mut String, lhs: usize, rhs: usize, tmp_index: &mut usize) -> Option<String> {
    let lhs_value = next_tmp(tmp_index);
    let rhs_value = next_tmp(tmp_index);
    let cond = next_tmp(tmp_index);
    ir.push_str(&format!("  {lhs_value} = load i64, ptr %r{lhs}.slot\n"));
    ir.push_str(&format!("  {rhs_value} = load i64, ptr %r{rhs}.slot\n"));
    ir.push_str(&format!("  {cond} = icmp eq i64 {lhs_value}, {rhs_value}\n"));
    Some(cond)
}

fn emit_f64_compare(ir: &mut String, lhs: usize, rhs: usize, tmp_index: &mut usize) -> Option<String> {
    let lhs_value = next_tmp(tmp_index);
    let rhs_value = next_tmp(tmp_index);
    let cond = next_tmp(tmp_index);
    ir.push_str(&format!("  {lhs_value} = load double, ptr %r{lhs}.slot\n"));
    ir.push_str(&format!("  {rhs_value} = load double, ptr %r{rhs}.slot\n"));
    ir.push_str(&format!("  {cond} = fcmp oeq double {lhs_value}, {rhs_value}\n"));
    Some(cond)
}

fn emit_str_compare(ir: &mut String, lhs: usize, rhs: usize, tmp_index: &mut usize) -> Option<String> {
    let lhs_value = next_tmp(tmp_index);
    let rhs_value = next_tmp(tmp_index);
    let cmp = next_tmp(tmp_index);
    let cond = next_tmp(tmp_index);
    ir.push_str(&format!("  {lhs_value} = load ptr, ptr %r{lhs}.slot\n"));
    ir.push_str(&format!("  {rhs_value} = load ptr, ptr %r{rhs}.slot\n"));
    ir.push_str(&format!(
        "  {cmp} = call i32 @strcmp(ptr {lhs_value}, ptr {rhs_value})\n"
    ));
    ir.push_str(&format!("  {cond} = icmp eq i32 {cmp}, 0\n"));
    Some(cond)
}

fn emit_maybe_nil_condition(ir: &mut String, reg: usize, tmp_index: &mut usize) -> Option<String> {
    let present = next_tmp(tmp_index);
    let cond = next_tmp(tmp_index);
    ir.push_str(&format!("  {present} = load i64, ptr %r{reg}.present.slot\n"));
    ir.push_str(&format!("  {cond} = icmp eq i64 {present}, 0\n"));
    Some(cond)
}

fn emit_maybe_i64_compare(ir: &mut String, maybe: usize, value: usize, tmp_index: &mut usize) -> Option<String> {
    let present = next_tmp(tmp_index);
    let present_cond = next_tmp(tmp_index);
    let maybe_value = next_tmp(tmp_index);
    let value_loaded = next_tmp(tmp_index);
    let value_cond = next_tmp(tmp_index);
    let cond = next_tmp(tmp_index);
    ir.push_str(&format!("  {present} = load i64, ptr %r{maybe}.present.slot\n"));
    ir.push_str(&format!("  {present_cond} = icmp ne i64 {present}, 0\n"));
    ir.push_str(&format!("  {maybe_value} = load i64, ptr %r{maybe}.slot\n"));
    ir.push_str(&format!("  {value_loaded} = load i64, ptr %r{value}.slot\n"));
    ir.push_str(&format!("  {value_cond} = icmp eq i64 {maybe_value}, {value_loaded}\n"));
    ir.push_str(&format!("  {cond} = and i1 {present_cond}, {value_cond}\n"));
    Some(cond)
}

fn emit_maybe_i64_maybe_compare(ir: &mut String, lhs: usize, rhs: usize, tmp_index: &mut usize) -> Option<String> {
    let lhs_present = next_tmp(tmp_index);
    let rhs_present = next_tmp(tmp_index);
    let both_missing = next_tmp(tmp_index);
    let both_present = next_tmp(tmp_index);
    let lhs_value = next_tmp(tmp_index);
    let rhs_value = next_tmp(tmp_index);
    let values_equal = next_tmp(tmp_index);
    let present_equal = next_tmp(tmp_index);
    let selected = next_tmp(tmp_index);
    let cond = next_tmp(tmp_index);
    ir.push_str(&format!("  {lhs_present} = load i64, ptr %r{lhs}.present.slot\n"));
    ir.push_str(&format!("  {rhs_present} = load i64, ptr %r{rhs}.present.slot\n"));
    ir.push_str(&format!("  {both_missing} = icmp eq i64 {lhs_present}, 0\n"));
    ir.push_str(&format!(
        "  {present_equal} = icmp eq i64 {lhs_present}, {rhs_present}\n"
    ));
    ir.push_str(&format!("  {both_present} = icmp ne i64 {lhs_present}, 0\n"));
    ir.push_str(&format!("  {lhs_value} = load i64, ptr %r{lhs}.slot\n"));
    ir.push_str(&format!("  {rhs_value} = load i64, ptr %r{rhs}.slot\n"));
    ir.push_str(&format!("  {values_equal} = icmp eq i64 {lhs_value}, {rhs_value}\n"));
    ir.push_str(&format!(
        "  {selected} = select i1 {both_present}, i1 {values_equal}, i1 {both_missing}\n"
    ));
    ir.push_str(&format!("  {cond} = and i1 {selected}, {present_equal}\n"));
    Some(cond)
}

fn emit_maybe_str_compare(ir: &mut String, maybe: usize, value: usize, tmp_index: &mut usize) -> Option<String> {
    let present = next_tmp(tmp_index);
    let present_cond = next_tmp(tmp_index);
    let maybe_value = next_tmp(tmp_index);
    let value_loaded = next_tmp(tmp_index);
    let cmp = next_tmp(tmp_index);
    let value_cond = next_tmp(tmp_index);
    let cond = next_tmp(tmp_index);
    ir.push_str(&format!("  {present} = load i64, ptr %r{maybe}.present.slot\n"));
    ir.push_str(&format!("  {present_cond} = icmp ne i64 {present}, 0\n"));
    ir.push_str(&format!("  {maybe_value} = load ptr, ptr %r{maybe}.slot\n"));
    ir.push_str(&format!("  {value_loaded} = load ptr, ptr %r{value}.slot\n"));
    ir.push_str(&format!(
        "  {cmp} = call i32 @strcmp(ptr {maybe_value}, ptr {value_loaded})\n"
    ));
    ir.push_str(&format!("  {value_cond} = icmp eq i32 {cmp}, 0\n"));
    ir.push_str(&format!("  {cond} = and i1 {present_cond}, {value_cond}\n"));
    Some(cond)
}

fn emit_maybe_str_maybe_compare(ir: &mut String, lhs: usize, rhs: usize, tmp_index: &mut usize) -> Option<String> {
    let lhs_present = next_tmp(tmp_index);
    let rhs_present = next_tmp(tmp_index);
    let both_missing = next_tmp(tmp_index);
    let both_present = next_tmp(tmp_index);
    let lhs_value = next_tmp(tmp_index);
    let rhs_value = next_tmp(tmp_index);
    let cmp = next_tmp(tmp_index);
    let values_equal = next_tmp(tmp_index);
    let present_equal = next_tmp(tmp_index);
    let selected = next_tmp(tmp_index);
    let cond = next_tmp(tmp_index);
    ir.push_str(&format!("  {lhs_present} = load i64, ptr %r{lhs}.present.slot\n"));
    ir.push_str(&format!("  {rhs_present} = load i64, ptr %r{rhs}.present.slot\n"));
    ir.push_str(&format!("  {both_missing} = icmp eq i64 {lhs_present}, 0\n"));
    ir.push_str(&format!(
        "  {present_equal} = icmp eq i64 {lhs_present}, {rhs_present}\n"
    ));
    ir.push_str(&format!("  {both_present} = icmp ne i64 {lhs_present}, 0\n"));
    ir.push_str(&format!("  {lhs_value} = load ptr, ptr %r{lhs}.slot\n"));
    ir.push_str(&format!("  {rhs_value} = load ptr, ptr %r{rhs}.slot\n"));
    ir.push_str(&format!(
        "  {cmp} = call i32 @strcmp(ptr {lhs_value}, ptr {rhs_value})\n"
    ));
    ir.push_str(&format!("  {values_equal} = icmp eq i32 {cmp}, 0\n"));
    ir.push_str(&format!(
        "  {selected} = select i1 {both_present}, i1 {values_equal}, i1 {both_missing}\n"
    ));
    ir.push_str(&format!("  {cond} = and i1 {selected}, {present_equal}\n"));
    Some(cond)
}

fn emit_assert_branch(ir: &mut String, pc: usize, cond: &str) {
    let ok_label = format!("lk_assert_ok_{pc}");
    let fail_label = format!("lk_assert_fail_{pc}");
    ir.push_str(&format!("  br i1 {cond}, label %{ok_label}, label %{fail_label}\n"));
    ir.push_str(&format!("{fail_label}:\n"));
    ir.push_str("  br label %lk_assert_fail\n");
    ir.push_str(&format!("{ok_label}:\n"));
}
