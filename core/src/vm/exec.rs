//! Minimal safe executor for the new `Instr` VM path.

#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
mod arithmetic;
mod call;
mod callable_ops;
mod cell;
mod const_load;
mod container;
mod dispatch;
mod frame;
mod gc;
mod globals;
mod handler;
mod imports;
mod named_call;
mod profile;
mod program;
mod return_values;
mod runtime_callable;
mod stack;
mod support;
mod value_ops;

pub use super::RuntimeCallable;
pub use imports::import_runtime_export;
pub use program::{
    ModuleFunctionArg, ModuleFunctionCall, ModuleFunctionOutcome, call_module_function_with_ctx,
    call_module_function_with_ctx_keep_state, compile_program_module_with_ctx, execute_compiled_module_with_ctx,
    execute_module_artifact_with_ctx, execute_program, execute_program_with_ctx, execute_program_with_ctx_and_budget,
    execute_program_with_ctx_and_gc_threshold, execute_program_with_ctx_and_limits, execute_source,
};
#[cfg(test)]
pub(crate) use runtime_callable::call_runtime_callable_test;
pub use runtime_callable::{
    call_runtime_callable_runtime, call_runtime_value_runtime, call_runtime_value_runtime_list_args,
    call_runtime_value_runtime_named_map, call_runtime_value_runtime_named_map_list_args,
    call_runtime_value_runtime_with_receiver, call_runtime_value_runtime_with_receiver_list_args, copy_runtime_value,
    copy_runtime_value_same_module, runtime_value_to_callable_shared,
};

use crate::util::fast_map::{FastHashMap, fast_hash_map_new};
use alloc::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::val::{
    HeapStore, HeapValue, RuntimeMapKey, RuntimeSet, RuntimeVal, TypedList, TypedMap, typed_map_from_entries,
};

use super::{
    CallWindow, Function, Module, NativeEntry, Opcode, RegisterIndex, RuntimeExport, RuntimeModuleState, VmContext,
    analysis::{
        PerfIndexTargetKind, VmCallMetric, VmContainerMetric, VmRegisterWriteSource, record_call_op_known_enabled,
        record_container_op_known_enabled, vm_runtime_metrics_enabled,
    },
};
#[cfg(test)]
use super::{Compiler, GlobalSlot};
use call::push_traceback_frame;
use frame::{CallFrame, FrameOutcome};
pub use handler::LkRaisedValue;
use handler::{ErrorHandler, LanguageRaise};
use profile::{RuntimeProfileFrame, index_metric_kind};
use return_values::ReturnValues;
use support::*;

#[derive(Debug)]
pub struct ExecResult {
    pub returns: Vec<RuntimeVal>,
    pub state: RuntimeModuleState,
}

#[derive(Debug)]
pub struct ProgramResult {
    pub returns: Vec<RuntimeVal>,
    pub state: RuntimeModuleState,
    pub module: Arc<Module>,
}

pub(crate) struct ExecFailure {
    pub error: anyhow::Error,
    pub state: RuntimeModuleState,
}

impl ProgramResult {
    pub fn first_return(&self) -> &RuntimeVal {
        self.returns.first().unwrap_or(&RuntimeVal::Nil)
    }

    pub fn first_return_list(&self) -> Result<&TypedList> {
        let RuntimeVal::Obj(handle) = self.first_return() else {
            bail!("first return is {:?}, expected list object", self.first_return().kind());
        };
        match self.state.heap.get(*handle) {
            Some(HeapValue::List(values)) => Ok(values),
            Some(other) => bail!("first return heap object is {:?}, expected list", other),
            None => bail!("first return heap object {} out of bounds", handle.index()),
        }
    }

    pub fn first_return_map(&self) -> Result<&TypedMap> {
        let RuntimeVal::Obj(handle) = self.first_return() else {
            bail!("first return is {:?}, expected map object", self.first_return().kind());
        };
        match self.state.heap.get(*handle) {
            Some(HeapValue::Map(values)) => Ok(values),
            Some(other) => bail!("first return heap object is {:?}, expected map", other),
            None => bail!("first return heap object {} out of bounds", handle.index()),
        }
    }

    pub fn into_exports(self) -> RuntimeExport {
        let mut state = self.state;
        let mut entries = fast_hash_map_new();
        for (slot, value) in self.module.globals.iter().zip(state.globals.iter()) {
            entries.insert(RuntimeMapKey::String(slot.name.clone()), *value);
        }
        let value = RuntimeVal::Obj(state.heap.alloc(HeapValue::Map(typed_map_from_entries(entries))));
        RuntimeExport::new(
            value,
            Arc::new(crate::compat::sync::Mutex::new(RuntimeModuleState::new(
                state.heap,
                state.globals,
            ))),
            self.module,
        )
    }

    /// Returns `true` if the first return value is `nil`.
    pub fn first_return_is_nil(&self) -> bool {
        matches!(self.first_return(), RuntimeVal::Nil)
    }

    /// Format the first return value as a human-readable string for REPL/CLI display.
    pub fn display_first_return(&self) -> String {
        format_runtime_val(self.first_return(), &self.state.heap, 0)
    }
}

fn format_runtime_val(value: &RuntimeVal, heap: &HeapStore, depth: usize) -> String {
    const MAX_DEPTH: usize = 8;
    match value {
        RuntimeVal::Nil => "nil".to_string(),
        RuntimeVal::Bool(b) => b.to_string(),
        RuntimeVal::Int(i) => i.to_string(),
        RuntimeVal::Float(f) => f.to_string(),
        RuntimeVal::ShortStr(s) => s.as_str().to_string(),
        RuntimeVal::Obj(handle) => {
            let Some(heap_val) = heap.get(*handle) else {
                return "<invalid ref>".to_string();
            };
            match heap_val {
                HeapValue::String(s) => s.to_string(),
                HeapValue::List(list) if depth < MAX_DEPTH => format_typed_list(list, heap, depth + 1),
                HeapValue::List(_) => "[...]".to_string(),
                HeapValue::Map(map) if depth < MAX_DEPTH => format_typed_map(map, heap, depth + 1),
                HeapValue::Map(_) => "{...}".to_string(),
                HeapValue::Set(set) if depth < MAX_DEPTH => format_runtime_set(set),
                HeapValue::Set(_) => "Set([...])".to_string(),
                HeapValue::Callable(callable) => format_callable(callable),
                HeapValue::Object(obj) => {
                    if depth < MAX_DEPTH {
                        let mut out = String::new();
                        out.push('<');
                        out.push_str(&obj.type_name);
                        out.push_str(" {");
                        let mut first = true;
                        for (key, value) in &obj.fields {
                            if !first {
                                out.push_str(", ");
                            }
                            first = false;
                            out.push_str(key);
                            out.push_str(": ");
                            out.push_str(&format_runtime_val(value, heap, depth + 1));
                        }
                        out.push_str("}>");
                        out
                    } else {
                        format!("<{} {{...}}>", obj.type_name)
                    }
                }
                _ => "<value>".to_string(),
            }
        }
    }
}

fn format_callable(callable: &crate::val::CallableValue) -> String {
    match callable {
        crate::val::CallableValue::Closure {
            function_index,
            captures,
        } => format!("<fn #{}({} captures)>", function_index, captures.len()),
        crate::val::CallableValue::RuntimeNative { name, arity, .. } => {
            if *arity == NativeEntry::VARIADIC {
                format!("<native fn {}(...)>", name)
            } else {
                format!("<native fn {}({} args)>", name, arity)
            }
        }
        crate::val::CallableValue::Runtime(function) => {
            format!(
                "<fn {} ({} captures)>",
                function.display_signature(),
                function.capture_count()
            )
        }
    }
}

fn format_typed_list(list: &TypedList, heap: &HeapStore, depth: usize) -> String {
    let mut out = String::new();
    out.push('[');
    match list {
        TypedList::Int(values) => append_display_items(&mut out, values.iter().copied()),
        TypedList::Float(values) => append_display_items(&mut out, values.iter().copied()),
        TypedList::Bool(values) => append_display_items(&mut out, values.iter().copied()),
        TypedList::String(values) => append_display_items(&mut out, values.iter().map(|value| value.as_ref())),
        TypedList::Mixed(values) => append_runtime_items(&mut out, values, heap, depth),
    }
    out.push(']');
    out
}

fn format_typed_map(map: &TypedMap, heap: &HeapStore, depth: usize) -> String {
    let mut out = String::new();
    out.push('{');
    match map {
        TypedMap::Mixed(entries) => {
            let mut first = true;
            for (key, value) in entries {
                append_separator(&mut out, &mut first);
                out.push_str(&format_map_key(key));
                out.push_str(": ");
                out.push_str(&format_runtime_val(value, heap, depth));
            }
        }
        TypedMap::StringMixed(entries) => append_string_runtime_map_entries(&mut out, entries, heap, depth),
        TypedMap::StringInt(entries) => append_string_display_map_entries(&mut out, entries),
        TypedMap::StringFloat(entries) => append_string_display_map_entries(&mut out, entries),
        TypedMap::StringBool(entries) => append_string_display_map_entries(&mut out, entries),
    }
    out.push('}');
    out
}

