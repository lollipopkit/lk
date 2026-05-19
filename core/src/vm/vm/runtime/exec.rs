use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::{
    val::{ClosureCapture, Val, VmFastPathGuard},
    vm::alloc::RegionAllocator,
};

use crate::vm::bytecode::{CaptureSpec, Function};
use crate::vm::context::VmContext;
use crate::vm::vm::Vm;
use crate::vm::vm::caches::{ClosureFastCache, VmCaches};
use crate::vm::vm::frame::{
    CallArgs, CallFrame, CallFrameMeta, FrameInfo, FrameState, RegisterSpan, RegisterWindowRef,
};
use crate::vm::vm::guards::VmCurrentGuard;
use crate::vm::vm::runtime::frame::run_frame;

impl Vm {
    fn allocate_stack_window(&mut self, reg_count: usize) -> usize {
        let base = self.stack_top;
        self.stack_top += reg_count;
        if self.stack.len() < self.stack_top {
            self.stack.resize(self.stack_top, Val::Nil);
        }
        for slot in &mut self.stack[base..base + reg_count] {
            *slot = Val::Nil;
        }
        base
    }

    fn release_stack_window(&mut self, base: usize) {
        self.stack_top = base;
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
        frame_info: Option<FrameInfo>,
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
        frame_info: Option<FrameInfo>,
    ) -> Result<Val> {
        let _vm_fast_path_guard = VmFastPathGuard::enable();
        let self_ptr: *mut Vm = self;
        let _current_vm_guard = VmCurrentGuard::new(self_ptr, ctx as *mut VmContext);
        let initial_stack_top = self.stack_top;
        let initial_frame_depth = self.frames.len();
        let region_plan = f.analysis.as_ref().map(|analysis| analysis.region_plan.clone());
        let reg_count = f.n_regs as usize;
        let reg_base = self.allocate_stack_window(reg_count);
        let exec_result = {
            // Aliases to reusable instruction-site caches
            let access_ic = &mut self.access_ic;
            let index_ic = &mut self.index_ic;
            let global_ic = &mut self.global_ic;
            let call_ic = &mut self.call_ic;
            let for_range_ic = &mut self.for_range_ic;
            let packed_hot_ic = &mut self.packed_hot_ic;
            let quickening_ic = &mut self.quickening_ic;
            let func_key = f as *const Function as usize;
            if self.packed_hot_ic_key != func_key {
                packed_hot_ic.clear();
                self.packed_hot_ic_key = func_key;
            }
            if self.quickening_ic_key != func_key {
                quickening_ic.clear();
                self.quickening_ic_key = func_key;
            }
            let mut call_frame = CallFrame::new(f, reg_base, reg_count, captures, capture_specs, region_plan);
            let reg_count = call_frame.reg_count;

            let region_alloc_ptr: *const RegionAllocator = &self.region_alloc;
            let regs = &mut self.stack[reg_base..reg_base + reg_count];
            let mut frame = FrameState::new(&mut call_frame, regs, region_alloc_ptr);
            if let Some(a) = args
                && !f.param_regs.is_empty()
            {
                let n = a.len().min(f.param_regs.len());
                for (i, val) in a.iter().enumerate().take(n) {
                    let r = f.param_regs[i] as usize;
                    frame.write_reg(r, val.clone());
                }
            }
            if !named.is_empty() {
                for (reg_idx, val) in named {
                    let r = *reg_idx as usize;
                    if r < reg_count {
                        frame.write_reg(r, val.clone());
                    }
                }
            }
            let mut pushed_frame = false;
            if let Some(ref info) = frame_info {
                ctx.push_call_frame(Arc::clone(&info.name), info.location.as_ref().map(Arc::clone));
                pushed_frame = true;
            }

            let exec_raw = run_frame(
                &mut frame,
                ctx,
                VmCaches {
                    access_ic,
                    index_ic,
                    global_ic,
                    call_ic,
                    for_range: for_range_ic,
                    packed_hot: packed_hot_ic,
                    quickening: quickening_ic,
                },
                self_ptr,
            );
            let exec_result = exec_raw;
            if pushed_frame {
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
            } else {
                exec_result
            }
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
    pub(super) fn exec_function_positional_fast_span(
        &mut self,
        fun: &Function,
        args: RegisterSpan,
        ctx: &mut VmContext,
        frame_info: Option<&FrameInfo>,
        captures: Option<Arc<ClosureCapture>>,
        capture_specs: Option<Arc<Vec<CaptureSpec>>>,
        cache: &mut ClosureFastCache,
        return_meta: CallFrameMeta,
    ) -> Result<Val> {
        if fun.param_regs.len() != args.len {
            return Err(anyhow!(
                "Function expects {} positional arguments, got {}",
                fun.param_regs.len(),
                args.len
            ));
        }
        self.exec_function_positional_fast_span_impl(
            fun,
            args,
            ctx,
            frame_info,
            captures,
            capture_specs,
            cache,
            return_meta,
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
    ) -> Result<Val> {
        let reg_count = fun.n_regs as usize;
        let self_ptr: *mut Vm = self;
        let stack_base = self.allocate_stack_window(reg_count);

        for (idx, param_reg) in fun.param_regs.iter().enumerate() {
            let src_idx = args.base + idx;
            let value = self.read_reg(args.window, src_idx).cloned().unwrap_or(Val::Nil);
            self.stack[stack_base + *param_reg as usize] = value;
        }

        let region_plan = if let Some(ref rp) = cache.region_plan {
            Some(Arc::clone(rp))
        } else {
            let rp = fun.analysis.as_ref().map(|analysis| analysis.region_plan.clone());
            cache.region_plan = rp.clone();
            rp
        };
        let mut call_frame = CallFrame::new(fun, stack_base, reg_count, captures, capture_specs, region_plan);
        let region_alloc_ptr: *const RegionAllocator = &self.region_alloc;
        let regs = &mut self.stack[stack_base..stack_base + reg_count];
        let mut callee_state = FrameState::new_ephemeral(&mut call_frame, regs, region_alloc_ptr);
        callee_state.set_inline_return_meta(return_meta);

        let func_key = fun as *const Function as usize;
        if cache.packed_hot_key != func_key {
            cache.packed_hot.clear();
            cache.packed_hot_key = func_key;
        }
        let code_len = fun.code.len();
        if cache.prepared_func_key != func_key || cache.prepared_code_len < code_len {
            if cache.access_ic.len() < code_len {
                cache.access_ic.resize(code_len, None);
            }
            if cache.index_ic.len() < code_len {
                cache.index_ic.resize(code_len, None);
            }
            if cache.global_ic.len() < code_len {
                cache.global_ic.resize(code_len, None);
            }
            if cache.call_ic.len() < code_len {
                cache.call_ic.resize(code_len, None);
            }
            if cache.for_range.len() < code_len {
                cache.for_range.resize(code_len, None);
            }
            cache.prepared_func_key = func_key;
            cache.prepared_code_len = code_len;
        }

        let exec_raw = run_frame(
            &mut callee_state,
            ctx,
            VmCaches {
                access_ic: &mut cache.access_ic,
                index_ic: &mut cache.index_ic,
                global_ic: &mut cache.global_ic,
                call_ic: &mut cache.call_ic,
                for_range: &mut cache.for_range,
                packed_hot: &mut cache.packed_hot,
                quickening: &mut cache.quickening,
            },
            self_ptr,
        );

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
            out.push((reg, value.clone()));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exec_function_with_args(
        fun: &Function,
        positional: CallArgs,
        named_seed: &[(u16, Val)],
        captures: Option<Arc<ClosureCapture>>,
        capture_specs: Option<Arc<Vec<CaptureSpec>>>,
        ctx: &mut VmContext,
        self_ptr: *mut Vm,
        frame_info: Option<FrameInfo>,
    ) -> Result<Val> {
        let vm = unsafe { &mut *self_ptr };
        let parent_window = vm
            .frames
            .last()
            .map(|frame| frame.caller_window)
            .unwrap_or(RegisterWindowRef::Base(0));
        let positional_span = positional.span().relocate(parent_window);
        let reg_count = fun.n_regs as usize;
        if fun.param_regs.len() < positional_span.len {
            return Err(anyhow!(
                "Function expects {} positional arguments, got {}",
                fun.param_regs.len(),
                positional_span.len
            ));
        }
        let reg_base = vm.allocate_stack_window(reg_count);
        for (idx, param_reg) in fun.param_regs.iter().take(positional_span.len).enumerate() {
            let src_idx = positional_span.base + idx;
            let value = vm
                .read_reg(positional_span.window, src_idx)
                .cloned()
                .unwrap_or(Val::Nil);
            vm.stack[reg_base + *param_reg as usize] = value;
        }
        let region_plan = fun.analysis.as_ref().map(|analysis| analysis.region_plan.clone());
        let mut call_frame = CallFrame::new(fun, reg_base, reg_count, captures, capture_specs, region_plan);
        let region_alloc_ptr: *const RegionAllocator = &vm.region_alloc;
        let regs = &mut vm.stack[reg_base..reg_base + reg_count];
        let mut callee_state = FrameState::new(&mut call_frame, regs, region_alloc_ptr);
        for (reg_idx, value) in named_seed {
            let slot = *reg_idx as usize;
            callee_state.write_reg(slot, value.clone());
        }
        let mut nested_cache = vm.nested_cache_pool.pop().unwrap_or_else(ClosureFastCache::new);
        let func_key = fun as *const Function as usize;
        if nested_cache.prepared_func_key != func_key {
            nested_cache.access_ic.clear();
            nested_cache.index_ic.clear();
            nested_cache.global_ic.clear();
            nested_cache.call_ic.clear();
            nested_cache.for_range.clear();
            nested_cache.packed_hot.clear();
            nested_cache.packed_hot_key = 0;
            nested_cache.quickening.clear();
            nested_cache.prepared_func_key = func_key;
            nested_cache.prepared_code_len = 0;
        }
        let mut pushed_frame = false;
        if let Some(ref info) = frame_info {
            ctx.push_call_frame(Arc::clone(&info.name), info.location.as_ref().map(Arc::clone));
            pushed_frame = true;
        }

        let exec_raw = run_frame(
            &mut callee_state,
            ctx,
            VmCaches {
                access_ic: &mut nested_cache.access_ic,
                index_ic: &mut nested_cache.index_ic,
                global_ic: &mut nested_cache.global_ic,
                call_ic: &mut nested_cache.call_ic,
                for_range: &mut nested_cache.for_range,
                packed_hot: &mut nested_cache.packed_hot,
                quickening: &mut nested_cache.quickening,
            },
            self_ptr,
        );
        let exec_result = exec_raw;
        drop(callee_state);
        vm.nested_cache_pool.push(nested_cache);
        vm.release_stack_window(reg_base);
        if pushed_frame {
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
        } else {
            exec_result
        }
    }
}
