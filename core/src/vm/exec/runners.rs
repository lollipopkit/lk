use super::*;

/// What a `catch` binds for a given error.
///
/// Three cases, and the distinction is observable: an `error(v)` raise binds
/// `v` itself, a `raise`/message raise binds the message string, and **any other
/// runtime error** also binds its message. That last case is not an extra: the
/// parse-time desugar ran the body under `pcall`, which catches every `Err`, so
/// `try { 1 % 0 } catch e` has always been caught even though `ModInt divisor is
/// zero` is a plain `bail!` and not a raise at all.
enum RaiseKind {
    Message(alloc::sync::Arc<str>),
    Value(crate::val::RuntimeVal),
}

impl RaiseKind {
    fn of(error: &anyhow::Error) -> Self {
        if let Some(raise) = error.downcast_ref::<LanguageRaise>() {
            return Self::Message(raise.message.clone());
        }
        if let Some(raised) = error.root_cause().downcast_ref::<super::handler::LkRaisedValue>() {
            return Self::Value(raised.value);
        }
        // The call machinery adds context, so the deepest cause is the message a
        // user sees — the same one `pcall` hands back.
        Self::Message(alloc::sync::Arc::<str>::from(error.root_cause().to_string().as_str()))
    }
}

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
        self.captures = Some(captures);
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
        // Objects built by this activation belong to the module running it.
        // Cheap pointer compare: the scope only changes when execution actually
        // crosses into a different module.
        if let Some(module) = module
            && !self.type_scope.is_same(&module.type_scope)
        {
            self.type_scope = module.type_scope.clone();
            self.struct_decls = module.type_info.structs.clone();
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
        error: anyhow::Error,
        errored_function: &Function,
        module: Option<&Module>,
        ctx: &mut Option<&mut VmContext>,
        base_frame_depth: usize,
    ) -> Result<u32> {
        let mut errored_function = errored_function;
        // A panic is not catchable, on any host. Checked before the handler
        // stack rather than inside the classification below, so that both the
        // same-frame case here and the unwinding loop underneath get it from
        // one place.
        if error.downcast_ref::<super::handler::LkPanic>().is_some() {
            return Err(error);
        }
        // First: a handler installed in the frame that actually faulted. Nothing
        // is popped in that case, so the loop below would never see it — this is
        // the `try { 1 % 0 } catch e` shape, where the error is a plain `bail!`
        // from the arithmetic opcode rather than a raise.
        if let Some(index) = self
            .handler_stack
            .iter()
            .rposition(|handler| handler.frame_base == self.frame_base)
        {
            let handler = self.handler_stack.remove(index);
            let value = match RaiseKind::of(&error) {
                RaiseKind::Message(message) => self.caught_message_value(message.as_ref()),
                RaiseKind::Value(value) => value,
            };
            if let Some(ctx) = ctx.as_deref_mut() {
                ctx.truncate_call_stack(0);
            }
            self.enter_handler(handler, value)?;
            return Ok(self.current_function_index);
        }
        loop {
            if self.frames.len() == base_frame_depth {
                return Err(error);
            }
            let frame = self.frames.pop().expect("checked frames.len() above");
            self.exit_lk_call();
            // Both raise flavors are catchable, and they bind different things:
            // a message-only raise binds the message *string*, an `error(v)`
            // raise binds `v` itself (see `Executor::caught_message_value`).
            // Only `LanguageRaise` used to be looked for here, so a first-class
            // raise crossing a frame boundary escaped every `TryBegin` handler.
            // Every popped frame gets a chance, at any depth. The old code let
            // only the *immediate* caller catch and then rewrote the error so no
            // outer frame could — but the desugar wrapped the whole protected
            // body in `pcall`, which catches from arbitrarily deep inside it, so
            // limiting it to one hop would lose catches that work today.
            //
            // A handler may only fire in the frame that installed it, which the
            // one-hop version got for free. Without the check, unwinding a deep
            // recursion consumed the entry frame's handler at the *first* pop and
            // ran the catch block against the wrong frame (the call-depth cap
            // then looked like it had never raised at all).
            self.handler_stack.truncate(frame.handler_depth);
            let caught = self
                .handler_stack
                .last()
                .is_some_and(|handler| handler.frame_base == frame.frame_base)
                .then(|| {
                    let handler = self.handler_stack.pop().expect("checked above");
                    (handler, RaiseKind::of(&error))
                });
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
                Some((handler, raised)) => {
                    let value = match raised {
                        RaiseKind::Message(message) => self.caught_message_value(message.as_ref()),
                        RaiseKind::Value(value) => value,
                    };
                    // A caught error leaves no traceback behind, exactly as
                    // `pcall` truncated it — a later *uncaught* error must not
                    // report frames from this one.
                    if let Some(ctx) = ctx.as_deref_mut() {
                        ctx.truncate_call_stack(0);
                    }
                    // Same entry as the same-frame catch above. `frame_base` is
                    // already `handler.frame_base` (the guard above required
                    // them equal); `stack_top` narrows from the call site's to
                    // the region's, which is the temporaries discard the
                    // same-frame path has always done.
                    self.enter_handler(handler, value)?;
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
