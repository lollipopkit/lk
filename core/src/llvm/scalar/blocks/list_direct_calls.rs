use crate::{
    llvm::{
        const_display::llvm_string_constant,
        ir_text::next_tmp,
        scalar::facts::{NativeScalarFacts, NativeScalarKind},
        straightline_value::{NativeListElementKind, NativeStraightlineValue},
    },
    vm::{ConstHeapValue32Data, ConstRuntimeValue32Data, Instr32},
};

pub(super) fn emit_list_direct_call(
    ir: &mut String,
    extra_globals: &mut String,
    static_regs: &mut [Option<NativeStraightlineValue>],
    instr: Instr32,
    pc: usize,
    callee_index: usize,
    facts: &NativeScalarFacts,
    tmp_index: &mut usize,
) -> Option<()> {
    let start = instr.a().checked_add(1)? as usize;
    let end = start.checked_add(instr.c() as usize)?;
    let call_args = static_regs.get(start..end)?;
    let element = call_args
        .iter()
        .filter_map(|arg| native_list_arg_element_kind(arg.as_ref()?))
        .next()?;
    let args = call_args
        .iter()
        .enumerate()
        .map(|(index, arg)| {
            let reg = u8::try_from(start + index).ok()?;
            emit_native_list_call_arg(
                ir,
                extra_globals,
                pc,
                index,
                reg,
                arg.clone(),
                element,
                facts.register_kind_before(pc, reg),
                tmp_index,
            )
        })
        .collect::<Option<Vec<_>>>()?;
    let out_base = emit_native_list_out_base(ir, pc, element, tmp_index);
    let abi = match element {
        NativeListElementKind::I64 | NativeListElementKind::Bool => {
            let profile = args
                .iter()
                .map(|arg| match arg {
                    NativeListCallArg::List { .. } => 'l',
                    NativeListCallArg::I64(_) => 'i',
                })
                .collect::<String>();
            if profile.chars().all(|kind| kind == 'l') {
                "i64_list".to_string()
            } else {
                format!("i64_list_{profile}")
            }
        }
        NativeListElementKind::F64 => "f64_list".to_string(),
        NativeListElementKind::StrPtr | NativeListElementKind::Text => "list".to_string(),
    };
    let joined_args = args
        .into_iter()
        .map(|arg| match arg {
            NativeListCallArg::List { base, len_slot } => format!("ptr {base}, ptr {len_slot}"),
            NativeListCallArg::I64(value) => format!("i64 {value}"),
        })
        .collect::<Vec<_>>()
        .join(", ");
    ir.push_str(&format!(
        "  call void @lk_fn_{callee_index}_{abi}({joined_args}, ptr {out_base}, ptr %list{pc}.len.slot)\n"
    ));
    static_regs[instr.a() as usize] = Some(NativeStraightlineValue::DynamicList { id: pc, element });
    Some(())
}

enum NativeListCallArg {
    List { base: String, len_slot: String },
    I64(String),
}

