use super::*;

impl Executor {
    pub fn run_function(self, function: &Function) -> Result<ExecResult> {
        let mut ctx = None;
        let mut this = self;
        this.reset_entry_frame(function.register_count);
        // Module-less execution never pushes a `CallFrame` (CallDirect/`Call`-to-
        // closure both require a `Module`), so the entry index is never read.
        let returns = this.run_function_inner(function, 0, None, &mut ctx)?.into_vec();
        Ok(this.finish(returns))
    }

    pub fn run_module(self, module: &Module) -> Result<ExecResult> {
        let entry = module
            .entry_function()
            .ok_or_else(|| anyhow!("module entry function {} out of bounds", module.entry))?;
        let mut this = self;
        this.state.globals = vec![RuntimeVal::Nil; module.globals.len()];
        this.reset_entry_frame(entry.register_count);
        let mut ctx = None;
        let returns = this
            .run_function_inner(entry, module.entry, Some(module), &mut ctx)?
            .into_vec();
        Ok(this.finish(returns))
    }

    pub fn run_module_with_globals(self, module: &Module, globals: Vec<RuntimeVal>) -> Result<ExecResult> {
        self.run_module_with_globals_and_heap(module, globals, HeapStore::new())
    }

    pub fn run_module_with_globals_and_heap(
        mut self,
        module: &Module,
        globals: Vec<RuntimeVal>,
        heap: HeapStore,
    ) -> Result<ExecResult> {
        let entry = module
            .entry_function()
            .ok_or_else(|| anyhow!("module entry function {} out of bounds", module.entry))?;
        if globals.len() != module.globals.len() {
            bail!(
                "module expected {} globals, got {}",
                module.globals.len(),
                globals.len()
            );
        }
        self.state.globals = globals;
        self.state.heap = heap;
        self.reset_entry_frame(entry.register_count);
        let mut ctx = None;
        let returns = self
            .run_function_inner(entry, module.entry, Some(module), &mut ctx)?
            .into_vec();
        Ok(self.finish(returns))
    }

    pub fn run_module_with_globals_and_ctx(
        mut self,
        module: &Module,
        globals: Vec<RuntimeVal>,
        heap: HeapStore,
        ctx: &mut VmContext,
    ) -> Result<ExecResult> {
        let entry = module
            .entry_function()
            .ok_or_else(|| anyhow!("module entry function {} out of bounds", module.entry))?;
        if globals.len() != module.globals.len() {
            bail!(
                "module expected {} globals, got {}",
                module.globals.len(),
                globals.len()
            );
        }
        self.state.globals = globals;
        self.state.heap = heap;
        self.reset_entry_frame(entry.register_count);
        let mut ctx = Some(ctx);
        let returns = self
            .run_function_inner(entry, module.entry, Some(module), &mut ctx)?
            .into_vec();
        Ok(self.finish(returns))
    }

    pub fn run_shared_module_with_globals_and_heap_and_ctx(
        mut self,
        module: Arc<Module>,
        globals: Vec<RuntimeVal>,
        heap: HeapStore,
        ctx: &mut VmContext,
    ) -> Result<ExecResult> {
        self.shared_module = Some(Arc::clone(&module));
        self.run_module_with_globals_and_ctx(module.as_ref(), globals, heap, ctx)
    }

