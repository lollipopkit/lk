use super::*;

pub(super) struct CallFactsContext<'a> {
    pub(super) kinds: &'a mut [Option<NativeScalarKind>],
    pub(super) static_values: &'a mut [Option<NativeStraightlineValue>],
    pub(super) global_kinds: &'a [Option<NativeScalarKind>],
    pub(super) static_globals: &'a [Option<NativeStraightlineValue>],
    pub(super) global_count: usize,
    pub(super) global_names: &'a [String],
    pub(super) functions: Option<&'a [FunctionData]>,
    pub(super) code: &'a [Instr],
    pub(super) int_consts: &'a [i64],
    pub(super) strings: &'a [String],
    pub(super) heap_values: &'a [ConstHeapValueData],
    pub(super) pc: usize,
    pub(super) depth: usize,
    pub(super) recursive_hints: &'a [(u16, Option<NativeScalarKind>)],
}

pub(super) fn propagate_call_opcode(ctx: &mut CallFactsContext<'_>, instr: Instr) -> Option<()> {
    match instr.opcode() {
        Opcode::Call => propagate_call(ctx, instr),
        Opcode::CallDirect => propagate_call_direct(ctx, instr),
        Opcode::CallNamed => propagate_call_named(ctx, instr),
        _ => None,
    }
}

fn propagate_call(ctx: &mut CallFactsContext<'_>, instr: Instr) -> Option<()> {
    if instr.a() != instr.b() {
        return None;
    }
    let Some(target) = static_kind(ctx.static_values, instr.b()) else {
        ctx.kinds[instr.a() as usize] = None;
        ctx.static_values[instr.a() as usize] = None;
        return Some(());
    };
    if let Some((function_index, captures)) = static_call_target(&target) {
        let function_index = u8::try_from(function_index).ok()?;
        let direct_instr = Instr::abc(Opcode::CallDirect, instr.a(), function_index, instr.c());
        let kind = native_direct_call_return_kind(
            ctx.functions?,
            direct_instr,
            ctx.kinds,
            ctx.static_values,
            ctx.global_kinds,
            ctx.static_globals,
            ctx.global_count,
            ctx.global_names,
            &captures,
            ctx.depth,
            ctx.recursive_hints,
        )?;
        return set_native_kind(ctx.kinds, ctx.static_values, instr.a(), kind).then_some(());
    }
    let start = instr.b() as usize + 1;
    let end = start.checked_add(instr.c() as usize)?;
    if let Some(ok) = propagate_dynamic_map_call(ctx.kinds, ctx.static_values, instr, ctx.pc, &target, start) {
        return ok.then_some(());
    }
    if let Some(ok) =
        propagate_dynamic_ptr_list_builtin_call(ctx.kinds, ctx.static_values, instr, ctx.pc, &target, start)
    {
        return ok.then_some(());
    }
    if let Some(ok) = propagate_dynamic_i64_list_builtin_call(
        ctx.kinds,
        ctx.static_values,
        ctx.code,
        ctx.heap_values,
        instr,
        ctx.pc,
        &target,
        start,
    ) {
        return ok.then_some(());
    }
    if let Some(ok) =
        propagate_dynamic_f64_list_builtin_call(ctx.kinds, ctx.static_values, instr, ctx.pc, &target, start)
    {
        return ok.then_some(());
    }
    let Some(args) = ctx.static_values.get(start..end) else {
        let kind = native_builtin_return_kind_dynamic(&target, instr.c())?;
        return set_native_kind(ctx.kinds, ctx.static_values, instr.a(), kind).then_some(());
    };
    let args_vec: Vec<_> = args.iter().cloned().collect();
    if args_vec.iter().any(|arg| arg.is_none()) {
        let recovered = (start..end)
            .map(|reg| {
                let reg = u8::try_from(reg).ok()?;
                ctx.static_values
                    .get(reg as usize)
                    .cloned()
                    .flatten()
                    .or_else(|| local_static_heap_const_before(ctx.code, ctx.heap_values, ctx.pc, reg))
                    .or_else(|| local_static_object_before(ctx.static_values, ctx.code, ctx.int_consts, ctx.pc, reg))
                    .or_else(|| {
                        local_static_i64_value_before(
                            ctx.code,
                            ctx.int_consts,
                            ctx.strings,
                            ctx.heap_values,
                            ctx.pc,
                            reg,
                        )
                    })
                    .or_else(|| local_static_i64_before(ctx.code, ctx.int_consts, ctx.pc, reg))
            })
            .collect::<Option<Vec<_>>>();
        if let Some(args_vec) = recovered
            && let Some(kind) = native_builtin_return_kind(target.clone(), &args_vec)
        {
            return set_native_kind(ctx.kinds, ctx.static_values, instr.a(), kind).then_some(());
        }
        let kind = native_builtin_return_kind_dynamic(&target, instr.c())?;
        return set_native_kind(ctx.kinds, ctx.static_values, instr.a(), kind).then_some(());
    }
    let args_vec: Vec<NativeStraightlineValue> = args_vec.into_iter().map(|arg| arg.unwrap()).collect();
    if matches!(target, NativeStraightlineValue::Builtin(NativeBuiltin::CoreCallMethod)) {
        let mut tmp_index = 0usize;
        if let Some(value) = emit_native_static_core_call_method(&args_vec, &mut tmp_index) {
            let kind = static_value_kind(&value);
            return set_static_value(ctx.kinds, ctx.static_values, instr.a(), kind, value).then_some(());
        }
        if let Some(ok) = propagate_dynamic_string_list_method_call(ctx.kinds, ctx.static_values, instr, &args_vec)
            .or_else(|| propagate_dynamic_i64_list_method_call(ctx.kinds, ctx.static_values, instr, &args_vec))
            .or_else(|| propagate_dynamic_f64_list_method_call(ctx.kinds, ctx.static_values, instr, &args_vec))
        {
            return ok.then_some(());
        }
        if let Some(kind) = dynamic_map_get_method_kind(&args_vec) {
            return set_native_kind(ctx.kinds, ctx.static_values, instr.a(), kind).then_some(());
        }
    }
    if let Some(value) = match target.clone() {
        NativeStraightlineValue::Builtin(NativeBuiltin::CoreSet) => {
            let [arg] = args_vec.as_slice() else {
                return None;
            };
            native_static_set_from_arg(arg, format!("@lk_set_{}", ctx.pc))
        }
        NativeStraightlineValue::Builtin(NativeBuiltin::MapSet) => emit_native_map_set(&args_vec),
        NativeStraightlineValue::Builtin(NativeBuiltin::MapMutate) => {
            let [target, callable] = args_vec.as_slice() else {
                return None;
            };
            native_static_map_mutate(
                ctx.functions?,
                target.clone(),
                callable.clone(),
                format!("@lk_map_mutate_{}", ctx.pc),
            )
        }
        NativeStraightlineValue::Builtin(builtin) => emit_native_static_parse_builtin(builtin, &args_vec),
        _ => None,
    } {
        let kind = static_value_kind(&value);
        return set_static_value(ctx.kinds, ctx.static_values, instr.a(), kind, value).then_some(());
    }
    if matches!(target, NativeStraightlineValue::Builtin(NativeBuiltin::CoreCallMethod))
        && let [
            list,
            NativeStraightlineValue::String { value: method, .. },
            NativeStraightlineValue::ArgList { elements },
        ] = args_vec.as_slice()
        && method == "map"
        && matches!(
            elements.as_slice(),
            [NativeStraightlineValue::Function(_) | NativeStraightlineValue::Closure { .. }]
        )
    {
        let kind = static_value_kind(list);
        return set_static_value(ctx.kinds, ctx.static_values, instr.a(), kind, list.clone()).then_some(());
    }
    let kind = native_builtin_return_kind(target, &args_vec)?;
    set_native_kind(ctx.kinds, ctx.static_values, instr.a(), kind).then_some(())
}

