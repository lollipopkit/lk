use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::{
    val::{ClosureCapture, Val, VmFastPathGuard},
    vm::alloc::RegionAllocator,
};

use crate::vm::bytecode::{CaptureSpec, Function};
use crate::vm::context::VmContext;
use crate::vm::vm::Vm;
use crate::vm::vm::caches::{ClosureFastCache, FunctionRuntimePlan};
use crate::vm::vm::frame::{
    CallArgs, CallFrameMeta, FrameActivation, FrameInfo, FrameStateSetup, RegisterSpan, RegisterWindowRef, StackWindow,
};
use crate::vm::vm::guards::VmCurrentGuard;
use crate::vm::vm::runtime::frame::run_frame;
use crate::vm::{copy_call_arg_value_for_register_with_metrics, vm_runtime_metrics_enabled};

fn push_optional_call_frame(ctx: &mut VmContext, frame_info: Option<&FrameInfo>) -> bool {
    if let Some(info) = frame_info {
        ctx.push_call_frame(Arc::clone(&info.name), info.location.as_ref().map(Arc::clone));
        true
    } else {
        false
    }
}

fn finish_optional_call_frame(ctx: &mut VmContext, pushed_frame: bool, exec_result: Result<Val>) -> Result<Val> {
    if !pushed_frame {
        return exec_result;
    }
    match exec_result {
        Ok(val) => {
            ctx.pop_call_frame();
            Ok(val)
        }
        Err(err) => {
            let report = ctx.call_stack_report();
            ctx.pop_call_frame();
            if let Some(report) = report {
                Err(err.context(report))
            } else {
                Err(err)
            }
        }
    }
}

impl Vm {
    fn allocate_stack_window(&mut self, reg_count: usize) -> usize {
        let base = self.stack_top;
        let new_top = base + reg_count;
        let old_len = self.stack.len();
        self.stack_top = new_top;
        if old_len < new_top {
            self.stack.resize(new_top, Val::Nil);
        }
        let reused_end = old_len.min(new_top);
        for slot in &mut self.stack[base..reused_end] {
            *slot = Val::Nil;
        }
        base
    }

    fn release_stack_window(&mut self, base: usize) {
        self.stack_top = base;
    }

    fn allocate_runtime_stack_window(&mut self, runtime: &FunctionRuntimePlan) -> StackWindow {
        let base = self.allocate_stack_window(runtime.reg_count);
        StackWindow::from_runtime(base, runtime)
    }

    fn activate_runtime_frame(&mut self, runtime: FunctionRuntimePlan, setup: FrameStateSetup) -> FrameActivation {
        let window = self.allocate_runtime_stack_window(&runtime);
        FrameActivation::new(window, runtime, setup)
    }

    pub fn exec(&mut self, f: &Function, ctx: &mut VmContext) -> Result<Val> {
        self.exec_inner(f, ctx, None, &[], None, None, None)
    }

