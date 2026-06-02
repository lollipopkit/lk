use crate::llvm::ir_text::next_tmp;
use crate::vm::{Instr32, Opcode32};

use super::facts::NativeScalarKind;

pub(in crate::llvm) fn emit_i64_binary_block(ir: &mut String, instr: Instr32, tmp_index: &mut usize) {
    let lhs = next_tmp(tmp_index);
    let rhs = next_tmp(tmp_index);
    let out = next_tmp(tmp_index);
    let zero = next_tmp(tmp_index);
    let ok_label = format!("divisor_ok_{}", *tmp_index);
    let op = match instr.opcode() {
        Opcode32::AddInt => "add",
        Opcode32::SubInt => "sub",
        Opcode32::MulInt => "mul",
        Opcode32::DivInt => "sdiv",
        Opcode32::ModInt => "srem",
        _ => unreachable!("opcode matched by caller"),
    };
    ir.push_str(&format!("  {lhs} = load i64, ptr %r{}.slot\n", instr.b()));
    ir.push_str(&format!("  {rhs} = load i64, ptr %r{}.slot\n", instr.c()));
    if matches!(instr.opcode(), Opcode32::DivInt | Opcode32::ModInt) {
        ir.push_str(&format!("  {zero} = icmp eq i64 {rhs}, 0\n"));
        ir.push_str(&format!("  br i1 {zero}, label %lk_divisor_zero, label %{ok_label}\n"));
        ir.push_str(&format!("{ok_label}:\n"));
    }
    ir.push_str(&format!("  {out} = {op} i64 {lhs}, {rhs}\n"));
    ir.push_str(&format!("  store i64 {out}, ptr %r{}.slot\n", instr.a()));
    ir.push_str(&format!("  store i64 1, ptr %r{}.present.slot\n", instr.a()));
}

pub(in crate::llvm) fn emit_f64_binary_block(
    ir: &mut String,
    instr: Instr32,
    lhs_kind: NativeScalarKind,
    rhs_kind: NativeScalarKind,
    slot_prefix: &str,
    tmp_index: &mut usize,
) {
    let lhs = emit_numeric_load_as_f64(ir, slot_prefix, instr.b(), lhs_kind, tmp_index);
    let rhs = emit_numeric_load_as_f64(ir, slot_prefix, instr.c(), rhs_kind, tmp_index);
    let out = next_tmp(tmp_index);
    let zero = next_tmp(tmp_index);
    let ok_label = format!("divisor_ok_{}", *tmp_index);
    let op = match instr.opcode() {
        Opcode32::AddFloat => "fadd",
        Opcode32::SubFloat => "fsub",
        Opcode32::MulFloat => "fmul",
        Opcode32::DivFloat => "fdiv",
        Opcode32::ModFloat => "frem",
        _ => unreachable!("opcode matched by caller"),
    };
    if matches!(instr.opcode(), Opcode32::DivFloat | Opcode32::ModFloat) {
        ir.push_str(&format!("  {zero} = fcmp oeq double {rhs}, 0.0\n"));
        ir.push_str(&format!("  br i1 {zero}, label %lk_divisor_zero, label %{ok_label}\n"));
        ir.push_str(&format!("{ok_label}:\n"));
    }
    ir.push_str(&format!("  {out} = {op} double {lhs}, {rhs}\n"));
    ir.push_str(&format!(
        "  store double {out}, ptr %{slot_prefix}r{}.slot\n",
        instr.a()
    ));
}

fn emit_numeric_load_as_f64(
    ir: &mut String,
    slot_prefix: &str,
    reg: u8,
    kind: NativeScalarKind,
    tmp_index: &mut usize,
) -> String {
    let value = next_tmp(tmp_index);
    match kind {
        NativeScalarKind::F64 => {
            ir.push_str(&format!("  {value} = load double, ptr %{slot_prefix}r{reg}.slot\n"));
            value
        }
        NativeScalarKind::I64 => {
            let cast = next_tmp(tmp_index);
            ir.push_str(&format!("  {value} = load i64, ptr %{slot_prefix}r{reg}.slot\n"));
            ir.push_str(&format!("  {cast} = sitofp i64 {value} to double\n"));
            cast
        }
        NativeScalarKind::Bool
        | NativeScalarKind::Nil
        | NativeScalarKind::StrPtr
        | NativeScalarKind::MaybeI64
        | NativeScalarKind::MaybeStrPtr => {
            unreachable!("non-numeric float operand rejected earlier")
        }
    }
}

