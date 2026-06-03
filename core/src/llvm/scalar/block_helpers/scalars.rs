use crate::llvm::{
    ir_text::next_tmp,
    scalar::facts::{NativeScalarFacts, NativeScalarKind},
    straightline_value::NativeStraightlineValue,
};

pub(in crate::llvm) fn scalar_arg_value(
    ir: &mut String,
    slot_prefix: &str,
    facts: &NativeScalarFacts,
    pc: usize,
    static_regs: &[Option<NativeStraightlineValue>],
    reg: usize,
    tmp_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    if let Some(value) = static_regs.get(reg).cloned().flatten() {
        return Some(value);
    }
    let reg = u8::try_from(reg).ok()?;
    let kind = facts.register_kind_before(pc, reg)?;
    if kind == NativeScalarKind::Nil {
        return Some(NativeStraightlineValue::Nil);
    }
    let value = next_tmp(tmp_index);
    let ty = kind.llvm_type();
    ir.push_str(&format!("  {value} = load {ty}, ptr %{slot_prefix}r{reg}.slot\n"));
    match kind {
        NativeScalarKind::I64 | NativeScalarKind::MaybeI64 => Some(NativeStraightlineValue::I64(value)),
        NativeScalarKind::F64 => Some(NativeStraightlineValue::F64(value)),
        NativeScalarKind::Bool => Some(NativeStraightlineValue::Bool(value)),
        NativeScalarKind::Nil => Some(NativeStraightlineValue::Nil),
        NativeScalarKind::StrPtr => Some(NativeStraightlineValue::StringPtr(value)),
        NativeScalarKind::MaybeStrPtr => {
            let present = next_tmp(tmp_index);
            ir.push_str(&format!(
                "  {present} = load i64, ptr %{slot_prefix}r{reg}.present.slot\n"
            ));
            Some(NativeStraightlineValue::MaybeStrPtr { value, present })
        }
    }
}

pub(in crate::llvm) fn emit_static_scalar_value_store_if_needed(
    ir: &mut String,
    reg: u8,
    value: &NativeStraightlineValue,
) -> Option<()> {
    match value {
        NativeStraightlineValue::I64(value) => {
            ir.push_str(&format!("  store i64 {value}, ptr %r{reg}.slot\n"));
            ir.push_str(&format!("  store i64 1, ptr %r{reg}.present.slot\n"));
        }
        NativeStraightlineValue::MaybeI64 { value, present } => {
            ir.push_str(&format!("  store i64 {value}, ptr %r{reg}.slot\n"));
            ir.push_str(&format!("  store i64 {present}, ptr %r{reg}.present.slot\n"));
        }
        NativeStraightlineValue::MaybeF64 { value, present } => {
            ir.push_str(&format!("  store double {value}, ptr %r{reg}.slot\n"));
            ir.push_str(&format!("  store i64 {present}, ptr %r{reg}.present.slot\n"));
        }
        NativeStraightlineValue::MaybeBool { value, present } => {
            ir.push_str(&format!("  store i64 {value}, ptr %r{reg}.slot\n"));
            ir.push_str(&format!("  store i64 {present}, ptr %r{reg}.present.slot\n"));
        }
        NativeStraightlineValue::MaybeStrPtr { value, present } => {
            ir.push_str(&format!("  store ptr {value}, ptr %r{reg}.slot\n"));
            ir.push_str(&format!("  store i64 {present}, ptr %r{reg}.present.slot\n"));
        }
        NativeStraightlineValue::Bool(value) => {
            ir.push_str(&format!("  store i64 {value}, ptr %r{reg}.slot\n"));
        }
        NativeStraightlineValue::Nil => {
            ir.push_str(&format!("  store i64 0, ptr %r{reg}.slot\n"));
        }
        NativeStraightlineValue::F64(value) => {
            ir.push_str(&format!("  store double {value}, ptr %r{reg}.slot\n"));
        }
        NativeStraightlineValue::StringPtr(value) => {
            ir.push_str(&format!("  store ptr {value}, ptr %r{reg}.slot\n"));
            ir.push_str(&format!("  store i64 1, ptr %r{reg}.present.slot\n"));
        }
        NativeStraightlineValue::String { .. }
        | NativeStraightlineValue::Text(_)
        | NativeStraightlineValue::DynamicTextChar
        | NativeStraightlineValue::DynamicSplitText { .. }
        | NativeStraightlineValue::List { .. }
        | NativeStraightlineValue::Map { .. }
        | NativeStraightlineValue::DisplayMap { .. }
        | NativeStraightlineValue::DynamicMap { .. }
        | NativeStraightlineValue::DynamicMapIter { .. }
        | NativeStraightlineValue::DynamicMapEntry { .. }
        | NativeStraightlineValue::DynamicList { .. }
        | NativeStraightlineValue::DynamicPairList { .. }
        | NativeStraightlineValue::DynamicConstListElement { .. }
        | NativeStraightlineValue::DynamicArgListElement { .. }
        | NativeStraightlineValue::DynamicJoinedText { .. }
        | NativeStraightlineValue::Channel { .. }
        | NativeStraightlineValue::ArgList { .. }
        | NativeStraightlineValue::Object { .. }
        | NativeStraightlineValue::Error { .. }
        | NativeStraightlineValue::Builtin(_)
        | NativeStraightlineValue::Module(_)
        | NativeStraightlineValue::Function(_)
        | NativeStraightlineValue::Closure { .. }
        | NativeStraightlineValue::Cell { .. } => {}
    }
    Some(())
}