fn native_list_arg_element_kind(value: &NativeStraightlineValue) -> Option<NativeListElementKind> {
    match value {
        NativeStraightlineValue::DynamicList { element, .. } => Some(*element),
        NativeStraightlineValue::List { elements, .. } => {
            if elements
                .iter()
                .all(|value| matches!(value, ConstRuntimeValue32Data::Int(_)))
            {
                Some(NativeListElementKind::I64)
            } else if elements.iter().all(|value| match value {
                ConstRuntimeValue32Data::ShortStr(_) => true,
                ConstRuntimeValue32Data::Heap(value) => matches!(value.as_ref(), ConstHeapValue32Data::LongString(_)),
                _ => false,
            }) {
                Some(NativeListElementKind::StrPtr)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn emit_native_list_call_arg(
    ir: &mut String,
    extra_globals: &mut String,
    pc: usize,
    arg_index: usize,
    reg: u8,
    arg: Option<NativeStraightlineValue>,
    element: NativeListElementKind,
    scalar_kind: Option<NativeScalarKind>,
    tmp_index: &mut usize,
) -> Option<NativeListCallArg> {
    match arg {
        Some(NativeStraightlineValue::DynamicList { id, element: actual }) if actual == element => {
            let base = next_tmp(tmp_index);
            let (slot_field, slot_type) = native_list_slot_parts(element);
            ir.push_str(&format!(
                "  {base} = getelementptr [4096 x {slot_type}], ptr %list{id}.{slot_field}.slots, i64 0, i64 0\n"
            ));
            Some(NativeListCallArg::List {
                base,
                len_slot: format!("%list{id}.len.slot"),
            })
        }
        Some(NativeStraightlineValue::List { elements, .. }) if element == NativeListElementKind::I64 => {
            let values_name = format!("%call{pc}.arg{arg_index}.value.slots");
            let len_name = format!("%call{pc}.arg{arg_index}.len.slot");
            ir.push_str(&format!("  {len_name} = alloca i64\n"));
            ir.push_str(&format!("  {values_name} = alloca [4096 x i64]\n"));
            ir.push_str(&format!("  store i64 {}, ptr {len_name}\n", elements.len()));
            for (index, element) in elements.iter().enumerate() {
                let ConstRuntimeValue32Data::Int(value) = element else {
                    return None;
                };
                let slot = next_tmp(tmp_index);
                ir.push_str(&format!(
                    "  {slot} = getelementptr [4096 x i64], ptr {values_name}, i64 0, i64 {index}\n"
                ));
                ir.push_str(&format!("  store i64 {value}, ptr {slot}\n"));
            }
            let base = next_tmp(tmp_index);
            ir.push_str(&format!(
                "  {base} = getelementptr [4096 x i64], ptr {values_name}, i64 0, i64 0\n"
            ));
            Some(NativeListCallArg::List {
                base,
                len_slot: len_name,
            })
        }
        Some(NativeStraightlineValue::List { elements, .. }) if element != NativeListElementKind::I64 => {
            let values_name = format!("%call{pc}.arg{arg_index}.ptr.slots");
            let len_name = format!("%call{pc}.arg{arg_index}.len.slot");
            ir.push_str(&format!("  {len_name} = alloca i64\n"));
            ir.push_str(&format!("  {values_name} = alloca [4096 x ptr]\n"));
            ir.push_str(&format!("  store i64 {}, ptr {len_name}\n", elements.len()));
            for (index, element) in elements.iter().enumerate() {
                let symbol = static_string_element_symbol(extra_globals, pc, index, element)?;
                let slot = next_tmp(tmp_index);
                ir.push_str(&format!(
                    "  {slot} = getelementptr [4096 x ptr], ptr {values_name}, i64 0, i64 {index}\n"
                ));
                ir.push_str(&format!("  store ptr {symbol}, ptr {slot}\n"));
            }
            let base = next_tmp(tmp_index);
            ir.push_str(&format!(
                "  {base} = getelementptr [4096 x ptr], ptr {values_name}, i64 0, i64 0\n"
            ));
            Some(NativeListCallArg::List {
                base,
                len_slot: len_name,
            })
        }
        Some(NativeStraightlineValue::I64(value)) if element == NativeListElementKind::I64 => {
            Some(NativeListCallArg::I64(value))
        }
        None | Some(_) if element == NativeListElementKind::I64 && scalar_kind == Some(NativeScalarKind::I64) => {
            let value = next_tmp(tmp_index);
            ir.push_str(&format!("  {value} = load i64, ptr %r{reg}.slot\n"));
            Some(NativeListCallArg::I64(value))
        }
        _ => None,
    }
}

fn static_string_element_symbol(
    extra_globals: &mut String,
    pc: usize,
    index: usize,
    element: &ConstRuntimeValue32Data,
) -> Option<String> {
    match element {
        ConstRuntimeValue32Data::ShortStr(value) => {
            let symbol = format!("@lk_call{pc}_arg0_str_{index}");
            extra_globals.push_str(&llvm_string_constant(&symbol, value));
            Some(symbol)
        }
        ConstRuntimeValue32Data::Heap(value) => {
            let ConstHeapValue32Data::LongString(value) = value.as_ref() else {
                return None;
            };
            let symbol = format!("@lk_call{pc}_arg0_heap_str_{index}");
            extra_globals.push_str(&llvm_string_constant(&symbol, value));
            Some(symbol)
        }
        _ => None,
    }
}

fn emit_native_list_out_base(
    ir: &mut String,
    pc: usize,
    element: NativeListElementKind,
    tmp_index: &mut usize,
) -> String {
    let out_base = next_tmp(tmp_index);
    let (slot_field, slot_type) = native_list_slot_parts(element);
    ir.push_str(&format!(
        "  {out_base} = getelementptr [4096 x {slot_type}], ptr %list{pc}.{slot_field}.slots, i64 0, i64 0\n"
    ));
    out_base
}

fn native_list_slot_parts(element: NativeListElementKind) -> (&'static str, &'static str) {
    match element {
        NativeListElementKind::I64 | NativeListElementKind::Bool => ("value", "i64"),
        NativeListElementKind::F64 => ("f64", "double"),
        NativeListElementKind::Text | NativeListElementKind::StrPtr => ("ptr", "ptr"),
    }
}