fn format_runtime_set(set: &RuntimeSet) -> String {
    let mut out = String::from("Set([");
    let mut first = true;
    for value in set.entries() {
        append_separator(&mut out, &mut first);
        out.push_str(&format_map_key(value));
    }
    out.push_str("])");
    out
}

fn append_separator(out: &mut String, first: &mut bool) {
    if !*first {
        out.push_str(", ");
    }
    *first = false;
}

fn append_display_items<T: core::fmt::Display>(out: &mut String, values: impl IntoIterator<Item = T>) {
    let mut first = true;
    for value in values {
        append_separator(out, &mut first);
        out.push_str(&value.to_string());
    }
}

fn append_runtime_items(out: &mut String, values: &[RuntimeVal], heap: &HeapStore, depth: usize) {
    let mut first = true;
    for value in values {
        append_separator(out, &mut first);
        out.push_str(&format_runtime_val(value, heap, depth));
    }
}

fn append_string_runtime_map_entries(
    out: &mut String,
    entries: &FastHashMap<Arc<str>, RuntimeVal>,
    heap: &HeapStore,
    depth: usize,
) {
    let mut first = true;
    for (key, value) in entries {
        append_separator(out, &mut first);
        out.push_str(key);
        out.push_str(": ");
        out.push_str(&format_runtime_val(value, heap, depth));
    }
}

fn append_string_display_map_entries<T: core::fmt::Display>(out: &mut String, entries: &FastHashMap<Arc<str>, T>) {
    let mut first = true;
    for (key, value) in entries {
        append_separator(out, &mut first);
        out.push_str(key);
        out.push_str(": ");
        out.push_str(&value.to_string());
    }
}

fn format_map_key(key: &RuntimeMapKey) -> String {
    match key {
        RuntimeMapKey::Nil => "nil".to_string(),
        RuntimeMapKey::Bool(b) => b.to_string(),
        RuntimeMapKey::Int(i) => i.to_string(),
        RuntimeMapKey::ShortStr(s) => s.as_str().to_string(),
        RuntimeMapKey::String(s) => s.to_string(),
        RuntimeMapKey::Obj(h) => format!("<obj:{}>", h.index()),
    }
}

#[derive(Debug)]
pub struct Executor {
    state: RuntimeModuleState,
    captures: Arc<Vec<RuntimeVal>>,
    empty_captures: Arc<Vec<RuntimeVal>>,
    handler_stack: Vec<ErrorHandler>,
    frame_base: usize,
    register_count: u16,
    pc: usize,
    /// The function index currently executing at `frame_base`/`pc`, kept in
    /// sync at every push/pop so the flattened dispatch trampoline
    /// (`run_function_inner_impl`) knows which `Function`/`code` to resume
    /// without needing to search `frames` or `module` (plan M2.5 sub-step ①).
    current_function_index: u32,
    /// Suspended caller activations for flattened LK→LK calls (`CallDirect`,
    /// `Call`, and `CallNamed` when the target is a closure): LK call depth
    /// grows this `Vec` instead of the Rust stack (plan M2.5 sub-steps ①/②).
    /// `CallMethodK` and native/runtime re-entry (`pcall`, stdlib HOFs, the
    /// `Runtime` callable family) are unaffected by design — they run on a
    /// fresh, separate `Executor`/Rust call, not `self`, so there is nothing
    /// here for them to flatten.
    frames: Vec<CallFrame>,
    collect_metrics: bool,
    gc_pending: bool,
    /// `LK_GC_STRESS=1` forces a full collection at every GC safepoint instead
    /// of waiting for the heap threshold. Root-enumeration gaps then surface
    /// deterministically as use-after-collect failures in any test run instead
    /// of as rare allocation-timing-dependent corruption.
    gc_stress: bool,
    shared_module: Option<Arc<Module>>,
    instruction_budget: Option<u64>,
    instruction_count: u64,
    /// Optional cap on the number of live heap objects (sandbox memory bound).
    heap_object_limit: Option<usize>,
    /// Cap on live LK call depth (`RuntimeModuleState::call_depth`), checked
    /// by `enter_lk_call`/`exit_lk_call` at every `CallFrame` push/pop
    /// (`CallDirect`/`Call`/`CallNamed` to a closure, plan M2.5 sub-steps
    /// ①/②) and at the `run_module_function_with_state_recoverable` native
    /// entry boundary. Beyond it, a *catchable* error is raised instead of
    /// exhausting memory. Native re-entry that recurses through a *separate*
    /// `Executor` (`pcall`, stdlib HOFs, `CallMethodK`'s callable-property/
    /// trait-method/list-HOF tails, in `runtime_callable.rs`) isn't counted
    /// here and has no segmented-stack backstop either — unchanged from
    /// before this plan, and out of M2.5's LK→LK-call scope.
    max_call_depth: usize,
}

/// Default LK call-depth cap. Far beyond any sane program (Python defaults to
/// 1000), small enough that infinite recursion errors before exhausting
/// memory on stack segments. `LK_MAX_CALL_DEPTH` overrides it (consulted only
/// on the exceed path, so the hot path never reads the environment).
const DEFAULT_MAX_CALL_DEPTH: usize = 100_000;

/// Red zone / segment size (rustc's `ensure_sufficient_stack` pattern): when
/// fewer than 128KiB of Rust stack remain, run on a fresh 2MiB segment
/// instead of overflowing. Since plan M2.5 sub-steps ①/②, `CallDirect`/
/// `Call`/`CallNamed` to a closure no longer recurse through Rust at all (LK
/// call depth grows `Executor::frames` instead) — this now only guards the
/// `run_module_function_with_state_recoverable` native-entry boundary, which
/// can still nest if a host callback re-enters the VM from inside a native
/// call. Deep *LK* recursion measured before the M2.5 flattening: ~150 frames
/// (debug) / ~4000 (release) to a hard abort. no_std targets keep plain
/// recursion (their stack discipline is platform-specific).
#[cfg(feature = "std")]
#[inline]
fn grow_stack_if_needed<R>(f: impl FnOnce() -> R) -> R {
    stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, f)
}

#[cfg(not(feature = "std"))]
#[inline]
fn grow_stack_if_needed<R>(f: impl FnOnce() -> R) -> R {
    f()
}

impl Executor {
    #[inline]
    pub fn new(register_count: u16) -> Self {
        let mut this = Self {
            state: RuntimeModuleState::default(),
            captures: Arc::new(Vec::new()),
            empty_captures: Arc::new(Vec::new()),
            handler_stack: Vec::new(),
            frame_base: 0,
            register_count,
            pc: 0,
            current_function_index: 0,
            frames: Vec::new(),
            collect_metrics: false,
            gc_pending: false,
            gc_stress: gc_stress_enabled(),
            shared_module: None,
            instruction_budget: None,
            instruction_count: 0,
            heap_object_limit: None,
            max_call_depth: DEFAULT_MAX_CALL_DEPTH,
        };
        this.reset_entry_frame(register_count);
        this
    }

    pub fn with_instruction_budget(mut self, budget: u64) -> Self {
        self.instruction_budget = Some(budget);
        self
    }

    /// Cap live LK call depth (default [`DEFAULT_MAX_CALL_DEPTH`]); recursion
    /// beyond it raises a catchable error instead of exhausting memory.
    pub fn with_max_call_depth(mut self, limit: usize) -> Self {
        self.max_call_depth = limit;
        self
    }

    /// Depth check at every LK call boundary. The env override is consulted
    /// only on the exceed path (cold), so the hot path costs one compare.
    #[inline]
    pub(super) fn enter_lk_call(&mut self) -> Result<()> {
        if self.state.call_depth >= self.max_call_depth {
            self.call_depth_exceeded()?;
        }
        self.state.call_depth += 1;
        Ok(())
    }

    #[inline]
    pub(super) fn exit_lk_call(&mut self) {
        self.state.call_depth = self.state.call_depth.saturating_sub(1);
    }

    #[cold]
    fn call_depth_exceeded(&mut self) -> Result<()> {
        #[cfg(feature = "std")]
        if let Some(raised) = std::env::var("LK_MAX_CALL_DEPTH")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            && raised > self.max_call_depth
        {
            self.max_call_depth = raised;
            if self.state.call_depth < self.max_call_depth {
                return Ok(());
            }
        }
        bail!(
            "call depth limit exceeded ({}); set LK_MAX_CALL_DEPTH to raise it",
            self.max_call_depth
        )
    }

