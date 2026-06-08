use crate::{
    llvm::{
        scalar::block_helpers::{local_static_i64_before, store_native_scalar_call_result},
        straightline_value::{
            NativeBuiltin, NativeStraightlineValue, native_const_runtime_value, native_runtime_const_value,
        },
    },
    vm::{ConstHeapValueData, ConstRuntimeValueData, Instr, Opcode},
};

pub(super) fn emit_static_channel_call(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    code: &[Instr],
    int_consts: &[i64],
    pc: usize,
    instr: Instr,
    builtin: NativeBuiltin,
    tmp_index: &mut usize,
) -> Option<()> {
    let arg_reg = instr.b().checked_add(1)?;
    let channel_reg = call_arg_source_reg(code, pc, arg_reg).unwrap_or(arg_reg);
    let channel = static_regs.get(channel_reg as usize)?.clone()?;
    let NativeStraightlineValue::Channel { mut elements } = channel else {
        return None;
    };
    match builtin {
        NativeBuiltin::Send => {
            if instr.c() != 2 {
                return None;
            }
            let value_reg = instr.b().checked_add(2)?;
            let value = static_regs
                .get(value_reg as usize)?
                .clone()
                .or_else(|| local_static_i64_before(code, int_consts, pc, value_reg))?;
            elements.push(native_runtime_const_value(&value)?);
            set_static_channel(static_regs, arg_reg, channel_reg, elements);
            store_native_scalar_call_result(
                ir,
                extra_globals,
                static_regs,
                instr.a(),
                NativeStraightlineValue::Nil,
                tmp_index,
            )?;
        }
        NativeBuiltin::Recv => {
            if instr.c() != 1 {
                return None;
            }
            let first = elements.first()?.clone();
            elements.remove(0);
            set_static_channel(static_regs, arg_reg, channel_reg, elements);
            let result = ConstRuntimeValueData::Heap(Box::new(ConstHeapValueData::List(vec![
                ConstRuntimeValueData::Bool(true),
                first,
            ])));
            store_native_scalar_call_result(
                ir,
                extra_globals,
                static_regs,
                instr.a(),
                native_const_runtime_value(&result, String::new())?,
                tmp_index,
            )?;
        }
        _ => return None,
    }
    Some(())
}

fn call_arg_source_reg(code: &[Instr], pc: usize, arg_reg: u8) -> Option<u8> {
    for prev_pc in (pc.saturating_sub(8)..pc).rev() {
        let prev = code.get(prev_pc).copied()?;
        if prev.a() != arg_reg {
            continue;
        }
        return (prev.opcode() == Opcode::Move).then_some(prev.b());
    }
    None
}

fn set_static_channel(
    static_regs: &mut [Option<NativeStraightlineValue>],
    arg_reg: u8,
    channel_reg: u8,
    elements: Vec<ConstRuntimeValueData>,
) {
    let value = Some(NativeStraightlineValue::Channel { elements });
    if let Some(slot) = static_regs.get_mut(channel_reg as usize) {
        *slot = value.clone();
    }
    if channel_reg != arg_reg
        && let Some(slot) = static_regs.get_mut(arg_reg as usize)
    {
        *slot = value;
    }
}
