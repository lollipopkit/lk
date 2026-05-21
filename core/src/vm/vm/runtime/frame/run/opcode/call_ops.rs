use super::super::call_common::{
    CallHotPath, CallTarget, call_target_from_register, run_closure_exact_call_common, run_exact_call_common,
    run_positional_call_common,
};
use super::super::invoke::{
    ArgWindow, NativeCallable, ReturnSlot, clear_pending_resume_pc, invoke_native_callable_with_ic,
    invoke_rust_fast_function_named, invoke_rust_function_named_fast, take_pending_resume_pc,
};
use super::super::raw_boundary::{pop_vm_frame, push_vm_frame, region_allocator};
use super::*;
use crate::vm::copy_call_arg_value_for_register_with_metrics;

#[allow(clippy::too_many_arguments)]
pub(super) fn run_call_opcode(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut Vec<Option<CallIc>>,
    pc_ref: &mut usize,
    frame_base: usize,
    region_allocator_ptr: *const RegionAllocator,
    self_ptr: *mut Vm,
    rf: &u16,
    base: &u16,
    argc: &u8,
    retc: &u8,
    collect_metrics: bool,
) -> Result<Option<Val>> {
    let mut pc = *pc_ref;
    let resume_pc = pc + 1;
    let allocator = region_allocator(region_allocator_ptr);
    let next_pc = resume_pc;
    match run_positional_call_common(
        frame_raw,
        regs,
        ctx,
        call_ic,
        pc,
        resume_pc,
        frame_base,
        self_ptr,
        *rf,
        *base,
        *argc,
        *retc,
        allocator,
        collect_metrics,
    )? {
        CallHotPath::Done => {}
        CallHotPath::Return(value) => return Ok(Some(value)),
        CallHotPath::Miss => unreachable!("positional call common cannot miss"),
    }
    pc = take_pending_resume_pc(self_ptr, next_pc);
    *pc_ref = pc;
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_call_native_fast_opcode(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut Vec<Option<CallIc>>,
    pc_ref: &mut usize,
    frame_base: usize,
    region_allocator_ptr: *const RegionAllocator,
    self_ptr: *mut Vm,
    rf: &u16,
    base: &u16,
    argc: &u8,
    retc: &u8,
    collect_metrics: bool,
) -> Result<Option<Val>> {
    let pc = *pc_ref;
    if let Some(callable @ (NativeCallable::Rust(_) | NativeCallable::RustFast(_))) =
        NativeCallable::from_val(&regs[*rf as usize])
    {
        let ret_layout = CallReturnLayout::new(*base, *retc);
        let handled = invoke_native_callable_with_ic(ctx, regs, &mut call_ic[pc], callable, *argc, ret_layout)?;
        debug_assert!(handled);
        *pc_ref = pc + 1;
        return Ok(None);
    }

    run_call_opcode(
        frame_raw,
        regs,
        ctx,
        call_ic,
        pc_ref,
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
pub(super) fn run_call_closure_exact_opcode(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut Vec<Option<CallIc>>,
    pc_ref: &mut usize,
    frame_base: usize,
    _region_allocator_ptr: *const RegionAllocator,
    self_ptr: *mut Vm,
    rf: &u16,
    base: &u16,
    argc: &u8,
    retc: &u8,
) -> Result<Option<Val>> {
    let pc = *pc_ref;
    match run_closure_exact_call_common(
        frame_raw,
        regs,
        ctx,
        call_ic,
        pc,
        pc + 1,
        frame_base,
        self_ptr,
        *rf,
        *base,
        *argc,
        *retc,
    )? {
        CallHotPath::Done => {
            *pc_ref = take_pending_resume_pc(self_ptr, pc + 1);
            Ok(None)
        }
        CallHotPath::Return(value) => return Ok(Some(value)),
        CallHotPath::Miss => unreachable!("closure exact call common cannot miss"),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_call_exact_opcode(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut Vec<Option<CallIc>>,
    pc_ref: &mut usize,
    frame_base: usize,
    _region_allocator_ptr: *const RegionAllocator,
    self_ptr: *mut Vm,
    rf: &u16,
    base: &u16,
    argc: &u8,
    retc: &u8,
) -> Result<Option<Val>> {
    let pc = *pc_ref;
    match run_exact_call_common(
        frame_raw,
        regs,
        ctx,
        call_ic,
        pc,
        pc + 1,
        frame_base,
        self_ptr,
        *rf,
        *base,
        *argc,
        *retc,
    )? {
        CallHotPath::Done => {
            *pc_ref = take_pending_resume_pc(self_ptr, pc + 1);
            Ok(None)
        }
        CallHotPath::Return(value) => Ok(Some(value)),
        CallHotPath::Miss => unreachable!("exact call common cannot miss"),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_call_named_opcode(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut Vec<Option<CallIc>>,
    pc_ref: &mut usize,
    frame_base: usize,
    region_allocator_ptr: *const RegionAllocator,
    self_ptr: *mut Vm,
    rf: &u16,
    base_pos: &u16,
    posc: &u8,
    base_named: &u16,
    namedc: &u8,
    retc: &u8,
    collect_metrics: bool,
) -> Result<Option<Val>> {
    let mut pc = *pc_ref;
    let resume_pc = pc + 1;
    let frame_guard = CallFrameStackGuard::push(
        self_ptr,
        CallFrameMeta::inline_return(resume_pc, *base_pos, *retc, frame_base),
    );
    let start_pos = *base_pos as usize;
    let npos = *posc as usize;
    let start_named = *base_named as usize;
    let nnamed = *namedc as usize;
    let mut next_pc = resume_pc;
    let allocator = region_allocator(region_allocator_ptr);
    let pos_slice = &regs[start_pos..start_pos + npos];
    let ret_layout = CallReturnLayout::new(*base_pos, *retc);
    let call_result: Result<()> = match call_target_from_register(regs, *rf) {
        CallTarget::Closure(closure_arc) => {
            let closure = closure_arc.as_ref();
            let frame_info = closure.frame_info();
            if npos != closure.params.len() {
                return Err(anyhow!(
                    "Function expects {} positional arguments, got {}",
                    closure.params.len(),
                    npos
                ));
            }
            let named_params = closure.named_params.as_ref();
            let fun = closure.code.get_or_init(|| {
                let c = Compiler::new();
                Arc::new(c.compile_function_with_param_types_and_captures(
                    closure.params.as_ref(),
                    closure.param_types.as_ref(),
                    named_params,
                    closure.body.as_ref(),
                    closure.capture_specs.as_ref(),
                ))
            });
            let layout = &fun.named_param_layout;
            if layout.len() != named_params.len() {
                return Err(anyhow!(
                    "Named parameter layout mismatch (layout={}, decls={})",
                    layout.len(),
                    named_params.len()
                ));
            }
            let positional_span = RegisterSpan::current(start_pos, npos);
            let call_args = CallArgs::registers(positional_span);
            let named_slice = &regs[start_named..start_named + nnamed * 2];
            let closure_ptr = Arc::as_ptr(&closure_arc) as usize;
            let site_plan =
                get_or_build_named_call_site_plan(call_ic, pc, closure_ptr, nnamed, ret_layout, closure, named_slice)?;
            let plan = &site_plan.named;
            allocator.with_indexed_vals(
                plan.provided_indices.len() + plan.defaults_to_eval.len() + plan.optional_nil.len(),
                |seed_pairs| {
                    seed_pairs.clear();
                    for (arg_idx, param_idx) in plan.provided_indices.iter().enumerate() {
                        let val_reg = start_named + 2 * arg_idx + 1;
                        seed_pairs.push((
                            *param_idx,
                            copy_call_arg_value_for_register_with_metrics(&regs[val_reg], collect_metrics),
                        ));
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
                                Some(Arc::clone(&closure.captures)),
                                Some(Arc::clone(&closure.capture_specs)),
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
                        let captures = Some(Arc::clone(&closure.captures));
                        let capture_specs = Some(Arc::clone(&closure.capture_specs));
                        let result = Vm::exec_function_with_args(
                            fun.as_ref(),
                            call_args,
                            seed_regs.as_mut_slice(),
                            captures,
                            capture_specs,
                            ctx,
                            self_ptr,
                            Some(frame_info.clone()),
                        );
                        match result {
                            Ok(val) => {
                                if *retc > 0 {
                                    assign_reg(frame_raw, regs, *base_pos as usize, val);
                                }
                                Ok(())
                            }
                            Err(err) => Err(err),
                        }
                    })
                },
            )
        }
        CallTarget::Native(callable @ (NativeCallable::Rust(_) | NativeCallable::RustFast(_))) => {
            if nnamed > 0 {
                return Err(anyhow!("Named arguments are not supported for native functions"));
            }
            let call_result = invoke_native_callable_with_ic(ctx, regs, &mut call_ic[pc], callable, *posc, ret_layout)
                .map(|handled| debug_assert!(handled));
            match call_result {
                Ok(()) => Ok(()),
                Err(err) => Err(err),
            }
        }
        CallTarget::Native(NativeCallable::RustFastNamed(ptr)) => {
            let call_output = allocator.with_named_pairs(nnamed, |named_vec| {
                for i in 0..nnamed {
                    let key_val = &regs[start_named + 2 * i];
                    let val =
                        copy_call_arg_value_for_register_with_metrics(&regs[start_named + 2 * i + 1], collect_metrics);
                    let key = match key_val {
                        Val::Str(s) => s.to_string(),
                        Val::ShortStr(s) => s.as_str().to_string(),
                        Val::Int(i) => i.to_string(),
                        Val::Float(f) => f.to_string(),
                        Val::Bool(b) => b.to_string(),
                        _ => {
                            return Err(anyhow!("Named argument key must be primitive, got {:?}", key_val));
                        }
                    };
                    named_vec.push((key, val));
                }
                invoke_rust_fast_function_named(ctx, ptr, ArgWindow::new(pos_slice), named_vec.as_slice())
            });
            match call_output {
                Ok(value) => {
                    ReturnSlot::new(*base_pos as usize, *retc).write(regs, value);
                    Ok(())
                }
                Err(err) => Err(err),
            }
        }
        CallTarget::Native(NativeCallable::RustNamed(ptr)) => {
            let call_output = allocator.with_named_pairs(nnamed, |named_vec| {
                for i in 0..nnamed {
                    let key_val = &regs[start_named + 2 * i];
                    let val =
                        copy_call_arg_value_for_register_with_metrics(&regs[start_named + 2 * i + 1], collect_metrics);
                    let key = match key_val {
                        Val::Str(s) => s.to_string(),
                        Val::ShortStr(s) => s.as_str().to_string(),
                        Val::Int(i) => i.to_string(),
                        Val::Float(f) => f.to_string(),
                        Val::Bool(b) => b.to_string(),
                        _ => {
                            return Err(anyhow!("Named argument key must be primitive, got {:?}", key_val));
                        }
                    };
                    named_vec.push((key, val));
                }
                invoke_rust_function_named_fast(ctx, ptr, ArgWindow::new(pos_slice), named_vec.as_slice())
            });
            match call_output {
                Ok(value) => {
                    ReturnSlot::new(*base_pos as usize, *retc).write(regs, value);
                    Ok(())
                }
                Err(err) => Err(err),
            }
        }
        CallTarget::NotFunction(type_name) => Err(anyhow!("{} is not a function", type_name)),
    };
    if let Err(err) = call_result {
        return frame_return_common(frame_raw, pc, Err(err)).map(Some);
    }
    next_pc = take_pending_resume_pc(self_ptr, next_pc);
    drop(frame_guard);
    pc = next_pc;
    *pc_ref = pc;
    Ok(None)
}