    pub fn exec_with(&mut self, f: &Function, ctx: &mut VmContext, args: Option<&[Val]>) -> Result<Val> {
        self.exec_inner(f, ctx, args, &[], None, None, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exec_with_bindings(
        &mut self,
        f: &Function,
        ctx: &mut VmContext,
        positional: Option<&[Val]>,
        named: &[(u16, Val)],
        captures: Option<Arc<ClosureCapture>>,
        capture_specs: Option<Arc<Vec<CaptureSpec>>>,
        frame_info: Option<&FrameInfo>,
    ) -> Result<Val> {
        self.exec_inner(f, ctx, positional, named, captures, capture_specs, frame_info)
    }

    #[allow(clippy::too_many_arguments)]
    fn exec_inner(
        &mut self,
        f: &Function,
        ctx: &mut VmContext,
        args: Option<&[Val]>,
        named: &[(u16, Val)],
        captures: Option<Arc<ClosureCapture>>,
        capture_specs: Option<Arc<Vec<CaptureSpec>>>,
        frame_info: Option<&FrameInfo>,
    ) -> Result<Val> {
        let _vm_fast_path_guard = VmFastPathGuard::enable();
        let self_ptr: *mut Vm = self;
        let _current_vm_guard = VmCurrentGuard::new(self_ptr, ctx as *mut VmContext);
        let initial_stack_top = self.stack_top;
        let initial_frame_depth = self.frames.len();
        let collect_metrics = vm_runtime_metrics_enabled();
        let runtime = self.prepare_top_level_runtime(f);
        let activation = self.activate_runtime_frame(runtime, FrameStateSetup::synchronized(collect_metrics));
        let exec_result = {
            let runtime_caches = &mut self.runtime_caches;
            let mut activation_parts = activation.into_parts(f, captures, capture_specs);

            let region_alloc_ptr: *const RegionAllocator = &self.region_alloc;
            let mut frame = activation_parts.frame_state(&mut self.stack, region_alloc_ptr);
            if let Some(args) = args {
                frame.seed_positional_from_values(&f.param_regs, args);
            }
            if !named.is_empty() {
                frame.seed_named_call_arg_values(named);
            }
            let pushed_frame = push_optional_call_frame(ctx, frame_info);

            let exec_result = run_frame(&mut frame, ctx, runtime_caches.vm_caches(), self_ptr);
            finish_optional_call_frame(ctx, pushed_frame, exec_result)
        };
        // Since we've removed apply_to_environment, the snapshot already contains the right state
        // No additional synchronization needed
        debug_assert!(
            self.frames.len() == initial_frame_depth,
            "call frame stack leak: {} -> {}",
            initial_frame_depth,
            self.frames.len()
        );
        if initial_stack_top == 0 {
            debug_assert!(self.pending_resume_pc.is_none(), "pending resume PC leaked");
        }
        self.release_stack_window(initial_stack_top);
        exec_result
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn exec_function_positional_fast_span_unchecked(
        &mut self,
        fun: &Function,
        args: RegisterSpan,
        ctx: &mut VmContext,
        frame_info: Option<&FrameInfo>,
        captures: Option<Arc<ClosureCapture>>,
        capture_specs: Option<Arc<Vec<CaptureSpec>>>,
        cache: &mut ClosureFastCache,
        return_meta: CallFrameMeta,
        collect_metrics: bool,
    ) -> Result<Val> {
        debug_assert_eq!(
            fun.param_regs.len(),
            args.len,
            "unchecked positional fast call requires validated arity"
        );
        self.exec_function_positional_fast_span_impl(
            fun,
            args,
            ctx,
            frame_info,
            captures,
            capture_specs,
            cache,
            return_meta,
            collect_metrics,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn exec_function_positional_fast_span_impl(
        &mut self,
        fun: &Function,
        args: RegisterSpan,
        ctx: &mut VmContext,
        frame_info: Option<&FrameInfo>,
        captures: Option<Arc<ClosureCapture>>,
        capture_specs: Option<Arc<Vec<CaptureSpec>>>,
        cache: &mut ClosureFastCache,
        return_meta: CallFrameMeta,
        collect_metrics: bool,
    ) -> Result<Val> {
        let runtime = cache.prepare_function_runtime(fun);
        let self_ptr: *mut Vm = self;
        let activation =
            self.activate_runtime_frame(runtime, FrameStateSetup::inline_ephemeral(return_meta, collect_metrics));
        activation.seed_positional_from_stack(&mut self.stack, args, &fun.param_regs);

        let mut activation_parts = activation.into_parts(fun, captures, capture_specs);
        let stack_base = activation_parts.stack_base();
        let region_alloc_ptr: *const RegionAllocator = &self.region_alloc;
        let mut callee_state = activation_parts.frame_state(&mut self.stack, region_alloc_ptr);

        let exec_raw = run_frame(&mut callee_state, ctx, cache.vm_caches(), self_ptr);

        let exec_result = match exec_raw {
            Ok(val) => Ok(val),
            Err(err) => {
                if let Some(info) = frame_info {
                    Err(err.context(Self::format_call_stack_with_head(ctx, info)))
                } else {
                    Err(err)
                }
            }
        };

        drop(callee_state);
        self.release_stack_window(stack_base);

        exec_result
    }

    fn format_call_stack_with_head(ctx: &VmContext, head: &FrameInfo) -> String {
        fn push_line(msg: &mut String, depth: usize, name: &Arc<str>, location: Option<&Arc<str>>) {
            msg.push_str("  [");
            msg.push_str(&depth.to_string());
            msg.push_str("] ");
            msg.push_str(name.as_ref());
            if let Some(loc) = location {
                msg.push_str(" at ");
                msg.push_str(loc.as_ref());
            }
            msg.push('\n');
        }

        let mut msg = String::from("Call stack:\n");
        let base_depth = ctx.call_stack_depth();
        push_line(&mut msg, base_depth, &head.name, head.location.as_ref());
        for frame in ctx.call_stack().iter().rev() {
            push_line(&mut msg, frame.depth, &frame.function_name, frame.location.as_ref());
        }
        msg
    }

    pub(crate) fn map_named_seed(
        fun: &Function,
        named_bindings: &[(usize, Val)],
        out: &mut Vec<(u16, Val)>,
        collect_metrics: bool,
    ) -> Result<()> {
        out.clear();
        if named_bindings.is_empty() {
            return Ok(());
        }
        if out.capacity() < named_bindings.len() {
            out.reserve(named_bindings.len() - out.capacity());
        }
        for (idx, value) in named_bindings {
            let reg = fun
                .named_param_regs
                .get(*idx)
                .copied()
                .ok_or_else(|| anyhow!("Named parameter index {} out of range", idx))?;
            out.push((
                reg,
                copy_call_arg_value_for_register_with_metrics(value, collect_metrics),
            ));
        }
        Ok(())
    }

    pub(crate) fn map_named_seed_take(
        fun: &Function,
        named_bindings: &mut [(usize, Val)],
        out: &mut Vec<(u16, Val)>,
    ) -> Result<()> {
        out.clear();
        if named_bindings.is_empty() {
            return Ok(());
        }
        if out.capacity() < named_bindings.len() {
            out.reserve(named_bindings.len() - out.capacity());
        }
        for (idx, value) in named_bindings.iter_mut() {
            let reg = fun
                .named_param_regs
                .get(*idx)
                .copied()
                .ok_or_else(|| anyhow!("Named parameter index {} out of range", idx))?;
            out.push((reg, std::mem::replace(value, Val::Nil)));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exec_function_with_args(
        fun: &Function,
        positional: CallArgs,
        named_seed: &mut [(u16, Val)],
        captures: Option<Arc<ClosureCapture>>,
        capture_specs: Option<Arc<Vec<CaptureSpec>>>,
        ctx: &mut VmContext,
        self_ptr: *mut Vm,
        frame_info: Option<&FrameInfo>,
        collect_metrics: bool,
    ) -> Result<Val> {
        let vm = unsafe { &mut *self_ptr };
        let parent_window = vm
            .frames
            .last()
            .map(|frame| frame.caller_window)
            .unwrap_or(RegisterWindowRef::Base(0));
        let positional_span = positional.span().relocate(parent_window);
        if fun.param_regs.len() < positional_span.len {
            return Err(anyhow!(
                "Function expects {} positional arguments, got {}",
                fun.param_regs.len(),
                positional_span.len
            ));
        }
        let mut nested_cache = vm.nested_cache_pool.pop().unwrap_or_else(ClosureFastCache::new);
        let runtime = nested_cache.prepare_function_runtime(fun);
        let activation = vm.activate_runtime_frame(runtime, FrameStateSetup::synchronized(collect_metrics));
        activation.seed_positional_from_stack(&mut vm.stack, positional_span, &fun.param_regs);
        let mut activation_parts = activation.into_parts(fun, captures, capture_specs);
        let reg_base = activation_parts.stack_base();
        let region_alloc_ptr: *const RegionAllocator = &vm.region_alloc;
        let mut callee_state = activation_parts.frame_state(&mut vm.stack, region_alloc_ptr);
        for (reg_idx, value) in named_seed.iter_mut() {
            let slot = *reg_idx as usize;
            callee_state.write_reg(slot, std::mem::replace(value, Val::Nil));
        }
        let pushed_frame = push_optional_call_frame(ctx, frame_info);

        let exec_result = run_frame(&mut callee_state, ctx, nested_cache.vm_caches(), self_ptr);
        drop(callee_state);
        vm.nested_cache_pool.push(nested_cache);
        vm.release_stack_window(reg_base);
        finish_optional_call_frame(ctx, pushed_frame, exec_result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_window_allocation_clears_reused_slots_and_keeps_new_slots_nil() {
        let mut vm = Vm::new();

        let base = vm.allocate_stack_window(3);
        assert_eq!(base, 0);
        assert_eq!(vm.stack_top, 3);
        assert!(vm.stack[..3].iter().all(|slot| matches!(slot, Val::Nil)));

        vm.stack[1] = Val::Int(7);
        vm.release_stack_window(0);

        let base = vm.allocate_stack_window(2);
        assert_eq!(base, 0);
        assert_eq!(vm.stack_top, 2);
        assert!(vm.stack[..2].iter().all(|slot| matches!(slot, Val::Nil)));

        let old_len = vm.stack.len();
        vm.release_stack_window(old_len);
        let base = vm.allocate_stack_window(2);
        assert_eq!(base, old_len);
        assert_eq!(vm.stack_top, old_len + 2);
        assert!(
            vm.stack[old_len..old_len + 2]
                .iter()
                .all(|slot| matches!(slot, Val::Nil))
        );
    }
}
