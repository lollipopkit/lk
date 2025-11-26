use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::{
    val::{ClosureCapture, Val, VmFastPathGuard},
    vm::alloc::RegionAllocator,
};

use crate::vm::bytecode::{CaptureSpec, Function};
use crate::vm::context::VmContext;
use crate::vm::vm::Vm;
use crate::vm::vm::caches::{
    AccessIc, CallIc, ClosureFastCache, ForRangeState, GlobalEntry, IndexIc, PackedHotEntry, VmCaches,
};
use crate::vm::vm::frame::{CallArgs, CallFrame, CallFrameMeta, FrameInfo, FrameState};
use crate::vm::vm::guards::VmNestedCallGuard;
use crate::vm::vm::runtime::frame::run_frame;

impl Vm {
    pub(crate) fn enter_nested_call(&mut self) -> VmNestedCallGuard {
        VmNestedCallGuard::new(self)
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
        let initial_reg_stack_depth = self.reg_stack.len();
        let initial_frame_depth = self.frames.len();
        let exec_result = {
            // Aliases to reusable instruction-site caches
            let access_ic = &mut self.access_ic;
            let index_ic = &mut self.index_ic;
            let global_ic = &mut self.global_ic;
            let call_ic = &mut self.call_ic;
            let for_range_ic = &mut self.for_range_ic;
            let packed_hot_ic = &mut self.packed_hot_ic;
            let func_key = f as *const Function as usize;
            if self.packed_hot_ic_key != func_key {
                packed_hot_ic.clear();
                self.packed_hot_ic_key = func_key;
            }
            // Ensure capacity and initialize registers to Nil without unnecessary drops/reallocs.
            let region_plan = f.analysis.as_ref().map(|analysis| analysis.region_plan.clone());
            let mut call_frame = CallFrame::new(f, 0, f.n_regs as usize, captures, capture_specs, region_plan);
            let reg_base = call_frame.reg_base;
            let reg_count = call_frame.reg_count;
            {
                let regs = &mut self.regs;
                let needed = reg_base + reg_count;
                if regs.len() >= needed {
                    regs.truncate(needed);
                } else {
                    regs.resize(needed, Val::Nil);
                }
            }

            let region_alloc_ptr: *const RegionAllocator = &self.region_alloc;
            let mut frame = FrameState::new(&mut call_frame, &mut self.regs, region_alloc_ptr);
            if let Some(a) = args
                && !f.param_regs.is_empty()
            {
                let n = a.len().min(f.param_regs.len());
                for (i, val) in a.iter().enumerate().take(n) {
                    let r = reg_base + f.param_regs[i] as usize;
                    frame.write_reg(r, val.clone());
                }
            }
            if !named.is_empty() {
                for (reg_idx, val) in named {
                    let r = reg_base + (*reg_idx as usize);
                    if r < reg_base + reg_count {
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
                },
                self_ptr,
            );
            let _ = frame.clear_written_regs();
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
        if initial_reg_stack_depth == 0 {
            debug_assert!(self.pending_resume_pc.is_none(), "pending resume PC leaked");
        }
        debug_assert!(
            self.reg_stack.len() == initial_reg_stack_depth,
            "register window stack leak: {} (expected {})",
            self.reg_stack.len(),
            initial_reg_stack_depth
        );
        exec_result
    }

    pub(super) fn exec_function_positional_fast(
        &mut self,
        fun: &Function,
        args: &[Val],
        ctx: &mut VmContext,
        frame_info: Option<&FrameInfo>,
        cache: Option<&mut ClosureFastCache>,
        return_meta: Option<CallFrameMeta>,
    ) -> Result<Val> {
        if fun.param_regs.len() != args.len() {
            return Err(anyhow!(
                "Function expects {} positional arguments, got {}",
                fun.param_regs.len(),
                args.len()
            ));
        }
        match cache {
            Some(cache_ref) => {
                self.exec_function_positional_fast_impl(fun, args, ctx, frame_info, cache_ref, return_meta)
            }
            None => {
                let mut temp_cache = ClosureFastCache::new();
                self.exec_function_positional_fast_impl(fun, args, ctx, frame_info, &mut temp_cache, return_meta)
            }
        }
    }

    fn exec_function_positional_fast_impl(
        &mut self,
        fun: &Function,
        args: &[Val],
        ctx: &mut VmContext,
        frame_info: Option<&FrameInfo>,
        cache: &mut ClosureFastCache,
        return_meta: Option<CallFrameMeta>,
    ) -> Result<Val> {
        let nested_guard = self.enter_nested_call();
        let reg_count = fun.n_regs as usize;
        let self_ptr: *mut Vm = self;

        let pooled_regs = std::mem::take(&mut self.regs);
        let mut regs = std::mem::take(&mut cache.regs);
        #[cfg(debug_assertions)]
        if regs.is_empty() {
            eprintln!(
                "exec_function_positional_fast: function regs={}, code_len={}",
                reg_count,
                fun.code.len()
            );
        }
        if regs.is_empty() {
            #[cfg(debug_assertions)]
            eprintln!(
                "[debug] exec positional fast: regs={} code_len={}",
                reg_count,
                fun.code.len()
            );
        }
        if regs.len() >= reg_count {
            regs.truncate(reg_count);
        } else {
            regs.resize(reg_count, Val::Nil);
        }

        self.regs = regs;

        let region_plan = fun.analysis.as_ref().map(|analysis| analysis.region_plan.clone());
        let mut call_frame = CallFrame::new(fun, 0, reg_count, None, None, region_plan);
        let region_alloc_ptr: *const RegionAllocator = &self.region_alloc;
        let mut callee_state = FrameState::new(&mut call_frame, &mut self.regs, region_alloc_ptr);
        for (idx, param_reg) in fun.param_regs.iter().enumerate() {
            callee_state.write_reg(*param_reg as usize, args[idx].clone());
        }
        if let Some(meta) = return_meta {
            callee_state.set_inline_return_meta(meta);
        }

        let mut access_ic = std::mem::take(&mut cache.access_ic);
        let mut index_ic = std::mem::take(&mut cache.index_ic);
        let mut global_ic = std::mem::take(&mut cache.global_ic);
        let mut call_ic_cache = std::mem::take(&mut cache.call_ic);
        let mut for_range_ic = std::mem::take(&mut cache.for_range);
        let mut packed_hot = std::mem::take(&mut cache.packed_hot);
        let func_key = fun as *const Function as usize;
        if cache.packed_hot_key != func_key {
            packed_hot.clear();
            cache.packed_hot_key = func_key;
        }
        let code_len = fun.code.len();
        if access_ic.len() < code_len {
            access_ic.resize(code_len, None);
        }
        if index_ic.len() < code_len {
            index_ic.resize(code_len, None);
        }
        if global_ic.len() < code_len {
            global_ic.resize(code_len, None);
        }
        if call_ic_cache.len() < code_len {
            call_ic_cache.resize(code_len, None);
        }
        if for_range_ic.len() < code_len {
            for_range_ic.resize(code_len, None);
        }

        let exec_raw = run_frame(
            &mut callee_state,
            ctx,
            VmCaches {
                access_ic: &mut access_ic,
                index_ic: &mut index_ic,
                global_ic: &mut global_ic,
                call_ic: &mut call_ic_cache,
                for_range: &mut for_range_ic,
                packed_hot: &mut packed_hot,
            },
            self_ptr,
        );

        let _ = callee_state.clear_written_regs();

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

        cache.access_ic = access_ic;
        cache.index_ic = index_ic;
        cache.global_ic = global_ic;
        cache.call_ic = call_ic_cache;
        cache.for_range = for_range_ic;
        cache.packed_hot = packed_hot;

        let regs_back = std::mem::take(&mut self.regs);
        cache.regs = regs_back;
        self.regs = pooled_regs;
        drop(nested_guard);

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
        let nested_guard = vm.enter_nested_call();
        let parent_window = nested_guard.parent_window();
        let positional_span = positional.span().relocate(parent_window);
        let reg_count = fun.n_regs as usize;
        let reg_base = 0;
        if fun.param_regs.len() < positional_span.len {
            return Err(anyhow!(
                "Function expects {} positional arguments, got {}",
                fun.param_regs.len(),
                positional_span.len
            ));
        }
        let mut positional_values = Vec::with_capacity(positional_span.len);
        for idx in 0..positional_span.len {
            let src_idx = positional_span.base + idx;
            let value = vm
                .read_reg(positional_span.window, src_idx)
                .cloned()
                .unwrap_or(Val::Nil);
            positional_values.push(value);
        }
        if vm.regs.len() >= reg_count {
            vm.regs.truncate(reg_count);
        } else {
            vm.regs.resize(reg_count, Val::Nil);
        }
        let region_plan = fun.analysis.as_ref().map(|analysis| analysis.region_plan.clone());
        let mut call_frame = CallFrame::new(fun, reg_base, reg_count, captures, capture_specs, region_plan);
        let region_alloc_ptr: *const RegionAllocator = &vm.region_alloc;
        let mut callee_state = FrameState::new(&mut call_frame, &mut vm.regs, region_alloc_ptr);
        for (idx, param_reg) in fun.param_regs.iter().enumerate() {
            if let Some(value) = positional_values.get(idx) {
                let reg_idx = reg_base + (*param_reg as usize);
                callee_state.write_reg(reg_idx, value.clone());
            }
        }
        for (reg_idx, value) in named_seed {
            let slot = reg_base + (*reg_idx as usize);
            callee_state.write_reg(slot, value.clone());
        }
        let mut nested_access_ic: Vec<Option<AccessIc>> = Vec::new();
        let mut nested_index_ic: Vec<Option<IndexIc>> = Vec::new();
        let mut nested_global_ic: Vec<Option<GlobalEntry>> = Vec::new();
        let mut nested_call_ic: Vec<Option<CallIc>> = Vec::new();
        let mut nested_for_range_ic: Vec<Option<ForRangeState>> = Vec::new();
        let mut nested_packed_hot: Vec<Option<PackedHotEntry>> = Vec::new();
        let mut pushed_frame = false;
        if let Some(ref info) = frame_info {
            ctx.push_call_frame(Arc::clone(&info.name), info.location.as_ref().map(Arc::clone));
            pushed_frame = true;
        }

        let exec_raw = run_frame(
            &mut callee_state,
            ctx,
            VmCaches {
                access_ic: &mut nested_access_ic,
                index_ic: &mut nested_index_ic,
                global_ic: &mut nested_global_ic,
                call_ic: &mut nested_call_ic,
                for_range: &mut nested_for_range_ic,
                packed_hot: &mut nested_packed_hot,
            },
            self_ptr,
        );
        let _ = callee_state.clear_written_regs();
        let exec_result = exec_raw;
        drop(callee_state);
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
