use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::val::{ClosureCapture, ClosureValue, Type, Val};
use crate::vm::alloc::RegionAllocator;
use crate::vm::bytecode::{CaptureSpec, Function};
use crate::vm::compiler::Compiler;
use crate::vm::context::VmContext;
use crate::vm::vm::Vm;
use crate::vm::vm::caches::{CallIc, CallReturnLayout, ClosureFastCache, TinyCallPlan};
use crate::vm::vm::frame::{
    CallArgs, CallFrameMeta, CallFrameStackGuard, FrameInfo, FrameState, RegisterSpan, RegisterWindowRef,
};

use super::helpers::{assign_reg, frame_return_common};
use super::invoke::{clear_pending_resume_pc, invoke_vm_closure_fast};
use super::raw_boundary::{function_from_ptr, pop_vm_frame, push_vm_frame};

pub(crate) enum CallHotPath {
    Miss,
    Done,
    Return(Val),
}

pub(crate) struct PreparedExactClosureCall {
    closure_ptr: usize,
    fun_ptr: *const Function,
    captures: Option<Arc<ClosureCapture>>,
    capture_specs: Option<Arc<Vec<CaptureSpec>>>,
    frame_info: FrameInfo,
    tiny: Option<TinyCallPlan>,
}

pub(crate) fn prepare_exact_closure_call(regs: &[Val], rf: u16, argc: u8) -> Result<PreparedExactClosureCall> {
    let Val::Closure(closure_arc) = &regs[rf as usize] else {
        return Err(anyhow!("{} is not an exact closure", regs[rf as usize].type_name()));
    };
    if !closure_arc.named_params.is_empty() {
        return Err(anyhow!("exact closure call does not accept named fallback"));
    }
    if closure_arc.params.len() != argc as usize {
        return Err(anyhow!(
            "Function expects {} positional arguments, got {}",
            closure_arc.params.len(),
            argc
        ));
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
    let (captures, capture_specs) = closure.frame_captures();
    Ok(PreparedExactClosureCall {
        closure_ptr: Arc::as_ptr(closure_arc) as usize,
        fun_ptr: fun.as_ref() as *const Function,
        captures,
        capture_specs,
        frame_info: closure.frame_info(),
        tiny: TinyCallPlan::analyze(fun.as_ref()),
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_prepared_exact_closure_call(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut [Option<CallIc>],
    pc: usize,
    resume_pc: usize,
    frame_base: usize,
    self_ptr: *mut Vm,
    base: u16,
    argc: u8,
    retc: u8,
    prepared: PreparedExactClosureCall,
) -> Result<CallHotPath> {
    let fun = function_from_ptr(prepared.fun_ptr);
    let start = base as usize;
    let n = argc as usize;
    let ret = CallReturnLayout::new(base, retc);

    let mut cache = ClosureFastCache::new();
    let return_meta = CallFrameMeta::inline_return(resume_pc, base, retc, frame_base);
    match invoke_vm_closure_fast(
        self_ptr,
        fun,
        RegisterSpan::new(start, n, RegisterWindowRef::Base(frame_base)),
        ctx,
        Some(&prepared.frame_info),
        prepared.captures.clone(),
        prepared.capture_specs.clone(),
        &mut cache,
        return_meta,
    ) {
        Ok(val) => {
            if retc > 0 {
                assign_reg(frame_raw, regs, base as usize, val);
            }
            call_ic[pc] = Some(CallIc::ClosurePositional {
                closure_ptr: prepared.closure_ptr,
                fun_ptr: prepared.fun_ptr,
                argc,
                ret,
                tiny: prepared.tiny,
                captures: prepared.captures,
                capture_specs: prepared.capture_specs,
                cache,
                frame_info: prepared.frame_info,
            });
            Ok(CallHotPath::Done)
        }
        Err(err) => frame_return_common(frame_raw, pc, Err(err)).map(CallHotPath::Return),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_run_closure_ic_hot(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut [Option<CallIc>],
    pc: usize,
    resume_pc: usize,
    frame_base: usize,
    self_ptr: *mut Vm,
    rf: u16,
    base: u16,
    argc: u8,
    retc: u8,
) -> Result<CallHotPath> {
    let Some(CallIc::ClosurePositional {
        closure_ptr,
        fun_ptr,
        argc: ic_argc,
        ret,
        tiny,
        captures,
        capture_specs,
        ..
    }) = call_ic[pc].as_ref()
    else {
        return Ok(CallHotPath::Miss);
    };
    if *ic_argc != argc || !ret.matches(base, retc) {
        return Ok(CallHotPath::Miss);
    }

    let Val::Closure(arc) = &regs[rf as usize] else {
        return Ok(CallHotPath::Miss);
    };
    let current_closure_ptr = Arc::as_ptr(arc) as usize;
    let closure_ptr_matches = current_closure_ptr == *closure_ptr;
    let closure_matches = closure_ptr_matches
        || arc
            .code
            .get()
            .map(|fun| std::ptr::eq(Arc::as_ptr(fun), *fun_ptr))
            .unwrap_or(false);
    if !closure_matches {
        return Ok(CallHotPath::Miss);
    }

    let fun = function_from_ptr(*fun_ptr);
    let start = base as usize;
    let n = argc as usize;
    let args_slice_fast = &regs[start..start + n];
    let tiny_captures = if closure_ptr_matches {
        captures.as_deref()
    } else {
        Some(arc.captures.as_ref())
    };
    if let Some(val) = tiny
        .as_ref()
        .and_then(|plan| plan.try_eval(args_slice_fast, tiny_captures))
    {
        if retc > 0 {
            assign_reg(frame_raw, regs, base as usize, val);
        }
        return Ok(CallHotPath::Done);
    }

    let return_meta = CallFrameMeta::inline_return(resume_pc, base, retc, frame_base);
    let (invoke_captures, invoke_capture_specs) = if closure_ptr_matches {
        (captures.clone(), capture_specs.clone())
    } else {
        arc.frame_captures()
    };
    if let Some(CallIc::ClosurePositional { cache, frame_info, .. }) = call_ic[pc].as_mut() {
        let val = invoke_vm_closure_fast(
            self_ptr,
            fun,
            RegisterSpan::new(start, n, RegisterWindowRef::Base(frame_base)),
            ctx,
            Some(frame_info),
            invoke_captures,
            invoke_capture_specs,
            cache,
            return_meta,
        );
        match val {
            Ok(val) => {
                if retc > 0 {
                    assign_reg(frame_raw, regs, base as usize, val);
                }
            }
            Err(err) => return frame_return_common(frame_raw, pc, Err(err)).map(CallHotPath::Return),
        }
        return Ok(CallHotPath::Done);
    }

    Ok(CallHotPath::Miss)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn try_run_positional_closure_call(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    call_ic: &mut [Option<CallIc>],
    pc: usize,
    resume_pc: usize,
    frame_base: usize,
    self_ptr: *mut Vm,
    base: u16,
    argc: u8,
    retc: u8,
    ret_layout: CallReturnLayout,
    closure_arc: &Arc<ClosureValue>,
) -> Result<CallHotPath> {
    let closure_ptr = Arc::as_ptr(closure_arc) as usize;
    let mut cached_fast = matches!(call_ic[pc].as_ref(), Some(CallIc::ClosurePositional { closure_ptr: cached_ptr, argc: cached_argc, ret, .. }) if *cached_ptr == closure_ptr && *cached_argc == argc && ret.matches(base, retc));
    let supports_fast = cached_fast || closure_arc.supports_vm_positional_fast_path();
    if !supports_fast || !closure_arc.named_params.is_empty() {
        return Ok(CallHotPath::Miss);
    }

    let start = base as usize;
    let n = argc as usize;
    if !cached_fast && n != closure_arc.params.len() {
        return frame_return_common(
            frame_raw,
            pc,
            Err(anyhow!(
                "Function expects {} positional arguments, got {}",
                closure_arc.params.len(),
                n
            )),
        )
        .map(CallHotPath::Return);
    }

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
                Arc::new(c.compile_function_with_param_types_and_captures(
                    closure.params.as_ref(),
                    closure.param_types.as_ref(),
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

    let return_meta = CallFrameMeta::inline_return(resume_pc, base, retc, frame_base);
    if let Some(CallIc::ClosurePositional {
        closure_ptr: _,
        fun_ptr: _,
        argc: _,
        ret: _,
        tiny: _,
        captures: cached_captures,
        capture_specs: cached_capture_specs,
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
            cached_captures.clone(),
            cached_capture_specs.clone(),
            cache,
            return_meta,
        ) {
            Ok(val) => {
                if retc > 0 {
                    assign_reg(frame_raw, regs, base as usize, val);
                }
            }
            Err(err) => return frame_return_common(frame_raw, pc, Err(err)).map(CallHotPath::Return),
        }
    } else {
        let (captures, capture_specs) = closure.frame_captures();
        let mut cache = ClosureFastCache::new();
        let frame_info = closure.frame_info();
        match invoke_vm_closure_fast(
            self_ptr,
            fun,
            RegisterSpan::new(start, n, RegisterWindowRef::Base(frame_base)),
            ctx,
            Some(&frame_info),
            captures.clone(),
            capture_specs.clone(),
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
                    captures,
                    capture_specs,
                    cache,
                    frame_info,
                });
            }
            Err(err) => return frame_return_common(frame_raw, pc, Err(err)).map(CallHotPath::Return),
        }
    }

    Ok(CallHotPath::Done)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_closure_slow_call(
    frame_raw: *mut FrameState<'_>,
    regs: &mut [Val],
    ctx: &mut VmContext,
    pc: usize,
    resume_pc: usize,
    frame_base: usize,
    self_ptr: *mut Vm,
    base: u16,
    argc: u8,
    retc: u8,
    allocator: &RegionAllocator,
    closure_arc: &Arc<ClosureValue>,
) -> Result<CallHotPath> {
    let start = base as usize;
    let n = argc as usize;
    let call_args = CallArgs::registers(RegisterSpan::current(start, n));
    let _frame_guard = CallFrameStackGuard::push(
        self_ptr,
        CallFrameMeta::inline_return(resume_pc, base, retc, frame_base),
    );
    let positional_count = closure_arc.params.len();
    let named_count = closure_arc.named_params.len();
    if call_args.len() < positional_count
        || call_args.len() > positional_count + named_count
        || (named_count == 0 && call_args.len() != positional_count)
    {
        return frame_return_common(
            frame_raw,
            pc,
            Err(anyhow!(
                "Function expects {} positional arguments, got {}",
                positional_count,
                call_args.len()
            )),
        )
        .map(CallHotPath::Return);
    }
    let extra_positional_count = call_args.len().saturating_sub(positional_count);
    let positional_call_args = if extra_positional_count == 0 {
        call_args
    } else {
        let span = call_args.span();
        CallArgs::registers(RegisterSpan::new(span.base, positional_count, span.window))
    };
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
    let call_result = if closure.named_params.is_empty() {
        Vm::exec_function_with_args(
            fun.as_ref(),
            positional_call_args,
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
            for idx in 0..extra_positional_count {
                let span = call_args.span();
                resolved_seed.push((idx, regs[span.base + positional_count + idx].clone()));
            }
            for (idx, decl) in named_params.iter().enumerate() {
                if idx < extra_positional_count {
                    continue;
                }
                if let Some(default_fun) = closure.default_funcs.get(idx).and_then(|opt| opt.as_ref()) {
                    let default_frame = closure
                        .default_frame_info(idx)
                        .expect("default frame info should exist");
                    let hidden_frame = pop_vm_frame(self_ptr);
                    let default_result = allocator.with_reg_val_pairs(resolved_seed.len(), |seed_regs| {
                        Vm::map_named_seed(default_fun, resolved_seed.as_slice(), seed_regs)?;
                        Vm::exec_function_with_args(
                            default_fun,
                            positional_call_args,
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
                    fun.as_ref(),
                    positional_call_args,
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
            Ok(CallHotPath::Done)
        }
        Err(err) => frame_return_common(frame_raw, pc, Err(err)).map(CallHotPath::Return),
    }
}