    #[allow(clippy::too_many_arguments, clippy::result_large_err)] // ExecFailure carries the full recovery state by design
    pub(crate) fn run_module_function_with_state_recoverable<F>(
        mut self,
        module: &Module,
        shared_module: Option<Arc<Module>>,
        function_index: u32,
        captures: Arc<Vec<RuntimeVal>>,
        state: RuntimeModuleState,
        ctx: &mut VmContext,
        seed_args: F,
    ) -> core::result::Result<ExecResult, ExecFailure>
    where
        F: FnOnce(&mut Self) -> Result<u16>,
    {
        let Some(function) = module.functions.get(function_index as usize) else {
            return Err(ExecFailure {
                error: anyhow!("function index {} out of bounds", function_index),
                state,
            });
        };
        if state.globals.len() != module.globals.len() {
            return Err(ExecFailure {
                error: anyhow!(
                    "module expected {} globals, got {}",
                    module.globals.len(),
                    state.globals.len()
                ),
                state,
            });
        }
        let saved_top = state.stack_top();
        self.state = state;
        self.captures = captures;
        self.shared_module = shared_module;
        self.reset_entry_frame(function.register_count);
        let arg_count = match seed_args(&mut self) {
            Ok(arg_count) => arg_count,
            Err(error) => {
                self.state.stack_top = saved_top;
                return Err(ExecFailure {
                    error,
                    state: self.state,
                });
            }
        };
        if function.param_count != arg_count {
            self.state.stack_top = saved_top;
            return Err(ExecFailure {
                error: anyhow!(
                    "Function expects {} positional arguments, got {}",
                    function.param_count,
                    arg_count
                ),
                state: self.state,
            });
        }
        let mut ctx = Some(ctx);
        if let Err(error) = self.enter_lk_call() {
            self.state.stack_top = saved_top;
            return Err(ExecFailure {
                error,
                state: self.state,
            });
        }
        let result = grow_stack_if_needed(|| self.run_function_inner(function, function_index, Some(module), &mut ctx));
        self.exit_lk_call();
        match result {
            Ok(returns) => {
                let returns = returns.into_vec();
                self.state.stack_top = saved_top;
                Ok(self.finish(returns))
            }
            Err(error) => {
                self.state.stack_top = saved_top;
                Err(ExecFailure {
                    error,
                    state: self.state,
                })
            }
        }
    }

    pub(in crate::vm::exec) fn finish(self, returns: Vec<RuntimeVal>) -> ExecResult {
        ExecResult {
            returns,
            state: self.state,
        }
    }

    pub(in crate::vm::exec) fn run_function_inner(
        &mut self,
        function: &Function,
        function_index: u32,
        module: Option<&Module>,
        ctx: &mut Option<&mut VmContext>,
    ) -> Result<ReturnValues> {
        // Monomorphize the dispatch loop on whether an instruction budget is
        // active: only the WASM playground sets one, so direct execution
        // should not pay a checked counter increment per instruction.
        if self.instruction_budget.is_some() || self.heap_object_limit.is_some() {
            self.run_function_inner_impl::<true>(function, function_index, module, ctx)
        } else {
            self.run_function_inner_impl::<false>(function, function_index, module, ctx)
        }
    }

    /// Trampoline for a "flat run": `dispatch_within_frame` processes
    /// instructions for one LK function activation at a time and returns
    /// whenever a `CallDirect`/`Call`-to-closure pushes a callee frame, a
    /// `Return*` pops back to a caller frame *within this flat run*, or the
    /// flat run truly finishes. Neither case recurses through Rust — LK call
    /// depth grows `self.frames` (a `Vec`, heap-allocated) instead of the
    /// Rust stack (plan M2.5 sub-step ①). Native re-entry (`pcall`, stdlib
    /// HOFs, `CallNamed`/`CallMethodK`) still calls back into this function
    /// recursively, exactly as before — each such re-entry just starts a new
    /// bounded flat run (`base_frame_depth` scopes `self.frames` to frames
    /// pushed *within* this particular invocation).
    pub(in crate::vm::exec) fn run_function_inner_impl<const BUDGETED: bool>(
        &mut self,
        function: &Function,
        function_index: u32,
        module: Option<&Module>,
        ctx: &mut Option<&mut VmContext>,
    ) -> Result<ReturnValues> {
        if self.register_count < function.register_count {
            bail!(
                "executor frame has {} registers, function requires {}",
                self.register_count,
                function.register_count
            );
        }
        let base_frame_depth = self.frames.len();
        self.current_function_index = function_index;
        let mut function = function;
        loop {
            match self.dispatch_within_frame::<BUDGETED>(function, module, ctx, base_frame_depth) {
                Ok(FrameOutcome::Switch(idx)) => {
                    function = module
                        .and_then(|module| module.functions.get(idx as usize))
                        .ok_or_else(|| anyhow!("function index {} out of bounds", idx))?;
                }
                Ok(FrameOutcome::Done(values)) => return Ok(values),
                Err(error) => {
                    let idx = self.unwind_flat_run(error, function, module, ctx, base_frame_depth)?;
                    function = module
                        .and_then(|module| module.functions.get(idx as usize))
                        .ok_or_else(|| anyhow!("function index {} out of bounds", idx))?;
                }
            }
        }
    }

