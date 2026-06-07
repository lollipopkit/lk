use crate::{
    llvm::{
        scalar::facts::{
            analysis::{function_has_self_recursive_call_direct, global_kinds_from_fns},
            native_scalar_block_facts_with_initial,
            returns::peek_recursive_function_base_return_kind,
        },
        scalar::kind::{NativeScalarFacts, NativeScalarKind},
    },
    vm::{ConstHeapValueData, FunctionData, Instr},
};

#[allow(clippy::too_many_arguments)]
pub(in crate::llvm) fn native_scalar_block_facts_with_statics_and_functions(
    register_count: usize,
    global_count: usize,
    global_names: &[String],
    int_consts: &[i64],
    strings: &[String],
    heap_values: &[ConstHeapValueData],
    code: &[Instr],
    functions: Option<&[FunctionData]>,
) -> Option<NativeScalarFacts> {
    if let Some(facts) = native_scalar_block_facts_with_initial(
        register_count,
        global_count,
        global_names,
        int_consts,
        strings,
        heap_values,
        code,
        vec![None; register_count],
        vec![None; register_count],
        vec![None; global_count],
        vec![None; global_count],
        functions,
        &[],
        0,
        &[],
    ) {
        return Some(facts);
    }
    let Some(all_functions) = functions else {
        return None;
    };
    let mut hints: Vec<(u16, Option<NativeScalarKind>)> = Vec::new();
    for (func_idx, function) in all_functions.iter().enumerate() {
        if !function_has_self_recursive_call_direct(function, all_functions, func_idx as u16) {
            continue;
        }
        let hint = peek_recursive_function_base_return_kind(
            all_functions,
            func_idx as u16,
            global_count,
            global_names,
            global_kinds_from_fns(global_count),
        );
        hints.push((func_idx as u16, hint));
    }
    if hints.is_empty() {
        return None;
    }
    if hints.iter().any(|(_, kind)| kind.is_some()) {
        let resolved: Vec<_> = hints.iter().filter_map(|(i, k)| k.map(|k| (*i, Some(k)))).collect();
        if let Some(facts) = native_scalar_block_facts_with_initial(
            register_count,
            global_count,
            global_names,
            int_consts,
            strings,
            heap_values,
            code,
            vec![None; register_count],
            vec![None; register_count],
            vec![None; global_count],
            vec![None; global_count],
            functions,
            &[],
            0,
            &resolved,
        ) {
            return Some(facts);
        }
    }
    let i64_hints: Vec<_> = hints
        .iter()
        .map(|(idx, _)| (*idx, Some(NativeScalarKind::I64)))
        .collect();
    native_scalar_block_facts_with_initial(
        register_count,
        global_count,
        global_names,
        int_consts,
        strings,
        heap_values,
        code,
        vec![None; register_count],
        vec![None; register_count],
        vec![None; global_count],
        vec![None; global_count],
        functions,
        &[],
        0,
        &i64_hints,
    )
}
