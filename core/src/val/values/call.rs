use std::{cell::RefCell, sync::Arc};

use anyhow::{Result, anyhow};

use super::{CallLayoutInfo, ClosureCapture, ClosureValue, NamedParamKind, NativeArgs, Val, vm_fast_path_forced};
use crate::vm::{
    CaptureSpec, Compiler, FrameInfo, Function, Vm, VmContext, copy_call_arg_value_for_register_with_metrics,
    vm_runtime_metrics_enabled, with_current_vm,
};

impl Val {
    /// Call this value as a function with the given arguments.
    #[inline]
    fn call_with_mode(&self, args: &[Val], ctx: &mut VmContext, force_vm: bool) -> Result<Val> {
        let _ = force_vm;
        let _ = vm_fast_path_forced();
        match self {
            #[cfg(feature = "aot-minimal-runtime")]
            Val::Closure(_) => Err(anyhow!("AOT minimal runtime cannot call VM closures")),
            #[cfg(not(feature = "aot-minimal-runtime"))]
            Val::Closure(closure_arc) => {
                let closure = closure_arc.as_ref();
                let params = closure.params.as_ref();
                if args.len() != params.len() {
                    return Err(anyhow!(
                        "Function expects {} arguments, got {}",
                        params.len(),
                        args.len()
                    ));
                }
                let scope_capacity = params.len() + closure.named_params.len();
                let mut named_slots: Vec<Option<Val>> = vec![None; closure.named_params.len()];
                closure.with_call_env(ctx, scope_capacity, |call_env, layout_info| {
                    let frame_info = closure.frame_info_ref();
                    call_env.push_call_frame(
                        Arc::clone(&frame_info.name),
                        frame_info.location.as_ref().map(Arc::clone),
                    );
                    let result = Self::call_named_vm_fast(closure, args, &mut named_slots, call_env, layout_info);
                    call_env.pop_call_frame();
                    result
                })
            }
            Val::RustFunction(func) => func(args, ctx),
            Val::RustFastFunction(func) => func(NativeArgs::new(args), ctx),
            Val::RustFastFunctionNamed(func) => func(NativeArgs::new(args), &[], ctx),
            Val::RustFunctionNamed(func) => func(args, &[], ctx),
            _ => Err(anyhow!("{} is not a function", self.type_name())),
        }
    }

    #[inline]
    pub fn call(&self, args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        self.call_with_mode(args, ctx, false)
    }

    #[inline]
    pub fn call_vm(&self, args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        self.call_with_mode(args, ctx, true)
    }

    /// Call a function value with positional and named arguments.
    pub(super) fn call_named_with_mode(
        &self,
        pos: &[Val],
        named: &[(String, Val)],
        ctx: &mut VmContext,
        force_vm: bool,
    ) -> Result<Val> {
        let _ = force_vm;
        let _ = vm_fast_path_forced();
        match self {
            #[cfg(feature = "aot-minimal-runtime")]
            Val::Closure(_) => Err(anyhow!("AOT minimal runtime cannot call VM closures")),
            #[cfg(not(feature = "aot-minimal-runtime"))]
            Val::Closure(closure_arc) => {
                let closure = closure_arc.as_ref();
                let params = closure.params.as_ref();
                if pos.len() != params.len() {
                    return Err(anyhow!(
                        "Function expects {} positional arguments, got {}",
                        params.len(),
                        pos.len()
                    ));
                }
                let collect_metrics = vm_runtime_metrics_enabled();
                let mut named_slots = closure.build_named_slots_with_metrics(named, collect_metrics)?;
                let scope_capacity = params.len() + closure.named_params.len();
                closure.with_call_env(ctx, scope_capacity, |call_env, layout_info| {
                    let frame_info = closure.frame_info_ref();
                    call_env.push_call_frame(
                        Arc::clone(&frame_info.name),
                        frame_info.location.as_ref().map(Arc::clone),
                    );
                    let result = Self::call_named_vm_fast(closure, pos, &mut named_slots, call_env, layout_info);
                    call_env.pop_call_frame();
                    result
                })
            }
            Val::RustFunction(_) | Val::RustFastFunction(_) => {
                if named.is_empty() {
                    self.call_with_mode(pos, ctx, force_vm)
                } else {
                    Err(anyhow!("Named arguments are not supported for native functions"))
                }
            }
            Val::RustFastFunctionNamed(func) => func(NativeArgs::new(pos), named, ctx),
            Val::RustFunctionNamed(func) => func(pos, named, ctx),
            _ => Err(anyhow!("{} is not a function", self.type_name())),
        }
    }

