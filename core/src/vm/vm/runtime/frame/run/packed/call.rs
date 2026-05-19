use super::super::invoke::NativeCallable;
use super::super::raw_boundary::{function_from_ptr, pop_vm_frame, push_vm_frame, region_allocator};
use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn run_call_packed(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut [Option<CallIc>],
    pc_ref: &mut usize,
    next_pc_default: usize,
    frame_base: usize,
    region_allocator_ptr: *const RegionAllocator,
    self_ptr: *mut Vm,
    rf: u16,
    base: u16,
    argc: u8,
    retc: u8,
) -> Result<Option<Val>> {
    let pc = *pc_ref;
    let resume_pc = next_pc_default;
    let start = base as usize;
    let n = argc as usize;
    let allocator = region_allocator(region_allocator_ptr);
    let ret_layout = CallReturnLayout::new(base, retc);

    let mut ic_fast_path_taken = false;
    if let Some(CallIc::ClosurePositional {
        closure_ptr,
        fun_ptr,
        argc: ic_argc,
        ret,
        tiny,
        ..
    }) = call_ic[pc].as_ref()
        && *ic_argc == argc
        && ret.matches(base, retc)
    {
        let reg_val = &regs[rf as usize];
        if let Val::Closure(arc) = reg_val {
            let closure_matches = Arc::as_ptr(arc) as usize == *closure_ptr
                || arc
                    .code
                    .get()
                    .map(|fun| std::ptr::eq(Arc::as_ptr(fun), *fun_ptr))
                    .unwrap_or(false);
            if closure_matches {
                let fun = function_from_ptr(*fun_ptr);
                let args_slice_fast = &regs[start..start + n];
                if let Some(val) = tiny
                    .as_ref()
                    .and_then(|plan| plan.try_eval(args_slice_fast, Some(&arc.captures)))
                {
                    if retc > 0 {
                        assign_reg(frame_raw, regs, base as usize, val);
                    }
                    ic_fast_path_taken = true;
                } else {
                    let return_meta = CallFrameMeta::inline_return(resume_pc, base, retc, frame_base);
                    let (captures, capture_specs) = arc.frame_captures();
                    if let Some(CallIc::ClosurePositional { cache, frame_info, .. }) = call_ic[pc].as_mut() {
                        let val = invoke_vm_closure_fast(
                            self_ptr,
                            fun,
                            RegisterSpan::new(start, n, RegisterWindowRef::Base(frame_base)),
                            ctx,
                            Some(frame_info),
                            captures,
                            capture_specs,
                            cache,
                            return_meta,
                        );
                        match val {
                            Ok(val) => {
                                if retc > 0 {
                                    assign_reg(frame_raw, regs, base as usize, val);
                                }
                            }
                            Err(err) => return frame_return_common(frame_raw, pc, Err(err)).map(Some),
                        }
                        ic_fast_path_taken = true;
                    }
                }
            }
        }
    }
    if ic_fast_path_taken {
        *pc_ref = take_pending_resume_pc(self_ptr, resume_pc);
        return Ok(None);
    }

    if let Some(callable) = NativeCallable::from_val(&regs[rf as usize]) {
        let call_result: Result<()> =
            invoke_native_callable_with_ic(ctx, regs, &mut call_ic[pc], callable, argc, ret_layout)
                .map(|handled| debug_assert!(handled));
        if let Err(err) = call_result {
            return frame_return_common(frame_raw, pc, Err(err)).map(Some);
        }
        *pc_ref = take_pending_resume_pc(self_ptr, resume_pc);
        return Ok(None);
    }

    let args_slice = &regs[start..start + n];
    let func = regs[rf as usize].clone();
    let call_args = CallArgs::registers(RegisterSpan::current(start, n));
    match &func {
        Val::Closure(closure_arc) => {
            let closure_ptr = Arc::as_ptr(closure_arc) as usize;
            let mut cached_fast = matches!(call_ic[pc].as_ref(), Some(CallIc::ClosurePositional { closure_ptr: cached_ptr, argc: cached_argc, ret, .. }) if *cached_ptr == closure_ptr && *cached_argc == argc && ret.matches(base, retc));
            let supports_fast = cached_fast || closure_arc.supports_vm_positional_fast_path();
            if supports_fast && closure_arc.named_params.is_empty() {
                if !cached_fast && args_slice.len() != closure_arc.params.len() {
                    return frame_return_common(
                        frame_raw,
                        pc,
                        Err(anyhow!(
                            "Function expects {} positional arguments, got {}",
                            closure_arc.params.len(),
                            args_slice.len()
                        )),
                    )
                    .map(Some);
                }
                let return_meta = CallFrameMeta::inline_return(resume_pc, base, retc, frame_base);
                let closure = closure_arc.as_ref();
                let mut cached_fun_ptr = None;
                if let Some(CallIc::ClosurePositional {
                    closure_ptr: cached_ptr,
                    fun_ptr,
                    argc: cached_argc,
                    ret,
                    ..
                }) = call_ic[pc].as_ref()
                    && *cached_ptr == closure_ptr
                    && *cached_argc == argc
                    && ret.matches(base, retc)
                {
                    cached_fun_ptr = Some(*fun_ptr);
                }
                let fun: &Function = if let Some(ptr) = cached_fun_ptr {
                    function_from_ptr(ptr)
                } else {
                    closure
                        .code
                        .get_or_init(|| {
                            let c = Compiler::new();
                            Arc::new(c.compile_function_with_captures(
                                closure.params.as_ref(),
                                closure.named_params.as_ref(),
                                closure.body.as_ref(),
                                closure.capture_specs.as_ref(),
                            ))
                        })
                        .as_ref()
                };
                if !cached_fast
                    && let Some(CallIc::ClosurePositional {
                        fun_ptr,
                        argc: cached_argc,
                        ret,
                        ..
                    }) = call_ic[pc].as_ref()
                    && *cached_argc == argc
                    && ret.matches(base, retc)
                    && std::ptr::eq(*fun_ptr, fun as *const Function)
                {
                    cached_fast = true;
                }
                let (captures, capture_specs) = closure.frame_captures();
                if let Some(CallIc::ClosurePositional {
                    closure_ptr: _,
                    fun_ptr: _,
                    argc: _,
                    ret: _,
                    tiny: _,
                    cache,
                    frame_info,
                }) = call_ic[pc].as_mut()
                    && cached_fast
                {
                    match invoke_vm_closure_fast(
                        self_ptr,
                        fun,
                        RegisterSpan::new(start, n, RegisterWindowRef::Base(frame_base)),
                        ctx,
                        Some(frame_info),
                        captures.clone(),
                        capture_specs.clone(),
                        cache,
                        return_meta,
                    ) {
                        Ok(val) => {
                            if retc > 0 {
                                assign_reg(frame_raw, regs, base as usize, val);
                            }
                        }
                        Err(err) => return frame_return_common(frame_raw, pc, Err(err)).map(Some),
                    }
                } else {
                    let mut cache = ClosureFastCache::new();
                    let frame_info = closure.frame_info();
                    match invoke_vm_closure_fast(
                        self_ptr,
                        fun,
                        RegisterSpan::new(start, n, RegisterWindowRef::Base(frame_base)),
                        ctx,
                        Some(&frame_info),
                        captures,
                        capture_specs,
                        &mut cache,
                        return_meta,
                    ) {
                        Ok(val) => {
                            if retc > 0 {
                                assign_reg(frame_raw, regs, base as usize, val);
                            }
                            call_ic[pc] = Some(CallIc::ClosurePositional {
                                closure_ptr,
                                fun_ptr: fun as *const Function,
                                argc,
                                ret: ret_layout,
                                tiny: TinyCallPlan::analyze(fun),
                                cache,
                                frame_info,
                            });
                        }
                        Err(err) => return frame_return_common(frame_raw, pc, Err(err)).map(Some),
                    }
                }
            } else {
                let _frame_guard = CallFrameStackGuard::push(
                    self_ptr,
                    CallFrameMeta::inline_return(resume_pc, base, retc, frame_base),
                );
                if call_args.len() != closure_arc.params.len() {
                    return frame_return_common(
                        frame_raw,
                        pc,
                        Err(anyhow!(
                            "Function expects {} positional arguments, got {}",
                            closure_arc.params.len(),
                            call_args.len()
                        )),
                    )
                    .map(Some);
                }
                let closure = closure_arc.as_ref();
                let fun = closure.code.get_or_init(|| {
                    let c = Compiler::new();
                    Arc::new(c.compile_function_with_captures(
                        closure.params.as_ref(),
                        closure.named_params.as_ref(),
                        closure.body.as_ref(),
                        closure.capture_specs.as_ref(),
                    ))
                });
                let frame_info = closure.frame_info();
                let captures_arc = Arc::clone(&closure.captures);
                let capture_specs_arc = Arc::clone(&closure.capture_specs);
                let call_result = if closure.named_params.is_empty() {
                    Vm::exec_function_with_args(
                        fun.as_ref(),
                        call_args,
                        &[],
                        Some(Arc::clone(&captures_arc)),
                        Some(Arc::clone(&capture_specs_arc)),
                        ctx,
                        self_ptr,
                        Some(frame_info.clone()),
                    )
                } else {
                    let named_params = closure.named_params.as_ref();
                    allocator.with_indexed_vals(named_params.len(), |resolved_seed| -> Result<Val> {
                        for (idx, decl) in named_params.iter().enumerate() {
                            if let Some(default_fun) = closure.default_funcs.get(idx).and_then(|opt| opt.as_ref()) {
                                let default_frame = closure
                                    .default_frame_info(idx)
                                    .expect("default frame info should exist");
                                let hidden_frame = pop_vm_frame(self_ptr);
                                let default_result = allocator.with_reg_val_pairs(resolved_seed.len(), |seed_regs| {
                                    Vm::map_named_seed(default_fun, resolved_seed.as_slice(), seed_regs)?;
                                    Vm::exec_function_with_args(
                                        default_fun,
                                        call_args,
                                        seed_regs.as_slice(),
                                        Some(Arc::clone(&captures_arc)),
                                        Some(Arc::clone(&capture_specs_arc)),
                                        ctx,
                                        self_ptr,
                                        Some(default_frame.clone()),
                                    )
                                });
                                if let Some(meta) = hidden_frame {
                                    push_vm_frame(self_ptr, meta);
                                }
                                let default_val = default_result?;
                                clear_pending_resume_pc(self_ptr);
                                resolved_seed.push((idx, default_val));
                            } else if matches!(decl.type_annotation, Some(Type::Optional(_))) {
                                resolved_seed.push((idx, Val::Nil));
                            } else {
                                return Err(anyhow!("Missing required named argument: {}", decl.name));
                            }
                        }
                        allocator.with_reg_val_pairs(resolved_seed.len(), |seed_regs| {
                            Vm::map_named_seed(fun, resolved_seed.as_slice(), seed_regs)?;
                            Vm::exec_function_with_args(
                                fun,
                                call_args,
                                seed_regs.as_slice(),
                                Some(Arc::clone(&captures_arc)),
                                Some(Arc::clone(&capture_specs_arc)),
                                ctx,
                                self_ptr,
                                Some(frame_info.clone()),
                            )
                        })
                    })
                };
                match call_result {
                    Ok(val) => {
                        if retc > 0 {
                            assign_reg(frame_raw, regs, base as usize, val);
                        }
                    }
                    Err(err) => return frame_return_common(frame_raw, pc, Err(err)).map(Some),
                }
            }
        }
        _ => {
            return frame_return_common(frame_raw, pc, Err(anyhow!("{} is not a function", func.type_name())))
                .map(Some);
        }
    }
    *pc_ref = take_pending_resume_pc(self_ptr, resume_pc);
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_call_native_fast_packed(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut [Option<CallIc>],
    pc_ref: &mut usize,
    next_pc_default: usize,
    frame_base: usize,
    region_allocator_ptr: *const RegionAllocator,
    self_ptr: *mut Vm,
    rf: u16,
    base: u16,
    argc: u8,
    retc: u8,
) -> Result<Option<Val>> {
    let pc = *pc_ref;
    if let Some(callable @ (NativeCallable::Rust(_) | NativeCallable::RustFast(_))) =
        NativeCallable::from_val(&regs[rf as usize])
    {
        let ret_layout = CallReturnLayout::new(base, retc);
        match invoke_native_callable_with_ic(ctx, regs, &mut call_ic[pc], callable, argc, ret_layout) {
            Ok(handled) => debug_assert!(handled),
            Err(err) => return frame_return_common(frame_raw, pc, Err(err)).map(Some),
        }
        *pc_ref = next_pc_default;
        return Ok(None);
    }

    run_call_packed(
        frame_raw,
        regs,
        ctx,
        call_ic,
        pc_ref,
        next_pc_default,
        frame_base,
        region_allocator_ptr,
        self_ptr,
        rf,
        base,
        argc,
        retc,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_call_closure_exact_packed(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut [Option<CallIc>],
    pc_ref: &mut usize,
    next_pc_default: usize,
    frame_base: usize,
    region_allocator_ptr: *const RegionAllocator,
    self_ptr: *mut Vm,
    rf: u16,
    base: u16,
    argc: u8,
    retc: u8,
) -> Result<Option<Val>> {
    let pc = *pc_ref;
    match &regs[rf as usize] {
        Val::Closure(closure) if closure.named_params.is_empty() && closure.params.len() == argc as usize => {}
        Val::Closure(closure) if !closure.named_params.is_empty() => {
            return frame_return_common(
                frame_raw,
                pc,
                Err(anyhow!("exact closure call does not accept named fallback")),
            )
            .map(Some);
        }
        Val::Closure(closure) => {
            return frame_return_common(
                frame_raw,
                pc,
                Err(anyhow!(
                    "Function expects {} positional arguments, got {}",
                    closure.params.len(),
                    argc
                )),
            )
            .map(Some);
        }
        other => {
            return frame_return_common(
                frame_raw,
                pc,
                Err(anyhow!("{} is not an exact closure", other.type_name())),
            )
            .map(Some);
        }
    }

    run_call_packed(
        frame_raw,
        regs,
        ctx,
        call_ic,
        pc_ref,
        next_pc_default,
        frame_base,
        region_allocator_ptr,
        self_ptr,
        rf,
        base,
        argc,
        retc,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_call_exact_packed(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut [Option<CallIc>],
    pc_ref: &mut usize,
    next_pc_default: usize,
    frame_base: usize,
    region_allocator_ptr: *const RegionAllocator,
    self_ptr: *mut Vm,
    rf: u16,
    base: u16,
    argc: u8,
    retc: u8,
) -> Result<Option<Val>> {
    let pc = *pc_ref;
    match &regs[rf as usize] {
        Val::Closure(closure) if closure.named_params.is_empty() && closure.params.len() == argc as usize => {}
        Val::RustFunction(_) | Val::RustFastFunction(_) => {}
        Val::Closure(closure) if !closure.named_params.is_empty() => {
            return frame_return_common(frame_raw, pc, Err(anyhow!("exact call does not accept named fallback")))
                .map(Some);
        }
        Val::Closure(closure) => {
            return frame_return_common(
                frame_raw,
                pc,
                Err(anyhow!(
                    "Function expects {} positional arguments, got {}",
                    closure.params.len(),
                    argc
                )),
            )
            .map(Some);
        }
        other => {
            return frame_return_common(
                frame_raw,
                pc,
                Err(anyhow!("{} is not an exact positional callable", other.type_name())),
            )
            .map(Some);
        }
    }

    run_call_packed(
        frame_raw,
        regs,
        ctx,
        call_ic,
        pc_ref,
        next_pc_default,
        frame_base,
        region_allocator_ptr,
        self_ptr,
        rf,
        base,
        argc,
        retc,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_call_named_packed(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut [Option<CallIc>],
    pc_ref: &mut usize,
    next_pc_default: usize,
    frame_base: usize,
    region_allocator_ptr: *const RegionAllocator,
    self_ptr: *mut Vm,
    rf: u16,
    base_pos: u16,
    posc: u8,
    base_named: u16,
    namedc: u8,
    retc: u8,
) -> Result<Option<Val>> {
    let mut pc = *pc_ref;
    let resume_pc = next_pc_default;
    let frame_guard = CallFrameStackGuard::push(
        self_ptr,
        CallFrameMeta::inline_return(resume_pc, base_pos, retc, frame_base),
    );
    let func = regs[rf as usize].clone();
    let pos_start = base_pos as usize;
    let pos_len = posc as usize;
    let named_start = base_named as usize;
    let named_len = namedc as usize;
    let args_slice = &regs[pos_start..pos_start + pos_len];
    let call_args = CallArgs::registers(RegisterSpan::current(pos_start, pos_len));
    let named_slice = &regs[named_start..named_start + named_len * 2];
    let allocator = region_allocator(region_allocator_ptr);
    let ret_layout = CallReturnLayout::new(base_pos, retc);

    match &func {
        Val::Closure(closure_arc) => {
            if call_args.len() != closure_arc.params.len() {
                return frame_return_common(
                    frame_raw,
                    pc,
                    Err(anyhow!(
                        "Function expects {} positional arguments, got {}",
                        closure_arc.params.len(),
                        call_args.len()
                    )),
                )
                .map(Some);
            }
            let closure = closure_arc.as_ref();
            let fun = closure.code.get_or_init(|| {
                let c = Compiler::new();
                Arc::new(c.compile_function_with_captures(
                    closure.params.as_ref(),
                    closure.named_params.as_ref(),
                    closure.body.as_ref(),
                    closure.capture_specs.as_ref(),
                ))
            });
            let frame_info = closure.frame_info();
            let captures_arc = Arc::clone(&closure.captures);
            let capture_specs_arc = Arc::clone(&closure.capture_specs);
            let closure_ptr = Arc::as_ptr(closure_arc) as usize;
            let cached_plan = if let Some(CallIc::ClosureNamed {
                closure_ptr: cached_ptr,
                named_len: cached_len,
                ret,
                plan,
            }) = call_ic[pc].as_ref()
            {
                if *cached_ptr == closure_ptr && *cached_len as usize == named_len && ret.matches(base_pos, retc) {
                    Some(plan.clone())
                } else {
                    None
                }
            } else {
                None
            };
            let plan = if let Some(plan) = cached_plan {
                plan
            } else {
                match build_named_call_plan(closure, named_slice) {
                    Ok(plan) => {
                        call_ic[pc] = Some(CallIc::ClosureNamed {
                            closure_ptr,
                            named_len: named_len as u8,
                            ret: ret_layout,
                            plan: plan.clone(),
                        });
                        plan
                    }
                    Err(err) => return frame_return_common(frame_raw, pc, Err(err)).map(Some),
                }
            };
            let call_result = allocator.with_indexed_vals(
                plan.provided_indices.len() + plan.defaults_to_eval.len() + plan.optional_nil.len(),
                |seed_pairs| {
                    seed_pairs.clear();
                    for (arg_idx, param_idx) in plan.provided_indices.iter().enumerate() {
                        let value_val = named_slice[2 * arg_idx + 1].clone();
                        seed_pairs.push((*param_idx, value_val));
                    }
                    for &default_idx in plan.defaults_to_eval.iter() {
                        let default_fun = closure
                            .default_funcs
                            .get(default_idx)
                            .and_then(|opt| opt.as_ref())
                            .expect("default function must exist for DefaultThunk");
                        let default_frame = closure
                            .default_frame_info(default_idx)
                            .expect("default frame info should exist");
                        let default_layout = closure
                            .default_seed_regs(default_idx)
                            .expect("default seed layout should exist for default thunk");
                        let hidden_frame = pop_vm_frame(self_ptr);
                        let default_result = allocator.with_reg_val_pairs(seed_pairs.len(), |seed_regs| {
                            for (seed_idx, seed_val) in seed_pairs.iter() {
                                let reg = default_layout
                                    .get(*seed_idx)
                                    .copied()
                                    .expect("default seed layout must cover parent index");
                                seed_regs.push((reg, seed_val.clone()));
                            }
                            Vm::exec_function_with_args(
                                default_fun,
                                call_args,
                                seed_regs.as_slice(),
                                Some(Arc::clone(&captures_arc)),
                                Some(Arc::clone(&capture_specs_arc)),
                                ctx,
                                self_ptr,
                                Some(default_frame.clone()),
                            )
                        });
                        if let Some(meta) = hidden_frame {
                            push_vm_frame(self_ptr, meta);
                        }
                        let default_val = default_result?;
                        clear_pending_resume_pc(self_ptr);
                        seed_pairs.push((default_idx, default_val));
                    }
                    for &optional_idx in plan.optional_nil.iter() {
                        seed_pairs.push((optional_idx, Val::Nil));
                    }

                    allocator.with_reg_val_pairs(seed_pairs.len(), |seed_regs| {
                        for (seed_idx, seed_val) in seed_pairs.iter() {
                            let reg = fun
                                .named_param_regs
                                .get(*seed_idx)
                                .copied()
                                .ok_or_else(|| anyhow!("Named parameter index {} out of range", seed_idx))?;
                            seed_regs.push((reg, seed_val.clone()));
                        }
                        Vm::exec_function_with_args(
                            fun.as_ref(),
                            call_args,
                            seed_regs.as_slice(),
                            Some(Arc::clone(&captures_arc)),
                            Some(Arc::clone(&capture_specs_arc)),
                            ctx,
                            self_ptr,
                            Some(frame_info.clone()),
                        )
                    })
                },
            );
            match call_result {
                Ok(val) => {
                    if retc > 0 {
                        assign_reg(frame_raw, regs, base_pos as usize, val);
                    }
                }
                Err(err) => return frame_return_common(frame_raw, pc, Err(err)).map(Some),
            }
        }
        Val::RustFastFunctionNamed(_) | Val::RustFunctionNamed(_) => {
            let call_output = allocator.with_named_pairs(named_len, |named_vec| {
                load_named_pairs(regs, named_start, named_len, named_vec)?;
                match func.clone() {
                    Val::RustFastFunctionNamed(ptr) => {
                        invoke_rust_fast_function_named(ctx, ptr, ArgWindow::new(args_slice), named_vec.as_slice())
                    }
                    Val::RustFunctionNamed(ptr) => {
                        invoke_rust_function_named_fast(ctx, ptr, ArgWindow::new(args_slice), named_vec.as_slice())
                    }
                    _ => unreachable!(),
                }
            });
            match call_output {
                Ok(value) => ReturnSlot::new(base_pos as usize, retc).write(regs, value),
                Err(err) => return frame_return_common(frame_raw, pc, Err(err)).map(Some),
            }
        }
        _ => {
            return frame_return_common(frame_raw, pc, Err(anyhow!("{} is not a function", func.type_name())))
                .map(Some);
        }
    }
    pc = take_pending_resume_pc(self_ptr, resume_pc);
    drop(frame_guard);
    *pc_ref = pc;
    Ok(None)
}
