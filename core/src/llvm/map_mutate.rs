use crate::llvm::{
    const_display::native_const_list_display,
    known_key::native_known_string_key,
    straightline_value::{NativeStraightlineValue, native_runtime_const_value, native_static_set_index},
};
use crate::vm::{FunctionData, Instr, Opcode};

pub(super) fn native_static_map_mutate(
    functions: &[FunctionData],
    target: NativeStraightlineValue,
    callable: NativeStraightlineValue,
    symbol: String,
) -> Option<NativeStraightlineValue> {
    let function_index = match callable {
        NativeStraightlineValue::Function(function_index) => function_index,
        NativeStraightlineValue::Closure {
            function_index,
            captures,
        } if captures.is_empty() => function_index,
        _ => return None,
    };
    let function = functions.get(function_index as usize)?;
    if function.param_count != 1 || function.capture_count != 0 {
        return None;
    }
    let code = function
        .code
        .iter()
        .copied()
        .map(Instr::try_from_raw)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let (set_pc, set) = code
        .iter()
        .copied()
        .enumerate()
        .find(|(_, instr)| instr.opcode() == Opcode::SetIndex && instr.a() == 0)?;
    let key = native_known_string_key(function, set_pc, format!("@lk_mutate_known_key_{set_pc}"))
        .or_else(|| local_value_before(function, &code, set.b()))?;
    let value = local_value_before(function, &code, set.c())?;
    let updated = native_static_set_index(target, key, value)?;
    let elements = vec![native_runtime_const_value(&updated)?];
    Some(NativeStraightlineValue::List {
        value: native_const_list_display(&elements)?,
        symbol,
        elements,
    })
}

fn local_value_before(function: &FunctionData, code: &[Instr], reg: u8) -> Option<NativeStraightlineValue> {
    for instr in code.iter().copied() {
        if instr.a() != reg {
            continue;
        }
        return match instr.opcode() {
            Opcode::LoadNil => Some(NativeStraightlineValue::Nil),
            Opcode::LoadBool => Some(NativeStraightlineValue::Bool(i64::from(instr.b() != 0).to_string())),
            Opcode::LoadInt => function
                .consts
                .ints
                .get(instr.bx() as usize)
                .map(|value| NativeStraightlineValue::I64(value.to_string())),
            Opcode::LoadString => {
                function
                    .consts
                    .strings
                    .get(instr.bx() as usize)
                    .map(|value| NativeStraightlineValue::String {
                        symbol: String::new(),
                        value: value.clone(),
                        len: value.chars().count(),
                        key_kind: crate::llvm::straightline_value::native_runtime_string_key_kind(value),
                    })
            }
            _ => None,
        };
    }
    None
}
