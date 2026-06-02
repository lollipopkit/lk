use crate::{
    llvm::{
        ir_text::next_tmp,
        scalar::facts::{NativeScalarFacts, NativeScalarKind},
        straightline_value::NativeBuiltin,
    },
    vm::Instr32,
};

pub(in crate::llvm) fn emit_runtime_builtin_call(
    ir: &mut String,
    builtin: NativeBuiltin,
    instr: Instr32,
    register_count: usize,
    facts: &NativeScalarFacts,
    pc: usize,
    tmp_index: &mut usize,
) -> bool {
    match builtin {
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
