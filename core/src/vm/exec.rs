//! Minimal safe executor for the new `Instr` VM path.

mod arithmetic;
mod call;
mod callable_ops;
mod cell;
mod const_load;
mod container;
mod dispatch;
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
    compile_program_module_with_ctx, execute_compiled_module_with_ctx, execute_module_artifact_with_ctx,
    execute_program, execute_program_with_ctx, execute_source,
};
#[cfg(test)]
pub(crate) use runtime_callable::call_runtime_callable_test;
pub use runtime_callable::{
    call_runtime_callable_runtime, call_runtime_value_runtime, call_runtime_value_runtime_list_args,
    call_runtime_value_runtime_named_map, call_runtime_value_runtime_named_map_list_args,
    call_runtime_value_runtime_with_receiver, call_runtime_value_runtime_with_receiver_list_args, copy_runtime_value,
    runtime_value_to_callable_shared,
};

use crate::util::fast_map::{FastHashMap, fast_hash_map_new};
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::val::{HeapStore, HeapValue, RuntimeMapKey, RuntimeVal, TypedList, TypedMap, typed_map_from_entries};

use super::{
    CallWindow, Function, Instr, Module, NativeEntry, Opcode, RegisterIndex, RuntimeExport, RuntimeModuleState,
    VmContext,
    analysis::{
        PerfIndexTargetKind, VmCallMetric, VmContainerMetric, VmRegisterWriteSource, record_call_op_known_enabled,
        record_container_op_known_enabled, record_copy_policy_clone, vm_runtime_metrics_enabled,
    },
};
#[cfg(test)]
use super::{Compiler, GlobalSlot};
use handler::{ErrorHandler, LanguageRaise};
use profile::{RuntimeProfileFrame, index_metric_kind, move_clone_metric};
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
            entries.insert(RuntimeMapKey::String(slot.name.clone()), value.clone());
        }
        let value = RuntimeVal::Obj(state.heap.alloc(HeapValue::Map(typed_map_from_entries(entries))));
        RuntimeExport::new(
            value,
            Arc::new(std::sync::Mutex::new(RuntimeModuleState::new(
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

fn append_separator(out: &mut String, first: &mut bool) {
    if !*first {
        out.push_str(", ");
    }
    *first = false;
}

fn append_display_items<T: std::fmt::Display>(out: &mut String, values: impl IntoIterator<Item = T>) {
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

fn append_string_display_map_entries<T: std::fmt::Display>(out: &mut String, entries: &FastHashMap<Arc<str>, T>) {
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
    collect_metrics: bool,
    gc_pending: bool,
    shared_module: Option<Arc<Module>>,
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
            collect_metrics: false,
            gc_pending: false,
            shared_module: None,
        };
        this.reset_entry_frame(register_count);
        this
    }

    pub fn run_function(self, function: &Function) -> Result<ExecResult> {
        let mut ctx = None;
        let mut this = self;
        this.reset_entry_frame(function.register_count);
        let returns = this.run_function_inner(function, None, &mut ctx)?.into_vec();
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
        let returns = this.run_function_inner(entry, Some(module), &mut ctx)?.into_vec();
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
        let returns = self.run_function_inner(entry, Some(module), &mut ctx)?.into_vec();
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
        let returns = self.run_function_inner(entry, Some(module), &mut ctx)?.into_vec();
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

    pub(crate) fn run_module_function_with_state_recoverable<F>(
        mut self,
        module: &Module,
        shared_module: Option<Arc<Module>>,
        function_index: u32,
        captures: Arc<Vec<RuntimeVal>>,
        state: RuntimeModuleState,
        ctx: &mut VmContext,
        seed_args: F,
    ) -> std::result::Result<ExecResult, ExecFailure>
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
        match self.run_function_inner(function, Some(module), &mut ctx) {
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

        let collect_metrics = vm_runtime_metrics_enabled();
        self.collect_metrics = collect_metrics;
        let code = &function.code;
        let mut profile = RuntimeProfileFrame::new();
        while self.pc < code.len() {
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
                    let value = match self.read_unchecked(instr.b()) {
                        RuntimeVal::Obj(_) => {
                            let move_fact = function.performance.register_copy(self.pc);
                            if move_fact.is_some_and(|fact| fact.move_source) {
                                self.take_unchecked(instr.b())
                            } else {
                                let value = self.read_unchecked(instr.b()).clone();
                                if collect_metrics {
                                    record_copy_policy_clone(
                                        move_clone_metric(function, self.pc, instr.a() as u16, instr.b() as u16),
                                        true,
                                    );
                                }
                                value
                            }
                        }
                        value => value.clone(),
                    };
                    self.write_unchecked(instr.a(), value);
                    profile.record_write_source(VmRegisterWriteSource::Move, collect_metrics);
                    self.pc += 1;
                    if !matches!(code.get(self.pc).copied().map(Instr::opcode), Some(Opcode::Move)) {
                        break;
                    }
                    profile.record_opcode(Opcode::Move, collect_metrics);
                },
                Opcode::LoadCapture => {
                    self.dispatch_load_capture(instr)?;
                    profile.record_write_source(VmRegisterWriteSource::Other, collect_metrics);
                }
                Opcode::LoadCellVal => {
                    self.dispatch_load_cell_val(instr)?;
                    profile.record_write_source(VmRegisterWriteSource::Other, collect_metrics);
                }
                Opcode::StoreCellVal => self.dispatch_store_cell_val(function, instr)?,
                Opcode::LoadFunction => {
                    self.dispatch_load_function(instr, module)?;
                    profile.record_write_source(VmRegisterWriteSource::Other, collect_metrics);
                }
                Opcode::MakeClosure => {
                    self.dispatch_make_closure(instr, module)?;
                    profile.record_write_source(VmRegisterWriteSource::Other, collect_metrics);
                }
                Opcode::LoadNative => {
                    self.dispatch_load_native(instr, module)?;
                    profile.record_write_source(VmRegisterWriteSource::Other, collect_metrics);
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
                    let dst = instr.a() as usize;
                    let lhs_idx = instr.b() as usize;
                    match &self.state.stack[lhs_idx] {
                        RuntimeVal::Int(lhs) => {
                            self.state.stack[dst] = RuntimeVal::Int(lhs.wrapping_add(instr.sc() as i64));
                            profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                            self.pc += 1;
                        }
                        lhs => bail!("AddIntI expected Int lhs, got {:?}", lhs.kind()),
                    }
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
                            self.state.stack[dst] = RuntimeVal::Int(*l % *r);
                            profile.record_write_source(VmRegisterWriteSource::Arithmetic, collect_metrics);
                            self.pc += 1;
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
                    #[cfg(any(test, feature = "vm-profile"))]
                    let old_pc = self.pc;
                    self.dispatch_not(function, instr)?;
                    #[cfg(any(test, feature = "vm-profile"))]
                    if collect_metrics && self.pc == old_pc + 1 {
                        profile.record_write_source(VmRegisterWriteSource::Compare, collect_metrics);
                    }
                }
                Opcode::IsNil => {
                    #[cfg(any(test, feature = "vm-profile"))]
                    let old_pc = self.pc;
                    self.dispatch_is_nil(function, instr)?;
                    #[cfg(any(test, feature = "vm-profile"))]
                    if collect_metrics && self.pc == old_pc + 1 {
                        profile.record_write_source(VmRegisterWriteSource::Compare, collect_metrics);
                    }
                }
                Opcode::IsList => {
                    #[cfg(any(test, feature = "vm-profile"))]
                    let old_pc = self.pc;
                    self.dispatch_is_list(function, instr)?;
                    #[cfg(any(test, feature = "vm-profile"))]
                    if collect_metrics && self.pc == old_pc + 1 {
                        profile.record_write_source(VmRegisterWriteSource::Compare, collect_metrics);
                    }
                }
                Opcode::IsMap => {
                    #[cfg(any(test, feature = "vm-profile"))]
                    let old_pc = self.pc;
                    self.dispatch_is_map(function, instr)?;
                    #[cfg(any(test, feature = "vm-profile"))]
                    if collect_metrics && self.pc == old_pc + 1 {
                        profile.record_write_source(VmRegisterWriteSource::Compare, collect_metrics);
                    }
                }
                Opcode::ToString => {
                    self.dispatch_to_string(instr, module, ctx)?;
                    profile.record_write_source(VmRegisterWriteSource::String, collect_metrics);
                }
                Opcode::ConcatString => {
                    self.dispatch_concat_string(instr, module, ctx)?;
                    profile.record_write_source(VmRegisterWriteSource::String, collect_metrics);
                }
                Opcode::StringStartsWith => {
                    self.dispatch_string_starts_with(instr)?;
                    profile.record_write_source(VmRegisterWriteSource::String, collect_metrics);
                }
                Opcode::StringSplit => {
                    self.dispatch_string_split(instr)?;
                    profile.record_write_source(VmRegisterWriteSource::String, collect_metrics);
                }
                Opcode::ListJoin => {
                    self.dispatch_list_join(instr)?;
                    profile.record_write_source(VmRegisterWriteSource::String, collect_metrics);
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
                    #[cfg(any(test, feature = "vm-profile"))]
                    let old_pc = self.pc;
                    self.dispatch_contains(function, instr)?;
                    #[cfg(any(test, feature = "vm-profile"))]
                    if collect_metrics && self.pc == old_pc + 1 {
                        profile.record_write_source(VmRegisterWriteSource::Compare, collect_metrics);
                    }
                }
                Opcode::SliceFrom => {
                    self.dispatch_slice_from(instr)?;
                    profile.record_write_source(VmRegisterWriteSource::Container, collect_metrics);
                }
                Opcode::MapRest => {
                    self.dispatch_map_rest(instr)?;
                    profile.record_write_source(VmRegisterWriteSource::Container, collect_metrics);
                }
                Opcode::Raise => self.dispatch_raise(function, instr)?,
                Opcode::TryBegin => self.dispatch_try_begin(instr)?,
                Opcode::TryEnd => self.dispatch_try_end(),
                Opcode::Test => self.dispatch_test(instr)?,
                Opcode::BrFalse => self.dispatch_br_false(instr)?,
                Opcode::BrTrue => self.dispatch_br_true(instr)?,
                Opcode::BrNil => self.dispatch_br_nil(instr)?,
                Opcode::BrNotNil => self.dispatch_br_not_nil(instr)?,
                opcode if opcode.is_compare_test() => self.dispatch_compare_test(function, instr)?,
                Opcode::Jmp => self.dispatch_jmp(instr)?,
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
                            self.pc = self.relative_pc(fact.jump_offset)?;
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
                    self.dispatch_new_object(instr, collect_metrics)?;
                    profile.record_write_source(VmRegisterWriteSource::Container, collect_metrics);
                }
                Opcode::NewRange => {
                    self.dispatch_new_range(instr, collect_metrics)?;
                    profile.record_write_source(VmRegisterWriteSource::Container, collect_metrics);
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
                    if index_fact.is_some_and(|fact| fact.target_kind == PerfIndexTargetKind::List)
                        && let Some(value) = self.try_get_known_list_index(instr.b(), instr.c())
                    {
                        self.write_unchecked(instr.a(), value);
                        profile.record_write_source(VmRegisterWriteSource::Index, collect_metrics);
                        self.pc += 1;
                        continue;
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
                    self.dispatch_call(function, module, instr, ctx, collect_metrics)?;
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
                    let call_pc = self.pc;
                    let value = self.call_direct_function(module, instr.b() as u32, window, ctx)?;
                    if self.pc != call_pc {
                        continue;
                    }
                    self.clear_call_window_temps(window, 0)?;
                    self.write_returns(window, [value])?;
                    profile.record_write_source(VmRegisterWriteSource::CallReturn, collect_metrics);
                    self.pc += 1;
                }
                Opcode::CallNamed => {
                    #[cfg(any(test, feature = "vm-profile"))]
                    let old_pc = self.pc;
                    self.dispatch_call_named(function, module, instr, ctx, collect_metrics)?;
                    #[cfg(any(test, feature = "vm-profile"))]
                    if collect_metrics && self.pc == old_pc + 1 {
                        profile.record_write_source(VmRegisterWriteSource::CallReturn, collect_metrics);
                    }
                }
                Opcode::GetGlobal => {
                    let slot = self.global_slot_from_fact_cache_or_instr(function, instr);
                    let value = self.read_global(slot)?;
                    self.write(instr.a(), value)?;
                    profile.record_write_source(VmRegisterWriteSource::Global, collect_metrics);
                    self.pc += 1;
                }
                Opcode::SetGlobal => self.dispatch_set_global(function, instr)?,
                Opcode::Return => {
                    self.collect_pending_garbage();
                    profile.flush(collect_metrics);
                    return self.take_return_values(instr.a(), instr.b());
                }
                other => bail!("Opcode {:?} is not implemented in Executor yet", other),
            }
        }

        profile.flush(collect_metrics);
        Ok(ReturnValues::None)
    }

    #[inline]
    #[cfg(test)]
    pub(crate) fn seed_param_arg(&mut self, index: usize, value: RuntimeVal) -> Result<()> {
        let register = u8::try_from(index).map_err(|_| anyhow!("function arg index {} exceeds u8", index))?;
        self.write(register, value)
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