fn propagate_call_direct(ctx: &mut CallFactsContext<'_>, instr: Instr) -> Option<()> {
    let callee_index = instr.b();
    if let Some((_, hint)) = ctx.recursive_hints.iter().find(|(idx, _)| *idx as u8 == callee_index) {
        if let Some(kind) = hint {
            let value = kind_symbolic_value(*kind, instr.a());
            return set_static_value(ctx.kinds, ctx.static_values, instr.a(), Some(*kind), value).then_some(());
        }
        ctx.kinds[instr.a() as usize] = None;
        ctx.static_values[instr.a() as usize] = None;
        return Some(());
    }
    if let Some(value) = native_direct_call_static_return_value(
        ctx.functions?,
        instr,
        ctx.static_values,
        ctx.code,
        ctx.int_consts,
        ctx.pc,
        &[],
        ctx.depth,
    ) {
        let kind = static_value_kind(&value);
        return set_static_value(ctx.kinds, ctx.static_values, instr.a(), kind, value).then_some(());
    }
    if let Some(callee) = ctx.functions?.get(callee_index as usize) {
        let start = instr.a().checked_add(1)? as usize;
        let end = start.checked_add(instr.c() as usize)?;
        let args = ctx.static_values.get(start..end)?;
        if let Some(value) = dynamic_list_return_value(callee, args, ctx.pc) {
            return set_static_value(ctx.kinds, ctx.static_values, instr.a(), None, value).then_some(());
        }
    }
    let kind = native_direct_call_return_kind(
        ctx.functions?,
        instr,
        ctx.kinds,
        ctx.static_values,
        ctx.global_kinds,
        ctx.static_globals,
        ctx.global_count,
        ctx.global_names,
        &[],
        ctx.depth,
        ctx.recursive_hints,
    )?;
    set_native_kind(ctx.kinds, ctx.static_values, instr.a(), kind).then_some(())
}

fn propagate_call_named(ctx: &mut CallFactsContext<'_>, instr: Instr) -> Option<()> {
    let Some(target) = static_kind(ctx.static_values, instr.a()) else {
        ctx.kinds[instr.a() as usize] = None;
        ctx.static_values[instr.a() as usize] = None;
        return Some(());
    };
    let Some((function_index, captures)) = static_call_target(&target) else {
        ctx.kinds[instr.a() as usize] = None;
        ctx.static_values[instr.a() as usize] = None;
        return Some(());
    };
    let function = ctx.functions?.get(function_index as usize)?;
    let args = native_named_call_args(
        function,
        ctx.kinds,
        ctx.static_values,
        instr.a(),
        instr.bx() & 0x7f,
        instr.bx() >> 7,
    )?;
    if let Some((_, hint)) = ctx.recursive_hints.iter().find(|(idx, _)| *idx == function_index) {
        if let Some(kind) = hint {
            let value = kind_symbolic_value(*kind, instr.a());
            return set_static_value(ctx.kinds, ctx.static_values, instr.a(), Some(*kind), value).then_some(());
        }
        ctx.kinds[instr.a() as usize] = None;
        ctx.static_values[instr.a() as usize] = None;
        return Some(());
    }
    let kind = native_static_function_return_kind(
        ctx.functions?,
        function_index as usize,
        &args,
        &captures,
        ctx.global_kinds,
        ctx.static_globals,
        ctx.global_count,
        ctx.global_names,
        ctx.depth,
        ctx.recursive_hints,
    )?;
    set_native_kind(ctx.kinds, ctx.static_values, instr.a(), kind).then_some(())
}