pub(in crate::llvm) fn emit_numeric_compare_block(
    ir: &mut String,
    instr: Instr32,
    lhs_kind: NativeScalarKind,
    rhs_kind: NativeScalarKind,
    tmp_index: &mut usize,
) {
    let cmp = next_tmp(tmp_index);
    let out = next_tmp(tmp_index);
    if lhs_kind == NativeScalarKind::I64 && rhs_kind == NativeScalarKind::I64 {
        let lhs = next_tmp(tmp_index);
        let rhs = next_tmp(tmp_index);
        ir.push_str(&format!("  {lhs} = load i64, ptr %r{}.slot\n", instr.b()));
        ir.push_str(&format!("  {rhs} = load i64, ptr %r{}.slot\n", instr.c()));
        let pred = match instr.opcode() {
            Opcode32::CmpInt => "eq",
            Opcode32::CmpNeInt => "ne",
            Opcode32::CmpLtInt => "slt",
            Opcode32::CmpLeInt => "sle",
            Opcode32::CmpGtInt => "sgt",
            Opcode32::CmpGeInt => "sge",
            _ => unreachable!("opcode matched by caller"),
        };
        ir.push_str(&format!("  {cmp} = icmp {pred} i64 {lhs}, {rhs}\n"));
    } else {
        let lhs = emit_numeric_load_as_f64(ir, "", instr.b(), lhs_kind, tmp_index);
        let rhs = emit_numeric_load_as_f64(ir, "", instr.c(), rhs_kind, tmp_index);
        let pred = match instr.opcode() {
            Opcode32::CmpInt => "oeq",
            Opcode32::CmpNeInt => "une",
            Opcode32::CmpLtInt => "olt",
            Opcode32::CmpLeInt => "ole",
            Opcode32::CmpGtInt => "ogt",
            Opcode32::CmpGeInt => "oge",
            _ => unreachable!("opcode matched by caller"),
        };
        ir.push_str(&format!("  {cmp} = fcmp {pred} double {lhs}, {rhs}\n"));
    }
    ir.push_str(&format!("  {out} = zext i1 {cmp} to i64\n"));
    ir.push_str(&format!("  store i64 {out}, ptr %r{}.slot\n", instr.a()));
}

pub(in crate::llvm) fn emit_scalar_equality_block(
    ir: &mut String,
    instr: Instr32,
    lhs_kind: NativeScalarKind,
    rhs_kind: NativeScalarKind,
    tmp_index: &mut usize,
) {
    let eq_result =
        match (lhs_kind, rhs_kind) {
            (NativeScalarKind::Bool, NativeScalarKind::Bool) => None,
            (NativeScalarKind::Bool, NativeScalarKind::I64) | (NativeScalarKind::I64, NativeScalarKind::Bool) => None,
            (NativeScalarKind::Bool, NativeScalarKind::MaybeI64)
            | (NativeScalarKind::MaybeI64, NativeScalarKind::Bool) => None,
            (NativeScalarKind::Nil, NativeScalarKind::Nil) => Some(true),
            (NativeScalarKind::MaybeI64, NativeScalarKind::Nil)
            | (NativeScalarKind::Nil, NativeScalarKind::MaybeI64) => None,
            (NativeScalarKind::I64, NativeScalarKind::Nil) | (NativeScalarKind::Nil, NativeScalarKind::I64) => None,
            (NativeScalarKind::MaybeI64, NativeScalarKind::I64)
            | (NativeScalarKind::I64, NativeScalarKind::MaybeI64) => None,
            _ => Some(false),
        };
    if let Some(equal) = eq_result {
        let value = i64::from(if instr.opcode() == Opcode32::CmpNeInt {
            !equal
        } else {
            equal
        });
        ir.push_str(&format!("  store i64 {value}, ptr %r{}.slot\n", instr.a()));
        return;
    }

    let lhs = next_tmp(tmp_index);
    let rhs = next_tmp(tmp_index);
    let cmp = next_tmp(tmp_index);
    let out = next_tmp(tmp_index);
    let pred = match instr.opcode() {
        Opcode32::CmpInt => "eq",
        Opcode32::CmpNeInt => "ne",
        _ => unreachable!("opcode matched by caller"),
    };
    if lhs_kind == NativeScalarKind::MaybeI64
        || rhs_kind == NativeScalarKind::MaybeI64
        || matches!(
            (lhs_kind, rhs_kind),
            (NativeScalarKind::I64, NativeScalarKind::Nil) | (NativeScalarKind::Nil, NativeScalarKind::I64)
        )
    {
        let maybe_reg = if lhs_kind == NativeScalarKind::MaybeI64
            || matches!((lhs_kind, rhs_kind), (NativeScalarKind::I64, NativeScalarKind::Nil))
        {
            instr.b()
        } else {
            instr.c()
        };
        let present = next_tmp(tmp_index);
        ir.push_str(&format!("  {present} = load i64, ptr %r{maybe_reg}.present.slot\n"));
        if lhs_kind == NativeScalarKind::Nil || rhs_kind == NativeScalarKind::Nil {
            let nil_equal = if instr.opcode() == Opcode32::CmpInt { "eq" } else { "ne" };
            ir.push_str(&format!("  {cmp} = icmp {nil_equal} i64 {present}, 0\n"));
        } else {
            let maybe_value = next_tmp(tmp_index);
            let other_reg = if lhs_kind == NativeScalarKind::MaybeI64 {
                instr.c()
            } else {
                instr.b()
            };
            let other_value = next_tmp(tmp_index);
            let value_eq = next_tmp(tmp_index);
            let present_ok = next_tmp(tmp_index);
            ir.push_str(&format!("  {maybe_value} = load i64, ptr %r{maybe_reg}.slot\n"));
            ir.push_str(&format!("  {other_value} = load i64, ptr %r{other_reg}.slot\n"));
            ir.push_str(&format!("  {value_eq} = icmp eq i64 {maybe_value}, {other_value}\n"));
            ir.push_str(&format!("  {present_ok} = icmp ne i64 {present}, 0\n"));
            ir.push_str(&format!("  {cmp} = and i1 {present_ok}, {value_eq}\n"));
            if instr.opcode() == Opcode32::CmpNeInt {
                let ne = next_tmp(tmp_index);
                ir.push_str(&format!("  {ne} = xor i1 {cmp}, true\n"));
                ir.push_str(&format!("  {out} = zext i1 {ne} to i64\n"));
                ir.push_str(&format!("  store i64 {out}, ptr %r{}.slot\n", instr.a()));
                return;
            }
        }
    } else {
        ir.push_str(&format!("  {lhs} = load i64, ptr %r{}.slot\n", instr.b()));
        ir.push_str(&format!("  {rhs} = load i64, ptr %r{}.slot\n", instr.c()));
        ir.push_str(&format!("  {cmp} = icmp {pred} i64 {lhs}, {rhs}\n"));
    }
    ir.push_str(&format!("  {out} = zext i1 {cmp} to i64\n"));
    ir.push_str(&format!("  store i64 {out}, ptr %r{}.slot\n", instr.a()));
}