    /// A `Return*` opcode (or falling off the end of a function's code)
    /// completed the *currently dispatching* activation. If there's a caller
    /// frame within this flat run, pop it, restore the caller's context, and
    /// deliver the value into the call's result register (mirrors what the
    /// old recursive `call_closure_stack_args` did after its nested
    /// `run_function_inner` call returned `Ok`). Otherwise this flat run is
    /// genuinely done.
    pub(in crate::vm::exec) fn finish_return(
        &mut self,
        values: ReturnValues,
        base_frame_depth: usize,
    ) -> Result<FrameOutcome> {
        if self.frames.len() == base_frame_depth {
            return Ok(FrameOutcome::Done(values));
        }
        let frame = self.frames.pop().expect("checked frames.len() above");
        self.exit_lk_call();
        let value = values.into_first();
        self.current_function_index = frame.function_index;
        self.frame_base = frame.frame_base;
        self.register_count = frame.register_count;
        self.state.stack_top = frame.stack_top;
        self.captures = frame.captures;
        self.handler_stack.truncate(frame.handler_depth);
        self.pc = frame.pc + 1;
        self.clear_call_window_temps(frame.window, frame.named_count)?;
        self.write_returns(frame.window, [value])?;
        Ok(FrameOutcome::Switch(frame.function_index))
    }

    /// An instruction in the currently dispatching activation raised an
    /// error. Pop frames within this flat run one at a time — mirroring how
    /// the old recursive implementation unwound one Rust call boundary at a
    /// time — pushing a traceback entry for each, until either a `try`
    /// wrapping the *immediate* caller's call catches it (the only case
    /// `handler_stack` ever supported — see `docs/vm-stackless.md`) or the
    /// flat run's own frames are exhausted (propagate to whatever Rust caller
    /// invoked `run_function_inner_impl`, exactly as today).
    pub(in crate::vm::exec) fn unwind_flat_run(
        &mut self,
        mut error: anyhow::Error,
        errored_function: &Function,
        module: Option<&Module>,
        ctx: &mut Option<&mut VmContext>,
        base_frame_depth: usize,
    ) -> Result<u32> {
        let mut errored_function = errored_function;
        loop {
            if self.frames.len() == base_frame_depth {
                return Err(error);
            }
            let frame = self.frames.pop().expect("checked frames.len() above");
            self.exit_lk_call();
            let raise_message = error.downcast_ref::<LanguageRaise>().map(|raise| raise.message.clone());
            let mut caught = None;
            if let Some(message) = raise_message {
                self.handler_stack.truncate(frame.handler_depth);
                if let Some(handler) = self.handler_stack.pop() {
                    caught = Some((handler, message));
                } else {
                    // Mirrors `handle_language_raise`'s `bail!` conversion:
                    // once the immediate caller's own try-stack has been
                    // checked and found no match, this is no longer a
                    // catchable `LanguageRaise` for any further (still
                    // flattened) caller — only the single immediate hop ever
                    // got a chance, exactly like the old per-Rust-frame check.
                    error = anyhow!("{message}");
                }
            }
            if caught.is_none() {
                push_traceback_frame(ctx, errored_function);
                self.handler_stack.truncate(frame.handler_depth);
            }
            self.current_function_index = frame.function_index;
            self.frame_base = frame.frame_base;
            self.register_count = frame.register_count;
            self.state.stack_top = frame.stack_top;
            self.captures = frame.captures;
            match caught {
                Some((handler, message)) => {
                    let error_val = RuntimeVal::Obj(self.alloc_heap_value(HeapValue::ErrorVal(crate::val::ErrorVal {
                        message,
                        trace: Vec::new(),
                    })));
                    self.write(handler.catch_reg, error_val)?;
                    self.pc = handler.catch_pc;
                    return Ok(frame.function_index);
                }
                None => {
                    // Not caught here either: keep propagating. The next pop
                    // (if any) unwinds out of *this* frame's own activation,
                    // so it should name `frame.function_index` (the function
                    // we just restored into) if it's also uncaught — matching
                    // how the old recursive code named its own `function`
                    // parameter (the callee it had just invoked) at each
                    // successive Rust-recursion level.
                    errored_function = module
                        .and_then(|module| module.functions.get(frame.function_index as usize))
                        .ok_or_else(|| anyhow!("function index {} out of bounds", frame.function_index))?;
                    continue;
                }
            }
        }
    }
}