    /// Cap the number of live heap objects (a coarse memory bound for the
    /// sandbox model, plan M2.6). Checked at the same per-instruction cadence as
    /// the instruction budget, so it is zero-cost when unset.
    pub fn with_heap_object_limit(mut self, limit: usize) -> Self {
        self.heap_object_limit = Some(limit);
        self
    }

    #[inline]
    fn consume_instruction(&mut self) -> Result<()> {
        self.instruction_count = self
            .instruction_count
            .checked_add(1)
            .ok_or_else(|| anyhow!("instruction counter overflow"))?;
        if let Some(budget) = self.instruction_budget
            && self.instruction_count > budget
        {
            bail!("execution step limit exceeded ({budget} instructions)");
        }
        if let Some(limit) = self.heap_object_limit
            && self.state.heap.len() > limit
        {
            bail!("heap object limit exceeded ({limit} objects)");
        }
        Ok(())
    }

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

    fn finish(self, returns: Vec<RuntimeVal>) -> ExecResult {
        ExecResult {
            returns,
            state: self.state,
        }
    }

    fn run_function_inner(
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
    fn run_function_inner_impl<const BUDGETED: bool>(
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

    fn dispatch_within_frame<const BUDGETED: bool>(
        &mut self,
        function: &Function,
        module: Option<&Module>,
        ctx: &mut Option<&mut VmContext>,
        base_frame_depth: usize,
    ) -> Result<FrameOutcome> {
        let collect_metrics = vm_runtime_metrics_enabled();
        self.collect_metrics = collect_metrics;
        let code = &function.code;
        let mut profile = RuntimeProfileFrame::new();
        while self.pc < code.len() {
            if BUDGETED {
                self.consume_instruction()?;
            }
            let instr = code[self.pc];
            let opcode = instr.opcode();
            profile.record_opcode(opcode, collect_metrics);
            match opcode {
                Opcode::Nop => self.pc += 1,
                Opcode::LoadNil
                | Opcode::LoadBool
                | Opcode::LoadInt
                | Opcode::LoadFloat
                | Opcode::LoadString
                | Opcode::LoadHeapConst => {
                    if collect_metrics && !function.performance.is_dead_write(self.pc) {
                        profile.record_write_source(VmRegisterWriteSource::ConstLoad, collect_metrics);
                    }
                    self.load_const_instr(function, instr)?;
                }
                Opcode::Move => loop {
                    let instr = code[self.pc];
                    let value = *self.read_unchecked(instr.b());
                    self.write_unchecked(instr.a(), value);
                    self.pc += 1;
                    if self.pc >= code.len() || code[self.pc].opcode() != Opcode::Move {
                        break;
                    }
                    if BUDGETED {
                        self.consume_instruction()?;
                    }
                },
                Opcode::Move2 => {
                    let first = *self.read_unchecked(instr.b());
                    self.write_unchecked(instr.a(), first);
                    let second = *self.read_unchecked(instr.c());
                    self.write_unchecked(instr.b(), second);
                    self.pc += 1;
                }
                Opcode::LoadCapture => {
                    self.dispatch_cold(Opcode::LoadCapture, function, module, instr, ctx, collect_metrics)?;
                    let _ = &profile; // suppress unused warning
                }
                Opcode::LoadCellVal => {
                    self.dispatch_cold(Opcode::LoadCellVal, function, module, instr, ctx, collect_metrics)?;
                    let _ = &profile; // suppress unused warning
                }
                Opcode::StoreCellVal => {
                    self.dispatch_cold(Opcode::StoreCellVal, function, module, instr, ctx, collect_metrics)?
                }
                Opcode::LoadFunction => {
                    self.dispatch_cold(Opcode::LoadFunction, function, module, instr, ctx, collect_metrics)?;
                    let _ = &profile; // suppress unused warning
                }
                Opcode::MakeClosure => {
                    self.dispatch_cold(Opcode::MakeClosure, function, module, instr, ctx, collect_metrics)?;
                    let _ = &profile; // suppress unused warning
                }
                Opcode::LoadNative => {
                    self.dispatch_cold(Opcode::LoadNative, function, module, instr, ctx, collect_metrics)?;
                    let _ = &profile; // suppress unused warning
                }
                Opcode::AddInt => {
                    let (dst, lhs_idx, rhs_idx) = self.stack_abc_unchecked(instr);
                    let lhs = &self.state.stack[lhs_idx];
                    let rhs = &self.state.stack[rhs_idx];
                    match (lhs, rhs) {
                        (RuntimeVal::Int(l), RuntimeVal::Int(r)) => {
                            self.state.stack[dst] = RuntimeVal::Int(l.wrapping_add(*r));
                            profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                            self.pc += 1;
                        }
                        _ => {
                            self.dynamic_add(instr)?;
                            profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                        }
                    }
                }
                Opcode::AddIntI => {
                    let dst = self.frame_base + instr.a() as usize;
                    let lhs_idx = self.frame_base + instr.b() as usize;
                    match &self.state.stack[lhs_idx] {
                        RuntimeVal::Int(lhs) => {
                            self.state.stack[dst] = RuntimeVal::Int(lhs.wrapping_add(instr.sc() as i64));
                            profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                            self.pc += 1;
                        }
                        lhs => bail!("AddIntI expected Int lhs, got {:?}", lhs.kind()),
                    }
                }
                Opcode::MulIntI => {
                    let dst = self.frame_base + instr.a() as usize;
                    let lhs_idx = self.frame_base + instr.b() as usize;
                    match &self.state.stack[lhs_idx] {
                        RuntimeVal::Int(lhs) => {
                            self.state.stack[dst] = RuntimeVal::Int(lhs.wrapping_mul(instr.sc() as i64));
                            profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                            self.pc += 1;
                        }
                        lhs => bail!("MulIntI expected Int lhs, got {:?}", lhs.kind()),
                    }
                }
                Opcode::ModIntI => {
                    let dst = self.frame_base + instr.a() as usize;
                    let lhs_idx = self.frame_base + instr.b() as usize;
                    let rhs = instr.sc() as i64;
                    if rhs == 0 {
                        bail!("ModIntI divisor is zero");
                    }
                    match &self.state.stack[lhs_idx] {
                        RuntimeVal::Int(lhs) => {
                            let value = *lhs % rhs;
                            self.state.stack[dst] = RuntimeVal::Int(value);
                            profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                            if !self.try_apply_next_zero_branch_for_written_int(code, instr.a(), value) {
                                self.pc += 1;
                            }
                        }
                        lhs => bail!("ModIntI expected Int lhs, got {:?}", lhs.kind()),
                    }
                }
                Opcode::MinInt => {
                    let (dst, lhs_idx, rhs_idx) = self.stack_abc_unchecked(instr);
                    match (&self.state.stack[lhs_idx], &self.state.stack[rhs_idx]) {
                        (RuntimeVal::Int(lhs), RuntimeVal::Int(rhs)) => {
                            self.state.stack[dst] = RuntimeVal::Int((*lhs).min(*rhs));
                            profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                            self.pc += 1;
                        }
                        (lhs, rhs) => bail!(
                            "MinInt expected Int operands, got {:?} and {:?}",
                            lhs.kind(),
                            rhs.kind()
                        ),
                    }
                }
                Opcode::MaxInt => {
                    let (dst, lhs_idx, rhs_idx) = self.stack_abc_unchecked(instr);
                    match (&self.state.stack[lhs_idx], &self.state.stack[rhs_idx]) {
                        (RuntimeVal::Int(lhs), RuntimeVal::Int(rhs)) => {
                            self.state.stack[dst] = RuntimeVal::Int((*lhs).max(*rhs));
                            profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                            self.pc += 1;
                        }
                        (lhs, rhs) => bail!(
                            "MaxInt expected Int operands, got {:?} and {:?}",
                            lhs.kind(),
                            rhs.kind()
                        ),
                    }
                }
                Opcode::AddMulInt => {
                    let (acc_idx, lhs_idx, rhs_idx) = self.stack_abc_unchecked(instr);
                    match (
                        &self.state.stack[acc_idx],
                        &self.state.stack[lhs_idx],
                        &self.state.stack[rhs_idx],
                    ) {
                        (RuntimeVal::Int(acc), RuntimeVal::Int(lhs), RuntimeVal::Int(rhs)) => {
                            self.state.stack[acc_idx] = RuntimeVal::Int(acc.wrapping_add(lhs.wrapping_mul(*rhs)));
                            profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                            self.pc += 1;
                        }
                        (acc, lhs, rhs) => bail!(
                            "AddMulInt expected Int operands, got {:?}, {:?}, and {:?}",
                            acc.kind(),
                            lhs.kind(),
                            rhs.kind()
                        ),
                    }
                }
                Opcode::Add2Int => {
                    let (acc_idx, lhs_idx, rhs_idx) = self.stack_abc_unchecked(instr);
                    match (
                        &self.state.stack[acc_idx],
                        &self.state.stack[lhs_idx],
                        &self.state.stack[rhs_idx],
                    ) {
                        (RuntimeVal::Int(acc), RuntimeVal::Int(lhs), RuntimeVal::Int(rhs)) => {
                            self.state.stack[acc_idx] = RuntimeVal::Int(acc.wrapping_add(*lhs).wrapping_add(*rhs));
                            profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                            self.pc += 1;
                        }
                        (acc, lhs, rhs) => bail!(
                            "Add2Int expected Int operands, got {:?}, {:?}, and {:?}",
                            acc.kind(),
                            lhs.kind(),
                            rhs.kind()
                        ),
                    }
                }
                Opcode::MidInt => {
                    let (dst, lhs_idx, rhs_idx) = self.stack_abc_unchecked(instr);
                    match (&self.state.stack[lhs_idx], &self.state.stack[rhs_idx]) {
                        (RuntimeVal::Int(lhs), RuntimeVal::Int(rhs)) => {
                            self.state.stack[dst] = RuntimeVal::Int(lhs.wrapping_add(*rhs) / 2);
                            profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                            self.pc += 1;
                        }
                        (lhs, rhs) => bail!(
                            "MidInt expected Int operands, got {:?} and {:?}",
                            lhs.kind(),
                            rhs.kind()
                        ),
                    }
                }
                Opcode::AddListInt | Opcode::SubListInt => {
                    let acc_idx = self.stack_index_unchecked(instr.a());
                    let RuntimeVal::Int(acc) = self.state.stack[acc_idx] else {
                        bail!(
                            "{:?} expected Int accumulator, got {:?}",
                            instr.opcode(),
                            self.state.stack[acc_idx].kind()
                        );
                    };
                    let item = self.read_known_int_list_index(instr.b(), instr.c())?;
                    let value = if instr.opcode() == Opcode::AddListInt {
                        acc.wrapping_add(item)
                    } else {
                        acc.wrapping_sub(item)
                    };
                    self.state.stack[acc_idx] = RuntimeVal::Int(value);
                    profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                    self.pc += 1;
                }
                Opcode::SubInt => {
                    let (dst, lhs_idx, rhs_idx) = self.stack_abc_unchecked(instr);
                    let lhs = &self.state.stack[lhs_idx];
                    let rhs = &self.state.stack[rhs_idx];
                    match (lhs, rhs) {
                        (RuntimeVal::Int(l), RuntimeVal::Int(r)) => {
                            self.state.stack[dst] = RuntimeVal::Int(l.wrapping_sub(*r));
                            profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                            self.pc += 1;
                        }
                        _ => {
                            self.dynamic_sub(instr)?;
                            profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                        }
                    }
                }
                Opcode::MulInt => {
                    let (dst, lhs_idx, rhs_idx) = self.stack_abc_unchecked(instr);
                    let lhs = &self.state.stack[lhs_idx];
                    let rhs = &self.state.stack[rhs_idx];
                    match (lhs, rhs) {
                        (RuntimeVal::Int(l), RuntimeVal::Int(r)) => {
                            self.state.stack[dst] = RuntimeVal::Int(l.wrapping_mul(*r));
                            profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                            self.pc += 1;
                        }
                        _ => {
                            self.dynamic_numeric_binary(instr, |lhs, rhs| lhs.wrapping_mul(rhs), |lhs, rhs| lhs * rhs)?;
                            profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                        }
                    }
                }
                Opcode::DivInt => {
                    let (dst, lhs_idx, rhs_idx) = self.stack_abc_unchecked(instr);
                    let lhs = &self.state.stack[lhs_idx];
                    let rhs = &self.state.stack[rhs_idx];
                    match (lhs, rhs) {
                        (RuntimeVal::Int(_), RuntimeVal::Int(0)) => bail!("DivInt divisor is zero"),
                        (RuntimeVal::Int(l), RuntimeVal::Int(r)) => {
                            self.state.stack[dst] = RuntimeVal::Int(*l / *r);
                            profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                            self.pc += 1;
                        }
                        _ => {
                            self.dynamic_div(instr)?;
                            profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                        }
                    }
                }
                Opcode::ModInt => {
                    let (dst, lhs_idx, rhs_idx) = self.stack_abc_unchecked(instr);
                    let lhs = &self.state.stack[lhs_idx];
                    let rhs = &self.state.stack[rhs_idx];
                    match (lhs, rhs) {
                        (RuntimeVal::Int(_), RuntimeVal::Int(0)) => bail!("ModInt divisor is zero"),
                        (RuntimeVal::Int(l), RuntimeVal::Int(r)) => {
                            let value = *l % *r;
                            self.state.stack[dst] = RuntimeVal::Int(value);
                            profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                            if !self.try_apply_next_zero_branch_for_written_int(code, instr.a(), value) {
                                self.pc += 1;
                            }
                        }
                        _ => {
                            self.dynamic_mod(instr)?;
                            profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                        }
                    }
                }
                Opcode::AddFloat => {
                    self.float_binary(instr, |lhs, rhs| lhs + rhs)?;
                    profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                }
                Opcode::SubFloat => {
                    self.float_binary(instr, |lhs, rhs| lhs - rhs)?;
                    profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                }
                Opcode::MulFloat => {
                    self.float_binary(instr, |lhs, rhs| lhs * rhs)?;
                    profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                }
                Opcode::DivFloat => {
                    let lhs = self.read_number_unchecked(instr.b());
                    let rhs = self.read_number_unchecked(instr.c());
                    if rhs == 0.0 {
                        bail!("DivFloat divisor is zero");
                    }
                    self.write_unchecked(instr.a(), RuntimeVal::Float(lhs / rhs));
                    profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                    self.pc += 1;
                }
                Opcode::ModFloat => {
                    let lhs = self.read_number_unchecked(instr.b());
                    let rhs = self.read_number_unchecked(instr.c());
                    if rhs == 0.0 {
                        bail!("ModFloat divisor is zero");
                    }
                    self.write_unchecked(instr.a(), RuntimeVal::Float(lhs % rhs));
                    profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                    self.pc += 1;
                }
                Opcode::Not => {
                    self.dispatch_cold(Opcode::Not, function, module, instr, ctx, collect_metrics)?;
                }
                Opcode::IsNil => {
                    self.dispatch_cold(Opcode::IsNil, function, module, instr, ctx, collect_metrics)?;
                }
                Opcode::IsList => {
                    self.dispatch_cold(Opcode::IsList, function, module, instr, ctx, collect_metrics)?;
                }
                Opcode::IsMap => {
                    self.dispatch_cold(Opcode::IsMap, function, module, instr, ctx, collect_metrics)?;
                }
                Opcode::ToString => {
                    self.dispatch_cold(Opcode::ToString, function, module, instr, ctx, collect_metrics)?;
                }
                Opcode::ConcatString => {
                    self.dispatch_concat_string(instr, module, ctx)?;
                    profile.record_write_source(VmRegisterWriteSource::String, collect_metrics);
                }
                Opcode::ConcatN => {
                    self.dispatch_concat_n(instr, module, ctx)?;
                    profile.record_write_source(VmRegisterWriteSource::String, collect_metrics);
                }
                Opcode::StringSplit => {
                    self.dispatch_cold(Opcode::StringSplit, function, module, instr, ctx, collect_metrics)?;
                }
                Opcode::ListJoin => {
                    self.dispatch_cold(Opcode::ListJoin, function, module, instr, ctx, collect_metrics)?;
                }
                Opcode::CmpInt => {
                    let (_dst, lhs_idx, rhs_idx) = self.stack_abc_unchecked(instr);
                    let equal = match (&self.state.stack[lhs_idx], &self.state.stack[rhs_idx]) {
                        (RuntimeVal::Int(l), RuntimeVal::Int(r)) => l == r,
                        (RuntimeVal::Int(l), RuntimeVal::Float(r)) => (*l as f64) == *r,
                        (RuntimeVal::Float(l), RuntimeVal::Int(r)) => *l == (*r as f64),
                        (RuntimeVal::Float(l), RuntimeVal::Float(r)) => l == r,
                        (RuntimeVal::Bool(l), RuntimeVal::Bool(r)) => l == r,
                        (RuntimeVal::ShortStr(l), RuntimeVal::ShortStr(r)) => l == r,
                        (RuntimeVal::Nil, RuntimeVal::Nil) => true,
                        (RuntimeVal::Nil, _) | (_, RuntimeVal::Nil) => false,
                        (RuntimeVal::Obj(l), RuntimeVal::Obj(r)) if l == r => true,
                        _ => self.values_equal(instr.b(), instr.c())?,
                    };
                    if self.try_fused_bool_branch(function, instr.a(), equal, collect_metrics)? {
                        continue;
                    }
                    self.write_unchecked(instr.a(), RuntimeVal::Bool(equal));
                    profile.record_write_source(VmRegisterWriteSource::Compare, collect_metrics);
                    self.pc += 1;
                }
                Opcode::CmpNeInt => {
                    let (_dst, lhs_idx, rhs_idx) = self.stack_abc_unchecked(instr);
                    let equal = match (&self.state.stack[lhs_idx], &self.state.stack[rhs_idx]) {
                        (RuntimeVal::Int(l), RuntimeVal::Int(r)) => l == r,
                        (RuntimeVal::Int(l), RuntimeVal::Float(r)) => (*l as f64) == *r,
                        (RuntimeVal::Float(l), RuntimeVal::Int(r)) => *l == (*r as f64),
                        (RuntimeVal::Float(l), RuntimeVal::Float(r)) => l == r,
                        (RuntimeVal::Bool(l), RuntimeVal::Bool(r)) => l == r,
                        (RuntimeVal::ShortStr(l), RuntimeVal::ShortStr(r)) => l == r,
                        (RuntimeVal::Nil, RuntimeVal::Nil) => true,
                        (RuntimeVal::Nil, _) | (_, RuntimeVal::Nil) => false,
                        (RuntimeVal::Obj(l), RuntimeVal::Obj(r)) if l == r => true,
                        _ => self.values_equal(instr.b(), instr.c())?,
                    };
                    if self.try_fused_bool_branch(function, instr.a(), !equal, collect_metrics)? {
                        continue;
                    }
                    self.write_unchecked(instr.a(), RuntimeVal::Bool(!equal));
                    profile.record_write_source(VmRegisterWriteSource::Compare, collect_metrics);
                    self.pc += 1;
                }
                Opcode::CmpLtInt => {
                    let (dst, lhs_idx, rhs_idx) = self.stack_abc_unchecked(instr);
                    match (&self.state.stack[lhs_idx], &self.state.stack[rhs_idx]) {
                        (RuntimeVal::Int(l), RuntimeVal::Int(r)) => {
                            let value = l < r;
                            if self.try_fused_bool_branch(function, instr.a(), value, collect_metrics)? {
                                continue;
                            }
                            self.state.stack[dst] = RuntimeVal::Bool(value);
                            profile.record_write_source(VmRegisterWriteSource::Compare, collect_metrics);
                            self.pc += 1;
                        }
                        _ => {
                            if self.try_fused_compare_branch(function, instr, collect_metrics)? {
                                continue;
                            }
                            self.number_compare(instr, |lhs, rhs| lhs < rhs, |lhs, rhs| lhs < rhs)?;
                            profile.record_write_source(VmRegisterWriteSource::Compare, collect_metrics);
                        }
                    }
                }
                Opcode::CmpLeInt => {
                    let (dst, lhs_idx, rhs_idx) = self.stack_abc_unchecked(instr);
                    match (&self.state.stack[lhs_idx], &self.state.stack[rhs_idx]) {
                        (RuntimeVal::Int(l), RuntimeVal::Int(r)) => {
                            let value = l <= r;
                            if self.try_fused_bool_branch(function, instr.a(), value, collect_metrics)? {
                                continue;
                            }
                            self.state.stack[dst] = RuntimeVal::Bool(value);
                            profile.record_write_source(VmRegisterWriteSource::Compare, collect_metrics);
                            self.pc += 1;
                        }
                        _ => {
                            if self.try_fused_compare_branch(function, instr, collect_metrics)? {
                                continue;
                            }
                            self.number_compare(instr, |lhs, rhs| lhs <= rhs, |lhs, rhs| lhs <= rhs)?;
                            profile.record_write_source(VmRegisterWriteSource::Compare, collect_metrics);
                        }
                    }
                }
                Opcode::CmpGtInt => {
                    let (dst, lhs_idx, rhs_idx) = self.stack_abc_unchecked(instr);
                    match (&self.state.stack[lhs_idx], &self.state.stack[rhs_idx]) {
                        (RuntimeVal::Int(l), RuntimeVal::Int(r)) => {
                            let value = l > r;
                            if self.try_fused_bool_branch(function, instr.a(), value, collect_metrics)? {
                                continue;
                            }
                            self.state.stack[dst] = RuntimeVal::Bool(value);
                            profile.record_write_source(VmRegisterWriteSource::Compare, collect_metrics);
                            self.pc += 1;
                        }
                        _ => {
                            if self.try_fused_compare_branch(function, instr, collect_metrics)? {
                                continue;
                            }
                            self.number_compare(instr, |lhs, rhs| lhs > rhs, |lhs, rhs| lhs > rhs)?;
                            profile.record_write_source(VmRegisterWriteSource::Compare, collect_metrics);
                        }
                    }
                }
                Opcode::CmpGeInt => {
                    let (dst, lhs_idx, rhs_idx) = self.stack_abc_unchecked(instr);
                    match (&self.state.stack[lhs_idx], &self.state.stack[rhs_idx]) {
                        (RuntimeVal::Int(l), RuntimeVal::Int(r)) => {
                            let value = l >= r;
                            if self.try_fused_bool_branch(function, instr.a(), value, collect_metrics)? {
                                continue;
                            }
                            self.state.stack[dst] = RuntimeVal::Bool(value);
                            profile.record_write_source(VmRegisterWriteSource::Compare, collect_metrics);
                            self.pc += 1;
                        }
                        _ => {
                            if self.try_fused_compare_branch(function, instr, collect_metrics)? {
                                continue;
                            }
                            self.number_compare(instr, |lhs, rhs| lhs >= rhs, |lhs, rhs| lhs >= rhs)?;
                            profile.record_write_source(VmRegisterWriteSource::Compare, collect_metrics);
                        }
                    }
                }
                Opcode::Contains => {
                    self.dispatch_cold(Opcode::Contains, function, module, instr, ctx, collect_metrics)?;
                }
                Opcode::SliceFrom => {
                    self.dispatch_cold(Opcode::SliceFrom, function, module, instr, ctx, collect_metrics)?;
                }
                Opcode::MapRest => {
                    self.dispatch_cold(Opcode::MapRest, function, module, instr, ctx, collect_metrics)?;
                }
                Opcode::Raise => self.dispatch_cold(Opcode::Raise, function, module, instr, ctx, collect_metrics)?,
                Opcode::TryBegin => {
                    self.dispatch_cold(Opcode::TryBegin, function, module, instr, ctx, collect_metrics)?
                }
                Opcode::TryEnd => {
                    self.dispatch_cold(Opcode::TryEnd, function, module, instr, ctx, collect_metrics)?;
                }
                Opcode::Test => self.dispatch_cold(Opcode::Test, function, module, instr, ctx, collect_metrics)?,
                Opcode::BrFalse => {
                    self.dispatch_cold(Opcode::BrFalse, function, module, instr, ctx, collect_metrics)?
                }
                Opcode::BrTrue => self.dispatch_cold(Opcode::BrTrue, function, module, instr, ctx, collect_metrics)?,
                Opcode::BrNil => {
                    let index = self.stack_index_unchecked(instr.a());
                    if matches!(self.state.stack[index], RuntimeVal::Nil) {
                        self.pc = self.relative_pc_unchecked(instr.sbx() as i32);
                    } else if !self.try_apply_fallthrough_move_jump(code) {
                        self.pc += 1;
                    }
                }
                Opcode::BrNotNil => {
                    let index = self.stack_index_unchecked(instr.a());
                    if !matches!(self.state.stack[index], RuntimeVal::Nil) {
                        self.pc = self.relative_pc_unchecked(instr.sbx() as i32);
                    } else if !self.try_apply_fallthrough_move_jump(code) {
                        self.pc += 1;
                    }
                }
                Opcode::BrEqZeroInt => {
                    let index = self.stack_index_unchecked(instr.a());
                    match &self.state.stack[index] {
                        RuntimeVal::Int(value) if *value == 0 => {
                            self.pc = self.relative_pc_unchecked(instr.sbx() as i32);
                        }
                        RuntimeVal::Int(_) => {
                            if !self.try_apply_fallthrough_move_jump(code) {
                                self.pc += 1;
                            }
                        }
                        value => bail!("BrEqZeroInt expected Int operand, got {:?}", value.kind()),
                    }
                }
                Opcode::BrNeZeroInt => {
                    let index = self.stack_index_unchecked(instr.a());
                    match &self.state.stack[index] {
                        RuntimeVal::Int(value) if *value != 0 => {
                            self.pc = self.relative_pc_unchecked(instr.sbx() as i32);
                        }
                        RuntimeVal::Int(_) => {
                            if !self.try_apply_fallthrough_move_jump(code) {
                                self.pc += 1;
                            }
                        }
                        value => bail!("BrNeZeroInt expected Int operand, got {:?}", value.kind()),
                    }
                }
                Opcode::BrEqIntI4 => {
                    let index = self.stack_index_unchecked(instr.a());
                    let rhs = i64::from(instr.branch_i4_immediate());
                    match &self.state.stack[index] {
                        RuntimeVal::Int(value) if *value == rhs => {
                            self.pc = self.relative_pc_unchecked(instr.branch_i4_offset() as i32);
                        }
                        RuntimeVal::Int(_) => {
                            if !self.try_apply_fallthrough_move_jump(code) {
                                self.pc += 1;
                            }
                        }
                        value => bail!("BrEqIntI4 expected Int operand, got {:?}", value.kind()),
                    }
                }
                Opcode::BrNeIntI4 => {
                    let index = self.stack_index_unchecked(instr.a());
                    let rhs = i64::from(instr.branch_i4_immediate());
                    match &self.state.stack[index] {
                        RuntimeVal::Int(value) if *value != rhs => {
                            self.pc = self.relative_pc_unchecked(instr.branch_i4_offset() as i32);
                        }
                        RuntimeVal::Int(_) => {
                            if !self.try_apply_fallthrough_move_jump(code) {
                                self.pc += 1;
                            }
                        }
                        value => bail!("BrNeIntI4 expected Int operand, got {:?}", value.kind()),
                    }
                }
                Opcode::BrModEqZeroIntI4 => {
                    let index = self.stack_index_unchecked(instr.a());
                    let divisor = i64::from(instr.branch_i4_immediate());
                    if divisor == 0 {
                        bail!("BrModEqZeroIntI4 divisor is zero");
                    }
                    match &self.state.stack[index] {
                        RuntimeVal::Int(value) if *value % divisor == 0 => {
                            self.pc = self.relative_pc_unchecked(instr.branch_i4_offset() as i32);
                        }
                        RuntimeVal::Int(_) => {
                            if !self.try_apply_fallthrough_move_jump(code) {
                                self.pc += 1;
                            }
                        }
                        value => bail!("BrModEqZeroIntI4 expected Int operand, got {:?}", value.kind()),
                    }
                }
                Opcode::BrModNeZeroIntI4 => {
                    let index = self.stack_index_unchecked(instr.a());
                    let divisor = i64::from(instr.branch_i4_immediate());
                    if divisor == 0 {
                        bail!("BrModNeZeroIntI4 divisor is zero");
                    }
                    match &self.state.stack[index] {
                        RuntimeVal::Int(value) if *value % divisor != 0 => {
                            self.pc = self.relative_pc_unchecked(instr.branch_i4_offset() as i32);
                        }
                        RuntimeVal::Int(_) => {
                            if !self.try_apply_fallthrough_move_jump(code) {
                                self.pc += 1;
                            }
                        }
                        value => bail!("BrModNeZeroIntI4 expected Int operand, got {:?}", value.kind()),
                    }
                }
                Opcode::TestEqInt => {
                    let lhs_idx = self.stack_index_unchecked(instr.a());
                    let rhs_idx = self.stack_index_unchecked(instr.b());
                    let value = match (&self.state.stack[lhs_idx], &self.state.stack[rhs_idx]) {
                        (RuntimeVal::Int(lhs), RuntimeVal::Int(rhs)) => lhs == rhs,
                        _ => self.compare_test_value_slow(instr, lhs_idx, rhs_idx)?,
                    };
                    self.apply_compare_test_branch_unchecked(function, code, instr, value);
                }
                Opcode::TestNeInt => {
                    let lhs_idx = self.stack_index_unchecked(instr.a());
                    let rhs_idx = self.stack_index_unchecked(instr.b());
                    let value = match (&self.state.stack[lhs_idx], &self.state.stack[rhs_idx]) {
                        (RuntimeVal::Int(lhs), RuntimeVal::Int(rhs)) => lhs != rhs,
                        _ => self.compare_test_value_slow(instr, lhs_idx, rhs_idx)?,
                    };
                    self.apply_compare_test_branch_unchecked(function, code, instr, value);
                }
                Opcode::TestEqIntI => {
                    let lhs_idx = self.stack_index_unchecked(instr.a());
                    let rhs = i64::from(instr.sc());
                    let value = match &self.state.stack[lhs_idx] {
                        RuntimeVal::Int(lhs) => *lhs == rhs,
                        _ => self.compare_test_immediate_value_slow(instr, lhs_idx)?,
                    };
                    self.apply_compare_test_branch_unchecked(function, code, instr, value);
                }
                Opcode::TestNeIntI => {
                    let lhs_idx = self.stack_index_unchecked(instr.a());
                    let rhs = i64::from(instr.sc());
                    let value = match &self.state.stack[lhs_idx] {
                        RuntimeVal::Int(lhs) => *lhs != rhs,
                        _ => self.compare_test_immediate_value_slow(instr, lhs_idx)?,
                    };
                    self.apply_compare_test_branch_unchecked(function, code, instr, value);
                }
                Opcode::TestLtInt => {
                    let lhs_idx = self.stack_index_unchecked(instr.a());
                    let rhs_idx = self.stack_index_unchecked(instr.b());
                    let value = match (&self.state.stack[lhs_idx], &self.state.stack[rhs_idx]) {
                        (RuntimeVal::Int(lhs), RuntimeVal::Int(rhs)) => lhs < rhs,
                        _ => self.compare_test_value_slow(instr, lhs_idx, rhs_idx)?,
                    };
                    self.apply_compare_test_branch_unchecked(function, code, instr, value);
                }
                Opcode::TestLeInt => {
                    let lhs_idx = self.stack_index_unchecked(instr.a());
                    let rhs_idx = self.stack_index_unchecked(instr.b());
                    let value = match (&self.state.stack[lhs_idx], &self.state.stack[rhs_idx]) {
                        (RuntimeVal::Int(lhs), RuntimeVal::Int(rhs)) => lhs <= rhs,
                        _ => self.compare_test_value_slow(instr, lhs_idx, rhs_idx)?,
                    };
                    self.apply_compare_test_branch_unchecked(function, code, instr, value);
                }
                Opcode::TestGtInt => {
                    let lhs_idx = self.stack_index_unchecked(instr.a());
                    let rhs_idx = self.stack_index_unchecked(instr.b());
                    let value = match (&self.state.stack[lhs_idx], &self.state.stack[rhs_idx]) {
                        (RuntimeVal::Int(lhs), RuntimeVal::Int(rhs)) => lhs > rhs,
                        _ => self.compare_test_value_slow(instr, lhs_idx, rhs_idx)?,
                    };
                    self.apply_compare_test_branch_unchecked(function, code, instr, value);
                }
                Opcode::TestGeInt => {
                    let lhs_idx = self.stack_index_unchecked(instr.a());
                    let rhs_idx = self.stack_index_unchecked(instr.b());
                    let value = match (&self.state.stack[lhs_idx], &self.state.stack[rhs_idx]) {
                        (RuntimeVal::Int(lhs), RuntimeVal::Int(rhs)) => lhs >= rhs,
                        _ => self.compare_test_value_slow(instr, lhs_idx, rhs_idx)?,
                    };
                    self.apply_compare_test_branch_unchecked(function, code, instr, value);
                }
                Opcode::TestEqIntI2 => {
                    let lhs_idx = self.stack_index_unchecked(instr.a());
                    let rhs_idx = self.stack_index_unchecked(instr.b());
                    let packed = instr.c();
                    let lhs_rhs = i64::from(packed >> 4);
                    let rhs_rhs = i64::from(packed & 0x0f);
                    let value = match (&self.state.stack[lhs_idx], &self.state.stack[rhs_idx]) {
                        (RuntimeVal::Int(lhs), RuntimeVal::Int(rhs)) => *lhs == lhs_rhs && *rhs == rhs_rhs,
                        _ => false,
                    };
                    self.apply_compare_test_false_branch_unchecked(function, code, value);
                }
                opcode if opcode.is_int_immediate_compare_test() => {
                    let lhs_idx = self.stack_index_unchecked(instr.a());
                    let rhs = i64::from(instr.sc());
                    let value = match &self.state.stack[lhs_idx] {
                        RuntimeVal::Int(lhs) => match opcode {
                            Opcode::TestLtIntI => *lhs < rhs,
                            Opcode::TestLeIntI => *lhs <= rhs,
                            Opcode::TestGtIntI => *lhs > rhs,
                            Opcode::TestGeIntI => *lhs >= rhs,
                            _ => unreachable!("immediate compare-test matched above"),
                        },
                        _ => self.compare_test_immediate_value_slow(instr, lhs_idx)?,
                    };
                    self.apply_compare_test_branch_unchecked(function, code, instr, value);
                }
                opcode if opcode.is_compare_test() => {
                    let lhs_idx = self.stack_index_unchecked(instr.a());
                    let rhs_idx = self.stack_index_unchecked(instr.b());
                    let int_result = match (&self.state.stack[lhs_idx], &self.state.stack[rhs_idx]) {
                        (RuntimeVal::Int(lhs), RuntimeVal::Int(rhs)) => Some(match instr.opcode() {
                            Opcode::TestEqInt => lhs == rhs,
                            Opcode::TestNeInt => lhs != rhs,
                            Opcode::TestLtInt => lhs < rhs,
                            Opcode::TestLeInt => lhs <= rhs,
                            Opcode::TestGtInt => lhs > rhs,
                            Opcode::TestGeInt => lhs >= rhs,
                            _ => unreachable!(),
                        }),
                        _ => None,
                    };
                    let value = match int_result {
                        Some(v) => v,
                        None => self.compare_test_value_slow(instr, lhs_idx, rhs_idx)?,
                    };
                    self.apply_compare_test_branch_unchecked(function, code, instr, value);
                }
                Opcode::Jmp => {
                    self.pc = self.relative_pc_unchecked(instr.sj_arg());
                }
                Opcode::ForLoopI => {
                    let Some(fact) = function.performance.for_loop(self.pc).copied() else {
                        bail!("ForLoopI missing performance fact at pc {}", self.pc);
                    };
                    let index_slot = self.stack_index_unchecked(instr.a());
                    let end_slot = self.stack_index_unchecked(instr.b());
                    let step_slot = self.stack_index_unchecked(instr.c());
                    let int_operands = match (
                        &self.state.stack[index_slot],
                        &self.state.stack[end_slot],
                        &self.state.stack[step_slot],
                    ) {
                        (RuntimeVal::Int(index), RuntimeVal::Int(end), RuntimeVal::Int(step)) => {
                            Some((*index, *end, *step))
                        }
                        _ => None,
                    };
                    if let Some((index, end, step)) = int_operands {
                        let next = index.wrapping_add(step);
                        self.state.stack[index_slot] = RuntimeVal::Int(next);
                        let keep_going = match (fact.positive_step, fact.inclusive) {
                            (true, true) => next <= end,
                            (true, false) => next < end,
                            (false, true) => next >= end,
                            (false, false) => next > end,
                        };
                        if keep_going {
                            self.pc = self.relative_pc_unchecked(fact.jump_offset);
                        } else {
                            self.pc += 1;
                        }
                    } else {
                        self.dispatch_for_loop_i(instr, fact)?;
                    }
                }
                Opcode::NewList => {
                    self.collect_pending_garbage();
                    if collect_metrics {
                        record_container_op_known_enabled(VmContainerMetric::List);
                    }
                    let build_fact = function.performance.container_build(self.pc).copied();
                    let list = if build_fact.is_some_and(|fact| fact.move_values) {
                        HeapValue::List(self.take_register_list(instr.b(), instr.c())?)
                    } else {
                        HeapValue::List(self.read_register_list(instr.b(), instr.c())?)
                    };
                    let handle = self.alloc_heap_value(list);
                    self.write(instr.a(), RuntimeVal::Obj(handle))?;
                    profile.record_write_source(VmRegisterWriteSource::Container, collect_metrics);
                    self.pc += 1;
                }
                Opcode::NewMap => {
                    self.collect_pending_garbage();
                    if collect_metrics {
                        record_container_op_known_enabled(VmContainerMetric::Map);
                    }
                    let build_fact = function.performance.container_build(self.pc).copied();
                    let map = if let Some(fact) = build_fact {
                        self.take_map_entries(instr.b(), instr.c(), fact.move_keys, fact.move_values)?
                    } else {
                        self.read_map_entries(instr.b(), instr.c())?
                    };
                    let handle = self.alloc_heap_value(HeapValue::Map(typed_map_from_entries(map)));
                    self.write(instr.a(), RuntimeVal::Obj(handle))?;
                    profile.record_write_source(VmRegisterWriteSource::Container, collect_metrics);
                    self.pc += 1;
                }
                Opcode::NewObject => {
                    self.dispatch_cold(Opcode::NewObject, function, module, instr, ctx, collect_metrics)?;
                }
                Opcode::NewRange => {
                    self.dispatch_cold(Opcode::NewRange, function, module, instr, ctx, collect_metrics)?;
                }
                Opcode::Len => {
                    self.dispatch_len(instr, collect_metrics)?;
                    profile.record_write_source(VmRegisterWriteSource::Container, collect_metrics);
                }
                Opcode::ToIter => {
                    self.dispatch_to_iter(instr, collect_metrics)?;
                    profile.record_write_source(VmRegisterWriteSource::Container, collect_metrics);
                }
                Opcode::GetIndex => {
                    let index_fact = self.static_index_fact(function);
                    let known_string_key = if index_fact.is_some_and(|fact| {
                        matches!(
                            fact.target_kind,
                            PerfIndexTargetKind::List | PerfIndexTargetKind::String
                        )
                    }) {
                        None
                    } else {
                        function
                            .performance
                            .known_key(self.pc)
                            .and_then(|fact| fact.const_key)
                            .and_then(|index| function.consts.string(index))
                    };
                    if collect_metrics {
                        record_container_op_known_enabled(index_metric_kind(index_fact));
                    }
                    if let Some(fact) = index_fact
                        && fact.target_kind == PerfIndexTargetKind::List
                    {
                        let value = if fact.value_kind == crate::vm::analysis::PerfValueKind::Int {
                            self.try_get_known_int_list_index(instr.b(), instr.c())
                                .or_else(|| self.try_get_known_list_index(instr.b(), instr.c()))
                        } else {
                            self.try_get_known_list_index(instr.b(), instr.c())
                        };
                        if let Some(value) = value {
                            self.write_unchecked(instr.a(), value);
                            profile.record_write_source(VmRegisterWriteSource::Index, collect_metrics);
                            self.pc += 1;
                            continue;
                        }
                    }
                    let value = self.get_index(
                        self.pc,
                        instr.b(),
                        instr.c(),
                        known_string_key,
                        index_fact,
                        profile.index_key_metrics(collect_metrics),
                    )?;
                    self.write_unchecked(instr.a(), value);
                    profile.record_write_source(VmRegisterWriteSource::Index, collect_metrics);
                    self.pc += 1;
                }
                Opcode::GetIndexStrI => {
                    let index_fact = self.static_index_fact(function);
                    let Some(key_fact) = function.performance.known_key(self.pc).and_then(|fact| fact.string_int)
                    else {
                        bail!("GetIndexStrI missing string-int key fact at pc {}", self.pc);
                    };
                    let Some(prefix) = function.consts.string(key_fact.prefix_key) else {
                        bail!("GetIndexStrI prefix const {} out of bounds", key_fact.prefix_key);
                    };
                    if collect_metrics {
                        record_container_op_known_enabled(index_metric_kind(index_fact));
                    }
                    let value = self.get_string_int_map_index(
                        instr.b(),
                        instr.c(),
                        prefix,
                        profile.index_key_metrics(collect_metrics),
                    )?;
                    self.write_unchecked(instr.a(), value);
                    profile.record_write_source(VmRegisterWriteSource::Index, collect_metrics);
                    self.pc += 1;
                }
                Opcode::GetFieldK => {
                    let index_fact = self.static_index_fact(function);
                    if collect_metrics {
                        record_container_op_known_enabled(index_metric_kind(index_fact));
                    }
                    let Some(key) = function.consts.string(instr.c() as u16) else {
                        bail!("GetFieldK const string index {} out of bounds", instr.c());
                    };
                    let value = self.get_index(
                        self.pc,
                        instr.b(),
                        instr.b(),
                        Some(key),
                        index_fact,
                        profile.index_key_metrics(collect_metrics),
                    )?;
                    self.write_unchecked(instr.a(), value);
                    profile.record_write_source(VmRegisterWriteSource::Index, collect_metrics);
                    let written = *self.read_unchecked(instr.a());
                    if !self.try_apply_next_nil_branch_for_written_value(code, instr.a(), &written) {
                        self.pc += 1;
                    }
                }
                Opcode::GetList => {
                    if collect_metrics {
                        record_container_op_known_enabled(VmContainerMetric::List);
                    }
                    let value = if let Some(value) = self.try_get_known_int_list_index(instr.b(), instr.c()) {
                        value
                    } else if let Some(value) = self.try_get_known_list_index(instr.b(), instr.c()) {
                        value
                    } else {
                        self.get_list_index(instr.b(), instr.c())?
                    };
                    self.write_unchecked(instr.a(), value);
                    profile.record_write_source(VmRegisterWriteSource::Index, collect_metrics);
                    self.pc += 1;
                }
                Opcode::SetIndex => {
                    let move_value = function
                        .performance
                        .container_move(self.pc)
                        .is_some_and(|fact| fact.move_value);
                    let move_key = function
                        .performance
                        .container_move(self.pc)
                        .is_some_and(|fact| fact.move_key);
                    let index_fact = self.static_index_fact(function);
                    let known_string_key = if index_fact.is_some_and(|fact| {
                        matches!(
                            fact.target_kind,
                            PerfIndexTargetKind::List | PerfIndexTargetKind::String
                        )
                    }) {
                        None
                    } else {
                        function
                            .performance
                            .known_key(self.pc)
                            .and_then(|fact| fact.const_key)
                            .and_then(|index| function.consts.string(index))
                    };
                    if collect_metrics {
                        record_container_op_known_enabled(index_metric_kind(index_fact));
                    }
                    self.set_index(
                        self.pc,
                        instr.a(),
                        instr.b(),
                        instr.c(),
                        move_key,
                        move_value,
                        known_string_key,
                        index_fact,
                        profile.index_key_metrics(collect_metrics),
                    )?;
                    self.pc += 1;
                }
                Opcode::SetIndexStrI => {
                    let move_value = function
                        .performance
                        .container_move(self.pc)
                        .is_some_and(|fact| fact.move_value);
                    let index_fact = self.static_index_fact(function);
                    let Some(key_fact) = function.performance.known_key(self.pc).and_then(|fact| fact.string_int)
                    else {
                        bail!("SetIndexStrI missing string-int key fact at pc {}", self.pc);
                    };
                    let Some(prefix) = function.consts.string(key_fact.prefix_key) else {
                        bail!("SetIndexStrI prefix const {} out of bounds", key_fact.prefix_key);
                    };
                    if collect_metrics {
                        record_container_op_known_enabled(index_metric_kind(index_fact));
                    }
                    self.set_string_int_map_index(
                        instr.a(),
                        instr.b(),
                        instr.c(),
                        prefix,
                        move_value,
                        index_fact.map(|fact| fact.value_kind),
                        profile.index_key_metrics(collect_metrics),
                    )?;
                    self.pc += 1;
                }
                Opcode::SetFieldK => {
                    let move_value = function
                        .performance
                        .container_move(self.pc)
                        .is_some_and(|fact| fact.move_value);
                    let index_fact = self.static_index_fact(function);
                    let Some(key) = function.consts.string(instr.c() as u16) else {
                        bail!("SetFieldK const string index {} out of bounds", instr.c());
                    };
                    if collect_metrics {
                        record_container_op_known_enabled(index_metric_kind(index_fact));
                    }
                    self.set_index(
                        self.pc,
                        instr.a(),
                        instr.a(),
                        instr.b(),
                        false,
                        move_value,
                        Some(key),
                        index_fact,
                        profile.index_key_metrics(collect_metrics),
                    )?;
                    self.pc += 1;
                }
                Opcode::ListPush => {
                    if collect_metrics {
                        record_container_op_known_enabled(VmContainerMetric::List);
                    }
                    let move_value = function
                        .performance
                        .container_move(self.pc)
                        .is_some_and(|fact| fact.move_value);
                    self.push_list(instr.a(), instr.b(), move_value)?;
                    self.pc += 1;
                }
                Opcode::Call => {
                    #[cfg(any(test, feature = "vm-profile"))]
                    let old_pc = self.pc;
                    if let Some(idx) = self.dispatch_call(function, module, instr, ctx, collect_metrics)? {
                        profile.flush(collect_metrics);
                        return Ok(FrameOutcome::Switch(idx));
                    }
                    #[cfg(any(test, feature = "vm-profile"))]
                    if collect_metrics && self.pc == old_pc + 1 {
                        profile.record_write_source(VmRegisterWriteSource::CallReturn, collect_metrics);
                    }
                }
                Opcode::CallDirect => {
                    self.collect_pending_garbage();
                    if collect_metrics {
                        record_call_op_known_enabled(VmCallMetric::Exact);
                    }
                    let call_fact = self.call_fact_from_static_cache_or_instr(function, instr, false);
                    let window =
                        CallWindow::new(RegisterIndex::new(call_fact.call_base), call_fact.positional_count, 1);
                    let function_index = instr.b() as u32;
                    self.call_direct_function(module, function_index, window)?;
                    profile.flush(collect_metrics);
                    return Ok(FrameOutcome::Switch(function_index));
                }
                Opcode::CallNamed => {
                    if let Some(idx) = self.dispatch_call_named(function, module, instr, ctx, collect_metrics)? {
                        profile.flush(collect_metrics);
                        return Ok(FrameOutcome::Switch(idx));
                    }
                }
                Opcode::CallMethodK => {
                    self.dispatch_call_method_k(function, module, instr, ctx)?;
                    profile.record_write_source(VmRegisterWriteSource::CallReturn, collect_metrics);
                }
                Opcode::GetGlobal => {
                    let slot = self.global_slot_from_fact_cache_or_instr(function, instr);
                    let value = self.read_global(slot)?;
                    self.write(instr.a(), value)?;
                    profile.record_write_source(VmRegisterWriteSource::Global, collect_metrics);
                    self.pc += 1;
                }
                Opcode::SetGlobal => {
                    self.dispatch_cold(Opcode::SetGlobal, function, module, instr, ctx, collect_metrics)?
                }
                Opcode::Return => {
                    self.collect_pending_garbage();
                    profile.flush(collect_metrics);
                    let values = self.take_return_values(instr.a(), instr.b())?;
                    return self.finish_return(values, base_frame_depth);
                }
                Opcode::Return0 => {
                    self.collect_pending_garbage();
                    profile.flush(collect_metrics);
                    return self.finish_return(ReturnValues::None, base_frame_depth);
                }
                Opcode::Return1 => {
                    self.collect_pending_garbage();
                    profile.flush(collect_metrics);
                    let index = self.stack_index_unchecked(instr.a());
                    let value = core::mem::take(&mut self.state.stack[index]);
                    return self.finish_return(ReturnValues::One(value), base_frame_depth);
                }
                other => bail!("Opcode {:?} is not implemented in Executor yet", other),
            }
        }

        profile.flush(collect_metrics);
        self.finish_return(ReturnValues::None, base_frame_depth)
    }

    /// A `Return*` opcode (or falling off the end of a function's code)
    /// completed the *currently dispatching* activation. If there's a caller
    /// frame within this flat run, pop it, restore the caller's context, and
    /// deliver the value into the call's result register (mirrors what the
    /// old recursive `call_closure_stack_args` did after its nested
    /// `run_function_inner` call returned `Ok`). Otherwise this flat run is
    /// genuinely done.
    fn finish_return(&mut self, values: ReturnValues, base_frame_depth: usize) -> Result<FrameOutcome> {
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
    fn unwind_flat_run(
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

    #[inline]
    #[cfg(test)]
    pub(crate) fn seed_param_arg(&mut self, index: usize, value: RuntimeVal) -> Result<()> {
        let register = u8::try_from(index).map_err(|_| anyhow!("function arg index {} exceeds u8", index))?;
        self.write(register, value)
    }
}

fn gc_stress_enabled() -> bool {
    #[cfg(feature = "std")]
    {
        std::env::var_os("LK_GC_STRESS").is_some_and(|value| value != "0")
    }
    #[cfg(not(feature = "std"))]
    {
        false
    }
}

pub fn execute(function: &Function) -> Result<ExecResult> {
    Executor::new(function.register_count).run_function(function)
}

pub fn execute_module(module: &Module) -> Result<ExecResult> {
    let register_count = module
        .entry_function()
        .ok_or_else(|| anyhow!("module entry function {} out of bounds", module.entry))?
        .register_count;
    Executor::new(register_count).run_module(module)
}

pub fn execute_module_with_globals(module: &Module, globals: Vec<RuntimeVal>) -> Result<ExecResult> {
    let register_count = module
        .entry_function()
        .ok_or_else(|| anyhow!("module entry function {} out of bounds", module.entry))?
        .register_count;
    Executor::new(register_count).run_module_with_globals(module, globals)
}

pub fn execute_module_with_globals_and_ctx(
    module: &Module,
    globals: Vec<RuntimeVal>,
    ctx: &mut VmContext,
) -> Result<ExecResult> {
    execute_module_with_globals_heap_and_ctx(module, globals, HeapStore::new(), ctx)
}

pub fn execute_module_with_globals_heap_and_ctx(
    module: &Module,
    globals: Vec<RuntimeVal>,
    heap: HeapStore,
    ctx: &mut VmContext,
) -> Result<ExecResult> {
    let register_count = module
        .entry_function()
        .ok_or_else(|| anyhow!("module entry function {} out of bounds", module.entry))?
        .register_count;
    Executor::new(register_count).run_module_with_globals_and_ctx(module, globals, heap, ctx)
}

#[cfg(test)]
mod exec_tests;