pub(in crate::llvm) fn emit_native_return_print(
    ir: &mut String,
    pc: usize,
    register: u8,
    kind: NativeScalarKind,
    tmp_index: &mut usize,
) {
    if kind == NativeScalarKind::Nil {
        return;
    }
    let value = next_tmp(tmp_index);
    ir.push_str(&format!(
        "  {value} = load {}, ptr %r{register}.slot\n",
        kind.llvm_type()
    ));
    match kind {
        NativeScalarKind::I64 => {
            ir.push_str(&format!(
                "  %print{pc} = call i32 (ptr, ...) @printf(ptr @lk_i64_fmt, i64 {value})\n"
            ));
        }
        NativeScalarKind::F64 => {
            ir.push_str(&format!(
                "  %print{pc} = call i32 (ptr, ...) @printf(ptr @lk_f64_fmt, double {value})\n"
            ));
        }
        NativeScalarKind::Bool => {
            let cond = next_tmp(tmp_index);
            let text = next_tmp(tmp_index);
            ir.push_str(&format!("  {cond} = icmp ne i64 {value}, 0\n"));
            ir.push_str(&format!(
                "  {text} = select i1 {cond}, ptr @lk_bool_true, ptr @lk_bool_false\n"
            ));
            ir.push_str(&format!(
                "  %print{pc} = call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {text})\n"
            ));
        }
        NativeScalarKind::Nil => unreachable!("nil return is intentionally silent"),
        NativeScalarKind::StrPtr => {
            ir.push_str(&format!(
                "  %print{pc} = call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {value})\n"
            ));
        }
        NativeScalarKind::MaybeStrPtr => {
            let present = next_tmp(tmp_index);
            let cond = next_tmp(tmp_index);
            let text = next_tmp(tmp_index);
            ir.push_str(&format!("  {present} = load i64, ptr %r{register}.present.slot\n"));
            ir.push_str(&format!("  {cond} = icmp ne i64 {present}, 0\n"));
            ir.push_str(&format!("  {text} = select i1 {cond}, ptr {value}, ptr @lk_nil_text\n"));
            ir.push_str(&format!(
                "  %print{pc} = call i32 (ptr, ...) @printf(ptr @lk_str_fmt, ptr {text})\n"
            ));
        }
        NativeScalarKind::MaybeI64 => {
            ir.push_str(&format!(
                "  %print{pc} = call i32 (ptr, ...) @printf(ptr @lk_i64_fmt, i64 {value})\n"
            ));
        }
    }
}