    fn call_named_vm_fast(
        closure: &ClosureValue,
        pos: &[Val],
        named_slots: &mut [Option<Val>],
        call_env: &mut VmContext,
        layout_info: &CallLayoutInfo,
    ) -> Result<Val> {
        let frame_info = closure.frame_info_ref();
        let collect_metrics = vm_runtime_metrics_enabled();
        let params = closure.params.as_ref();
        let named_params = closure.named_params.as_ref();
        let named_kinds = closure.named_param_kinds();
        let fun = closure.code.get_or_init(|| {
            let c = Compiler::new();
            Arc::new(c.compile_function_with_param_types_and_captures(
                params,
                closure.param_types.as_ref(),
                named_params,
                closure.body.as_ref(),
                closure.capture_specs.as_ref(),
            ))
        });
        Self::bind_positional_params(call_env, params, pos, layout_info, collect_metrics);
        let named_regs = &fun.named_param_regs;
        debug_assert_eq!(
            named_regs.len(),
            named_params.len(),
            "named param register layout mismatch ({} regs vs {} params)",
            named_regs.len(),
            named_params.len()
        );
        let mut named_seed_pairs: Vec<(usize, Val)> = Vec::with_capacity(named_params.len());
        let mut named_seed: Vec<(u16, Val)> = Vec::with_capacity(named_params.len());
        for (idx, decl) in named_params.iter().enumerate() {
            let kind = named_kinds.get(idx).copied().unwrap_or(NamedParamKind::Required);
            let value = if let Some(val) = named_slots.get_mut(idx).and_then(|slot| slot.take()) {
                val
            } else {
                match kind {
                    NamedParamKind::DefaultThunk => {
                        let default_fun = closure
                            .default_funcs
                            .get(idx)
                            .and_then(|opt| opt.as_ref())
                            .expect("default function must exist for DefaultThunk kind");
                        let default_frame = closure
                            .default_frame_info_ref(idx)
                            .expect("default frame info should exist");
                        let layout = closure
                            .default_seed_regs(idx)
                            .expect("default seed layout should exist for default thunk");
                        let mut default_named_seed: Vec<(u16, Val)> = Vec::with_capacity(named_seed_pairs.len());
                        for (seed_idx, seed_val) in named_seed_pairs.iter() {
                            let reg = layout
                                .get(*seed_idx)
                                .copied()
                                .expect("default seed layout must cover parent indices");
                            default_named_seed.push((
                                reg,
                                copy_call_arg_value_for_register_with_metrics(seed_val, collect_metrics),
                            ));
                        }
                        Self::exec_function_with_bindings(
                            default_fun,
                            call_env,
                            pos,
                            default_named_seed.as_slice(),
                            &closure.captures,
                            &closure.capture_specs,
                            Some(default_frame),
                        )?
                    }
                    NamedParamKind::OptionalNil => Val::Nil,
                    NamedParamKind::Required => {
                        return Err(anyhow!("Missing required named argument: {}", decl.name));
                    }
                }
            };
            Self::bind_named_param_value(
                call_env,
                decl,
                copy_call_arg_value_for_register_with_metrics(&value, collect_metrics),
                layout_info,
            );
            named_seed_pairs.push((
                idx,
                copy_call_arg_value_for_register_with_metrics(&value, collect_metrics),
            ));
            named_seed.push((named_regs[idx], value));
        }
        call_env.preload_slot_mappings_per_depth(&layout_info.locals);
        Self::exec_function_with_bindings(
            fun.as_ref(),
            call_env,
            pos,
            named_seed.as_slice(),
            &closure.captures,
            &closure.capture_specs,
            Some(frame_info),
        )
    }

    fn exec_function_with_bindings(
        fun: &Function,
        env: &mut VmContext,
        pos: &[Val],
        named_seed: &[(u16, Val)],
        captures: &Arc<ClosureCapture>,
        capture_specs: &Arc<Vec<CaptureSpec>>,
        frame_info: Option<&FrameInfo>,
    ) -> Result<Val> {
        let frame_captures = || {
            if captures.is_empty() && capture_specs.is_empty() {
                (None, None)
            } else {
                (Some(Arc::clone(captures)), Some(Arc::clone(capture_specs)))
            }
        };
        if let Some(res) = with_current_vm(|vm| {
            let (captures, capture_specs) = frame_captures();
            vm.exec_with_bindings(fun, env, Some(pos), named_seed, captures, capture_specs, frame_info)
        }) {
            res
        } else {
            thread_local! {
                static VM_POOL_NAMED_CALL: RefCell<Option<Vm>> = const { RefCell::new(None) };
            }
            let mut vm = VM_POOL_NAMED_CALL
                .with(|cell| cell.borrow_mut().take())
                .unwrap_or_default();
            let (captures, capture_specs) = frame_captures();
            let res = vm.exec_with_bindings(fun, env, Some(pos), named_seed, captures, capture_specs, frame_info);
            VM_POOL_NAMED_CALL.with(|cell| {
                let _ = cell.borrow_mut().replace(vm);
            });
            res
        }
    }

    pub fn call_named(&self, pos: &[Val], named: &[(String, Val)], ctx: &mut VmContext) -> Result<Val> {
        self.call_named_with_mode(pos, named, ctx, false)
    }

    pub fn call_named_vm(&self, pos: &[Val], named: &[(String, Val)], ctx: &mut VmContext) -> Result<Val> {
        self.call_named_with_mode(pos, named, ctx, true)
    }
}
