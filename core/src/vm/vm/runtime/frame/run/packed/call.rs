use super::super::call_common::{
    CallHotPath, CallTarget, call_target_from_register, run_closure_exact_call_common, run_exact_call_common,
    run_positional_call_common,
};
use super::super::invoke::NativeCallable;
use super::super::raw_boundary::{pop_vm_frame, push_vm_frame, region_allocator};
use super::*;
use crate::vm::copy_call_arg_value_for_register_with_metrics;

#[allow(clippy::too_many_arguments)]
pub(super) fn run_call_packed(
    frame_raw: *mut FrameState<'_, '_>,
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
    collect_metrics: bool,
) -> Result<Option<Val>> {
    let pc = *pc_ref;
    let resume_pc = next_pc_default;
    let allocator = region_allocator(region_allocator_ptr);
    match run_positional_call_common(
        frame_raw,
        regs,
        ctx,
        call_ic,
        pc,
        resume_pc,
        frame_base,
        self_ptr,
        rf,
        base,
        argc,
        retc,
        allocator,
        collect_metrics,
    )? {
        CallHotPath::Done => {}
        CallHotPath::Return(value) => return Ok(Some(value)),
        CallHotPath::Miss => unreachable!("positional call common cannot miss"),
    }
    *pc_ref = take_pending_resume_pc(self_ptr, resume_pc);
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_call_closure_exact_packed(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut [Option<CallIc>],
    pc_ref: &mut usize,
    next_pc_default: usize,
    frame_base: usize,
    _region_allocator_ptr: *const RegionAllocator,
    self_ptr: *mut Vm,
    rf: u16,
    base: u16,
    argc: u8,
    retc: u8,
) -> Result<Option<Val>> {
    let pc = *pc_ref;
    match run_closure_exact_call_common(
        frame_raw,
        regs,
        ctx,
        call_ic,
        pc,
        next_pc_default,
        frame_base,
        self_ptr,
        rf,
        base,
        argc,
        retc,
    )? {
        CallHotPath::Done => {
            *pc_ref = take_pending_resume_pc(self_ptr, next_pc_default);
            Ok(None)
        }
        CallHotPath::Return(value) => Ok(Some(value)),
        CallHotPath::Miss => unreachable!("closure exact call common cannot miss"),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_call_native_fast_packed(
    frame_raw: *mut FrameState<'_, '_>,
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
    collect_metrics: bool,
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
        collect_metrics,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_call_exact_packed(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut [Option<CallIc>],
    pc_ref: &mut usize,
    next_pc_default: usize,
    frame_base: usize,
    _region_allocator_ptr: *const RegionAllocator,
    self_ptr: *mut Vm,
    rf: u16,
    base: u16,
    argc: u8,
    retc: u8,
) -> Result<Option<Val>> {
    let pc = *pc_ref;
    match run_exact_call_common(
        frame_raw,
        regs,
        ctx,
        call_ic,
        pc,
        next_pc_default,
        frame_base,
        self_ptr,
        rf,
        base,
        argc,
        retc,
    )? {
        CallHotPath::Done => {
            *pc_ref = take_pending_resume_pc(self_ptr, next_pc_default);
            Ok(None)
        }
        CallHotPath::Return(value) => Ok(Some(value)),
        CallHotPath::Miss => unreachable!("exact call common cannot miss"),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_call_named_packed(
    frame_raw: *mut FrameState<'_, '_>,
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
    collect_metrics: bool,
) -> Result<Option<Val>> {
    let mut pc = *pc_ref;
    let resume_pc = next_pc_default;
    let frame_guard = CallFrameStackGuard::push(
        self_ptr,
        CallFrameMeta::inline_return(resume_pc, base_pos, retc, frame_base),
    );
    let pos_start = base_pos as usize;
    let pos_len = posc as usize;
    let named_start = base_named as usize;
    let named_len = namedc as usize;
    let args_slice = &regs[pos_start..pos_start + pos_len];
    let call_args = CallArgs::registers(RegisterSpan::current(pos_start, pos_len));
    let named_slice = &regs[named_start..named_start + named_len * 2];
    let allocator = region_allocator(region_allocator_ptr);
    let ret_layout = CallReturnLayout::new(base_pos, retc);

    match call_target_from_register(regs, rf) {
        CallTarget::Closure(closure_arc) => {
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
                Arc::new(c.compile_function_with_param_types_and_captures(
                    closure.params.as_ref(),
                    closure.param_types.as_ref(),
                    closure.named_params.as_ref(),
                    closure.body.as_ref(),
                    closure.capture_specs.as_ref(),
                ))
            });
            let frame_info = closure.frame_info();
            let captures_arc = Arc::clone(&closure.captures);
            let capture_specs_arc = Arc::clone(&closure.capture_specs);
            let closure_ptr = Arc::as_ptr(&closure_arc) as usize;
            let site_plan = match get_or_build_named_call_site_plan(
                call_ic,
                pc,
                closure_ptr,
                named_len,
                ret_layout,
                closure,
                named_slice,
            ) {
                Ok(plan) => plan,
                Err(err) => return frame_return_common(frame_raw, pc, Err(err)).map(Some),
            };
            let plan = &site_plan.named;
            let call_result = allocator.with_indexed_vals(
                plan.provided_indices.len() + plan.defaults_to_eval.len() + plan.optional_nil.len(),
                |seed_pairs| {
                    seed_pairs.clear();
                    for (arg_idx, param_idx) in plan.provided_indices.iter().enumerate() {
                        let value_val = copy_call_arg_value_for_register_with_metrics(
                            &named_slice[2 * arg_idx + 1],
                            collect_metrics,
                        );
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
                                seed_regs.push((
                                    reg,
                                    copy_call_arg_value_for_register_with_metrics(seed_val, collect_metrics),
                                ));
                            }
                            Vm::exec_function_with_args(
                                default_fun,
                                call_args,
                                seed_regs.as_mut_slice(),
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
                        for (seed_idx, seed_val) in seed_pairs.iter_mut() {
                            let reg = fun
                                .named_param_regs
                                .get(*seed_idx)
                                .copied()
                                .ok_or_else(|| anyhow!("Named parameter index {} out of range", seed_idx))?;
                            seed_regs.push((reg, std::mem::replace(seed_val, Val::Nil)));
                        }
                        Vm::exec_function_with_args(
                            fun.as_ref(),
                            call_args,
                            seed_regs.as_mut_slice(),
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
        CallTarget::Native(NativeCallable::RustFastNamed(ptr)) => {
            let call_output = allocator.with_named_pairs(named_len, |named_vec| {
                load_named_pairs(regs, named_start, named_len, named_vec, collect_metrics)?;
                invoke_rust_fast_function_named(ctx, ptr, ArgWindow::new(args_slice), named_vec.as_slice())
            });
            match call_output {
                Ok(value) => ReturnSlot::new(base_pos as usize, retc).write(regs, value),
                Err(err) => return frame_return_common(frame_raw, pc, Err(err)).map(Some),
            }
        }
        CallTarget::Native(NativeCallable::RustNamed(ptr)) => {
            let call_output = allocator.with_named_pairs(named_len, |named_vec| {
                load_named_pairs(regs, named_start, named_len, named_vec, collect_metrics)?;
                invoke_rust_function_named_fast(ctx, ptr, ArgWindow::new(args_slice), named_vec.as_slice())
            });
            match call_output {
                Ok(value) => ReturnSlot::new(base_pos as usize, retc).write(regs, value),
                Err(err) => return frame_return_common(frame_raw, pc, Err(err)).map(Some),
            }
        }
        CallTarget::Native(NativeCallable::Rust(_) | NativeCallable::RustFast(_)) => {
            return frame_return_common(
                frame_raw,
                pc,
                Err(anyhow!("Named arguments are not supported for native functions")),
            )
            .map(Some);
        }
        CallTarget::NotFunction(type_name) => {
            return frame_return_common(frame_raw, pc, Err(anyhow!("{} is not a function", type_name))).map(Some);
        }
    }
    pc = take_pending_resume_pc(self_ptr, resume_pc);
    drop(frame_guard);
    *pc_ref = pc;
    Ok(None)
}
