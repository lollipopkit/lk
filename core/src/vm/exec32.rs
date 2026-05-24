//! Minimal safe executor for the new `Instr32` VM path.

mod arithmetic;
mod call;
mod callable_ops;
mod cell;
mod const_load;
mod container;
mod gc;
mod globals;
mod handler;
mod imports;
mod named_call;
mod program;
mod return_values;
mod runtime_callable;
mod stack;
mod support;
mod value_ops;

pub use super::RuntimeCallable32;
pub use imports::import_runtime_export;
pub use program::{
    compile_program32_module_with_ctx, execute_compiled_module32_with_ctx, execute_module32_artifact_with_ctx,
    execute_program32, execute_program32_with_ctx, execute_source32,
};
#[cfg(test)]
pub(crate) use runtime_callable::call_runtime_callable32_test;
pub use runtime_callable::{
    call_runtime_callable32_runtime, call_runtime_value32_runtime, call_runtime_value32_runtime_list_args,
    call_runtime_value32_runtime_named_map, call_runtime_value32_runtime_named_map_list_args,
    call_runtime_value32_runtime_with_receiver, call_runtime_value32_runtime_with_receiver_list_args,
    copy_runtime_value, runtime_value_to_callable32_shared,
};

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow, bail};

use crate::val::{HeapStore, HeapValue, RuntimeMapKey, RuntimeVal, TypedList, TypedMap, typed_map_from_entries};

use super::{
    CallWindow32, Function32, Instr32, Module32, NativeEntry32, Opcode32, RegisterIndex, RuntimeExport32,
    RuntimeModuleState32, VmContext,
    analysis::{
        PerfCallFact, PerfIndexFact, VmCallMetric, VmContainerMetric, VmValueCopyMetric,
        record_branch_op_known_enabled, record_call_op_known_enabled, record_container_op_known_enabled,
        record_copy_policy_clone, record_opcode_step_known_enabled, vm_runtime_metrics_enabled,
    },
};
#[cfg(test)]
use super::{Compiler32, GlobalSlot32};
use handler::{ErrorHandler32, LanguageRaise32};
use return_values::ReturnValues32;
use support::*;

#[derive(Debug)]
pub struct Exec32Result {
    pub returns: Vec<RuntimeVal>,
    pub state: RuntimeModuleState32,
}

#[derive(Debug)]
pub struct Program32Result {
    pub returns: Vec<RuntimeVal>,
    pub state: RuntimeModuleState32,
    pub module: Arc<Module32>,
}

pub(crate) struct Exec32Failure {
    pub error: anyhow::Error,
    pub state: RuntimeModuleState32,
}

impl Program32Result {
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

    pub fn into_exports(self) -> RuntimeExport32 {
        let mut state = self.state;
        let mut entries = BTreeMap::new();
        for (slot, value) in self.module.globals.iter().zip(state.globals.iter()) {
            entries.insert(RuntimeMapKey::String(slot.name.clone()), value.clone());
        }
        let value = RuntimeVal::Obj(state.heap.alloc(HeapValue::Map(typed_map_from_entries(entries))));
        RuntimeExport32::new(
            value,
            Arc::new(std::sync::Mutex::new(RuntimeModuleState32::new(
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
        crate::val::CallableValue::RuntimeNative32 { name, arity, .. } => {
            if *arity == NativeEntry32::VARIADIC {
                format!("<native fn {}(...)>", name)
            } else {
                format!("<native fn {}({} args)>", name, arity)
            }
        }
        crate::val::CallableValue::Runtime32(function) => {
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
    entries: &BTreeMap<Arc<str>, RuntimeVal>,
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

fn append_string_display_map_entries<T: std::fmt::Display>(out: &mut String, entries: &BTreeMap<Arc<str>, T>) {
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
pub struct Executor32 {
    state: RuntimeModuleState32,
    captures: Arc<Vec<RuntimeVal>>,
    handler_stack: Vec<ErrorHandler32>,
    frame_base: usize,
    register_count: u16,
    pc: usize,
    shared_module: Option<Arc<Module32>>,
}

impl Executor32 {
    #[inline]
    pub fn new(register_count: u16) -> Self {
        let mut this = Self {
            state: RuntimeModuleState32::default(),
            captures: Arc::new(Vec::new()),
            handler_stack: Vec::new(),
            frame_base: 0,
            register_count,
            pc: 0,
            shared_module: None,
        };
        this.reset_entry_frame(register_count);
        this
    }

    pub fn run_function(self, function: &Function32) -> Result<Exec32Result> {
        let mut ctx = None;
        let mut this = self;
        this.reset_entry_frame(function.register_count);
        let returns = this.run_function_inner(function, None, &mut ctx)?.into_vec();
        Ok(this.finish(returns))
    }

    pub fn run_module(self, module: &Module32) -> Result<Exec32Result> {
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

    pub fn run_module_with_globals(self, module: &Module32, globals: Vec<RuntimeVal>) -> Result<Exec32Result> {
        self.run_module_with_globals_and_heap(module, globals, HeapStore::new())
    }

    pub fn run_module_with_globals_and_heap(
        mut self,
        module: &Module32,
        globals: Vec<RuntimeVal>,
        heap: HeapStore,
    ) -> Result<Exec32Result> {
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
        module: &Module32,
        globals: Vec<RuntimeVal>,
        heap: HeapStore,
        ctx: &mut VmContext,
    ) -> Result<Exec32Result> {
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
        module: Arc<Module32>,
        globals: Vec<RuntimeVal>,
        heap: HeapStore,
        ctx: &mut VmContext,
    ) -> Result<Exec32Result> {
        self.shared_module = Some(Arc::clone(&module));
        self.run_module_with_globals_and_ctx(module.as_ref(), globals, heap, ctx)
    }

    pub(crate) fn run_module_function_with_state_recoverable<F>(
        mut self,
        module: &Module32,
        shared_module: Option<Arc<Module32>>,
        function_index: u32,
        captures: Arc<Vec<RuntimeVal>>,
        state: RuntimeModuleState32,
        ctx: &mut VmContext,
        seed_args: F,
    ) -> std::result::Result<Exec32Result, Exec32Failure>
    where
        F: FnOnce(&mut Self) -> Result<u16>,
    {
        let Some(function) = module.functions.get(function_index as usize) else {
            return Err(Exec32Failure {
                error: anyhow!("function index {} out of bounds", function_index),
                state,
            });
        };
        if state.globals.len() != module.globals.len() {
            return Err(Exec32Failure {
                error: anyhow!(
                    "module expected {} globals, got {}",
                    module.globals.len(),
                    state.globals.len()
                ),
                state,
            });
        }
        self.state = state;
        self.captures = captures;
        self.shared_module = shared_module;
        self.reset_entry_frame(function.register_count);
        let arg_count = match seed_args(&mut self) {
            Ok(arg_count) => arg_count,
            Err(error) => {
                return Err(Exec32Failure {
                    error,
                    state: self.state,
                });
            }
        };
        if function.param_count != arg_count {
            return Err(Exec32Failure {
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
            Ok(returns) => Ok(self.finish(returns.into_vec())),
            Err(error) => Err(Exec32Failure {
                error,
                state: self.state,
            }),
        }
    }

    fn finish(self, returns: Vec<RuntimeVal>) -> Exec32Result {
        Exec32Result {
            returns,
            state: self.state,
        }
    }

    fn run_function_inner(
        &mut self,
        function: &Function32,
        module: Option<&Module32>,
        ctx: &mut Option<&mut VmContext>,
    ) -> Result<ReturnValues32> {
        if self.register_count < function.register_count {
            bail!(
                "executor frame has {} registers, function requires {}",
                self.register_count,
                function.register_count
            );
        }

        let collect_metrics = vm_runtime_metrics_enabled();
        while self.pc < function.code.len() {
            if collect_metrics {
                record_opcode_step_known_enabled();
            }
            self.maybe_collect_garbage();
            let instr = function.code[self.pc];
            if self.try_load_const_instr(function, instr)? {
                continue;
            }
            match instr.opcode() {
                Opcode32::Nop => self.pc += 1,
                Opcode32::Move => {
                    let value = if function
                        .performance
                        .register_copy(self.pc)
                        .is_some_and(|fact| fact.move_source)
                    {
                        self.take(instr.b())?
                    } else {
                        let value = self.read(instr.b())?.clone();
                        record_copy_policy_clone(
                            move_clone_metric(function, self.pc, instr.a() as u16, instr.b() as u16),
                            matches!(value, RuntimeVal::Obj(_)),
                        );
                        value
                    };
                    self.write(instr.a(), value)?;
                    self.pc += 1;
                }
                Opcode32::LoadCapture => {
                    let value = self
                        .captures
                        .get(instr.bx() as usize)
                        .cloned()
                        .ok_or_else(|| anyhow!("LoadCapture index {} out of bounds", instr.bx()))?;
                    self.write(instr.a(), value)?;
                    self.pc += 1;
                }
                Opcode32::LoadCellVal => {
                    let value = self.load_cell_value(instr.b())?;
                    self.write(instr.a(), value)?;
                    self.pc += 1;
                }
                Opcode32::StoreCellVal => {
                    self.store_cell_value(
                        instr.a(),
                        instr.b(),
                        function
                            .performance
                            .cell_move(self.pc)
                            .is_some_and(|fact| fact.move_value),
                    )?;
                    self.pc += 1;
                }
                Opcode32::LoadFunction => {
                    self.load_function_value(instr.a(), instr.bx(), module)?;
                    self.pc += 1;
                }
                Opcode32::MakeClosure => {
                    self.make_closure_value(instr.a(), instr.b(), instr.c(), module)?;
                    self.pc += 1;
                }
                Opcode32::LoadNative => {
                    self.load_native_value(instr.a(), instr.bx(), module)?;
                    self.pc += 1;
                }
                Opcode32::AddInt => self.dynamic_add(instr)?,
                Opcode32::SubInt => self.dynamic_sub(instr)?,
                Opcode32::MulInt => {
                    self.dynamic_numeric_binary(instr, |lhs, rhs| lhs.wrapping_mul(rhs), |lhs, rhs| lhs * rhs)?
                }
                Opcode32::DivInt => {
                    let rhs = self.read_number(instr.c())?;
                    if rhs == 0.0 {
                        bail!("DivInt divisor is zero");
                    }
                    self.dynamic_numeric_binary(instr, |lhs, rhs| lhs / rhs, |lhs, rhs| lhs / rhs)?;
                }
                Opcode32::ModInt => {
                    let rhs = self.read_number(instr.c())?;
                    if rhs == 0.0 {
                        bail!("ModInt divisor is zero");
                    }
                    self.dynamic_numeric_binary(instr, |lhs, rhs| lhs % rhs, |lhs, rhs| lhs % rhs)?;
                }
                Opcode32::AddFloat => self.float_binary(instr, |lhs, rhs| lhs + rhs)?,
                Opcode32::SubFloat => self.float_binary(instr, |lhs, rhs| lhs - rhs)?,
                Opcode32::MulFloat => self.float_binary(instr, |lhs, rhs| lhs * rhs)?,
                Opcode32::DivFloat => {
                    let lhs = self.read_number(instr.b())?;
                    let rhs = self.read_number(instr.c())?;
                    if rhs == 0.0 {
                        bail!("DivFloat divisor is zero");
                    }
                    self.write(instr.a(), RuntimeVal::Float(lhs / rhs))?;
                    self.pc += 1;
                }
                Opcode32::ModFloat => {
                    let lhs = self.read_number(instr.b())?;
                    let rhs = self.read_number(instr.c())?;
                    if rhs == 0.0 {
                        bail!("ModFloat divisor is zero");
                    }
                    self.write(instr.a(), RuntimeVal::Float(lhs % rhs))?;
                    self.pc += 1;
                }
                Opcode32::Not => {
                    let value = match self.read(instr.b())? {
                        RuntimeVal::Bool(value) => RuntimeVal::Bool(!value),
                        RuntimeVal::Nil => RuntimeVal::Bool(true),
                        other => bail!("Not expected Bool or Nil, got {:?}", other.kind()),
                    };
                    self.write(instr.a(), value)?;
                    self.pc += 1;
                }
                Opcode32::IsNil => {
                    let value = matches!(self.read(instr.b())?, RuntimeVal::Nil);
                    self.write(instr.a(), RuntimeVal::Bool(value))?;
                    self.pc += 1;
                }
                Opcode32::IsList => {
                    let value = self.runtime_value_is_list(self.read(instr.b())?)?;
                    self.write(instr.a(), RuntimeVal::Bool(value))?;
                    self.pc += 1;
                }
                Opcode32::IsMap => {
                    let value = self.runtime_value_is_map(self.read(instr.b())?)?;
                    self.write(instr.a(), RuntimeVal::Bool(value))?;
                    self.pc += 1;
                }
                Opcode32::ToString => {
                    let value = self.to_runtime_string(instr.b())?;
                    self.write_string(instr.a(), value)?;
                    self.pc += 1;
                }
                Opcode32::ConcatString => {
                    let lhs = self.to_runtime_string(instr.b())?;
                    let rhs = self.to_runtime_string(instr.c())?;
                    self.write_string(instr.a(), format!("{lhs}{rhs}"))?;
                    self.pc += 1;
                }
                Opcode32::CmpInt => {
                    let equal = self.values_equal(instr.b(), instr.c())?;
                    self.write(instr.a(), RuntimeVal::Bool(equal))?;
                    self.pc += 1;
                }
                Opcode32::CmpNeInt => {
                    let equal = self.values_equal(instr.b(), instr.c())?;
                    self.write(instr.a(), RuntimeVal::Bool(!equal))?;
                    self.pc += 1;
                }
                Opcode32::CmpLtInt => self.int_compare(instr, |lhs, rhs| lhs < rhs)?,
                Opcode32::CmpLeInt => self.int_compare(instr, |lhs, rhs| lhs <= rhs)?,
                Opcode32::CmpGtInt => self.int_compare(instr, |lhs, rhs| lhs > rhs)?,
                Opcode32::CmpGeInt => self.int_compare(instr, |lhs, rhs| lhs >= rhs)?,
                Opcode32::Contains => {
                    let value = self.contains_value(instr.b(), instr.c())?;
                    self.write(instr.a(), RuntimeVal::Bool(value))?;
                    self.pc += 1;
                }
                Opcode32::SliceFrom => {
                    let value = self.slice_from(instr.b(), instr.c())?;
                    self.write(instr.a(), value)?;
                    self.pc += 1;
                }
                Opcode32::MapRest => {
                    let value = self.map_rest(instr.b(), instr.c())?;
                    self.write(instr.a(), value)?;
                    self.pc += 1;
                }
                Opcode32::Raise => {
                    let message = function
                        .consts
                        .strings
                        .get(instr.bx() as usize)
                        .ok_or_else(|| anyhow!("Raise const index {} out of bounds", instr.bx()))?;
                    self.raise_language_message(message)?;
                }
                Opcode32::TryBegin => {
                    self.begin_try(instr.a(), instr.sbx() as i32)?;
                }
                Opcode32::TryEnd => {
                    self.end_try();
                }
                Opcode32::Test => {
                    if collect_metrics {
                        record_branch_op_known_enabled(true);
                    }
                    let truthy = self.truthy(instr.a())?;
                    if truthy == (instr.b() != 0) {
                        self.pc += 1;
                    } else {
                        self.pc = self.relative_pc(instr.c() as i8 as i32)?;
                    }
                }
                Opcode32::Jmp => {
                    if collect_metrics {
                        record_branch_op_known_enabled(false);
                    }
                    self.pc = self.relative_pc(instr.sj_arg())?;
                }
                Opcode32::NewList => {
                    if collect_metrics {
                        record_container_op_known_enabled(VmContainerMetric::List);
                    }
                    let build_fact = function.performance.container_build(self.pc).copied();
                    let list = if build_fact.is_some_and(|fact| fact.move_values) {
                        HeapValue::List(self.take_register_list(instr.b(), instr.c())?)
                    } else {
                        HeapValue::List(self.read_register_list(instr.b(), instr.c())?)
                    };
                    let handle = self.state.heap.alloc(list);
                    self.write(instr.a(), RuntimeVal::Obj(handle))?;
                    self.pc += 1;
                }
                Opcode32::NewMap => {
                    if collect_metrics {
                        record_container_op_known_enabled(VmContainerMetric::Map);
                    }
                    let build_fact = function.performance.container_build(self.pc).copied();
                    let map = if let Some(fact) = build_fact {
                        self.take_map_entries(instr.b(), instr.c(), fact.move_keys, fact.move_values)?
                    } else {
                        self.read_map_entries(instr.b(), instr.c())?
                    };
                    let handle = self.state.heap.alloc(HeapValue::Map(typed_map_from_entries(map)));
                    self.write(instr.a(), RuntimeVal::Obj(handle))?;
                    self.pc += 1;
                }
                Opcode32::NewObject => {
                    if collect_metrics {
                        record_container_op_known_enabled(VmContainerMetric::Generic);
                    }
                    let object = self.read_object_fields(instr.b(), instr.c())?;
                    let handle = self.state.heap.alloc(HeapValue::Object(object));
                    self.write(instr.a(), RuntimeVal::Obj(handle))?;
                    self.pc += 1;
                }
                Opcode32::NewRange => {
                    if collect_metrics {
                        record_container_op_known_enabled(VmContainerMetric::List);
                    }
                    let list = self.build_int_range(instr.b(), instr.c() != 0)?;
                    let handle = self.state.heap.alloc(HeapValue::List(TypedList::Int(list)));
                    self.write(instr.a(), RuntimeVal::Obj(handle))?;
                    self.pc += 1;
                }
                Opcode32::Len => {
                    if collect_metrics {
                        record_container_op_known_enabled(VmContainerMetric::Generic);
                    }
                    let len = self.len_value(instr.b())?;
                    self.write(instr.a(), RuntimeVal::Int(len as i64))?;
                    self.pc += 1;
                }
                Opcode32::ToIter => {
                    if collect_metrics {
                        record_container_op_known_enabled(VmContainerMetric::Generic);
                    }
                    let iter = self.to_iter(instr.b())?;
                    self.write(instr.a(), iter)?;
                    self.pc += 1;
                }
                Opcode32::GetIndex => {
                    if collect_metrics {
                        record_container_op_known_enabled(VmContainerMetric::Generic);
                    }
                    let known_string_key = function
                        .performance
                        .known_key(self.pc)
                        .and_then(|fact| fact.const_key)
                        .and_then(|index| function.consts.string(index))
                        .map(Arc::<str>::from);
                    let index_fact = self.static_index_fact(function);
                    let value = self.get_index(self.pc, instr.b(), instr.c(), known_string_key, index_fact)?;
                    self.write(instr.a(), value)?;
                    self.pc += 1;
                }
                Opcode32::SetIndex => {
                    if collect_metrics {
                        record_container_op_known_enabled(VmContainerMetric::Generic);
                    }
                    let move_value = function
                        .performance
                        .container_move(self.pc)
                        .is_some_and(|fact| fact.move_value);
                    let move_key = function
                        .performance
                        .container_move(self.pc)
                        .is_some_and(|fact| fact.move_key);
                    let known_string_key = function
                        .performance
                        .known_key(self.pc)
                        .and_then(|fact| fact.const_key)
                        .and_then(|index| function.consts.string(index))
                        .map(Arc::<str>::from);
                    let index_fact = self.static_index_fact(function);
                    self.set_index(
                        self.pc,
                        instr.a(),
                        instr.b(),
                        instr.c(),
                        move_key,
                        move_value,
                        known_string_key,
                        index_fact,
                    )?;
                    self.pc += 1;
                }
                Opcode32::Call => {
                    if collect_metrics {
                        record_call_op_known_enabled(VmCallMetric::Generic);
                    }
                    let call_fact = self.call_fact_from_static_cache_or_instr(function, instr, false);
                    let window =
                        CallWindow32::new(RegisterIndex::new(call_fact.call_base), call_fact.positional_count, 1);
                    let call_pc = self.pc;
                    let value = self.call_function(module, window, Some(call_fact.target_kind), ctx)?;
                    if self.pc != call_pc {
                        continue;
                    }
                    self.clear_call_window_temps(window, 0)?;
                    self.write_returns(window, [value])?;
                    self.pc += 1;
                }
                Opcode32::CallNamed => {
                    if collect_metrics {
                        record_call_op_known_enabled(VmCallMetric::Named);
                    }
                    let call_fact = self.call_fact_from_static_cache_or_instr(function, instr, true);
                    let window =
                        CallWindow32::new(RegisterIndex::new(call_fact.call_base), call_fact.positional_count, 1);
                    let call_pc = self.pc;
                    let value = self.call_function_named(
                        module,
                        window,
                        call_fact.named_count,
                        Some(call_fact.target_kind),
                        ctx,
                    )?;
                    if self.pc != call_pc {
                        continue;
                    }
                    self.clear_call_window_temps(window, call_fact.named_count)?;
                    self.write_returns(window, [value])?;
                    self.pc += 1;
                }
                Opcode32::GetGlobal => {
                    let slot = self.global_slot_from_fact_cache_or_instr(function, instr);
                    let value = self.read_global(slot)?;
                    self.write(instr.a(), value)?;
                    self.pc += 1;
                }
                Opcode32::SetGlobal => {
                    let global_fact = function.performance.global_op(self.pc).copied();
                    let value = if global_fact.is_some_and(|fact| fact.move_source) {
                        self.take(instr.a())?
                    } else {
                        self.read(instr.a())?.clone()
                    };
                    let slot = self.global_slot_from_fact_cache_or_instr(function, instr);
                    self.write_global(slot, value)?;
                    self.pc += 1;
                }
                Opcode32::Return => {
                    return self.take_return_values(instr.a(), instr.b());
                }
                other => bail!("Opcode32 {:?} is not implemented in Executor32 yet", other),
            }
        }

        Ok(ReturnValues32::None)
    }

    #[inline]
    #[cfg(test)]
    pub(crate) fn seed_param_arg(&mut self, index: usize, value: RuntimeVal) -> Result<()> {
        let register = u8::try_from(index).map_err(|_| anyhow!("function arg index {} exceeds u8", index))?;
        self.write(register, value)
    }

    #[inline]
    fn read_int(&self, register: u8) -> Result<i64> {
        match self.read(register)? {
            RuntimeVal::Int(value) => Ok(*value),
            other => bail!("register {} expected Int, got {:?}", register, other.kind()),
        }
    }

    #[inline]
    fn read_number(&self, register: u8) -> Result<f64> {
        self.number_value(self.read(register)?)
            .with_context(|| format!("register {} expected Int or Float", register))
    }

    fn number_value(&self, value: &RuntimeVal) -> Result<f64> {
        match value {
            RuntimeVal::Int(value) => Ok(*value as f64),
            RuntimeVal::Float(value) => Ok(*value),
            other => bail!("got {:?}", other.kind()),
        }
    }

    #[inline]
    fn truthy(&self, register: u8) -> Result<bool> {
        Ok(!matches!(
            self.read(register)?,
            RuntimeVal::Nil | RuntimeVal::Bool(false)
        ))
    }

    #[inline]
    pub(super) fn relative_pc(&self, offset: i32) -> Result<usize> {
        let next = self.pc as i64 + 1 + offset as i64;
        if next < 0 {
            bail!("jump before start of function");
        }
        Ok(next as usize)
    }

    #[inline]
    fn call_fact_from_static_cache_or_instr(
        &mut self,
        function: &Function32,
        instr: Instr32,
        named: bool,
    ) -> PerfCallFact {
        if let Some(fact) = function.performance.call_site(self.pc).copied()
            && (named || fact.named_count == 0)
        {
            self.state.inline_caches.set_call(self.pc, fact);
            return fact;
        }
        if let Some(fact) = self.state.inline_caches.call(self.pc) {
            return fact;
        }
        let (positional_count, named_count) = if named {
            let payload = instr.bx();
            ((payload & 0x7f) as u16, (payload >> 7) as u16)
        } else {
            (instr.c() as u16, 0)
        };
        let fact = PerfCallFact {
            // A holds the call-window base. B is only 7 bits and would truncate call_base >= 128.
            call_base: instr.a() as u16,
            positional_count,
            named_count,
            target_kind: self.observe_call_target_kind(instr.a() as u16),
        };
        self.state.inline_caches.set_call(self.pc, fact);
        fact
    }

    #[inline]
    fn global_slot_from_fact_cache_or_instr(&mut self, function: &Function32, instr: Instr32) -> u16 {
        let slot = function
            .performance
            .global_op(self.pc)
            .map(|fact| fact.slot)
            .or_else(|| self.state.inline_caches.global(self.pc))
            .unwrap_or_else(|| instr.bx());
        self.state.inline_caches.set_global(self.pc, slot);
        slot
    }

    fn static_index_fact(&self, function: &Function32) -> Option<PerfIndexFact> {
        function.performance.index_op(self.pc).copied()
    }
}

fn move_clone_metric(function: &Function32, pc: usize, dst: u16, src: u16) -> VmValueCopyMetric {
    if function.performance.local_copy(pc).is_some() {
        VmValueCopyMetric::LocalStore
    } else if function.performance.is_local_slot(src) && !function.performance.is_local_slot(dst) {
        VmValueCopyMetric::LocalLoad
    } else {
        VmValueCopyMetric::Register
    }
}

pub fn execute32(function: &Function32) -> Result<Exec32Result> {
    Executor32::new(function.register_count).run_function(function)
}

pub fn execute_module32(module: &Module32) -> Result<Exec32Result> {
    let register_count = module
        .entry_function()
        .ok_or_else(|| anyhow!("module entry function {} out of bounds", module.entry))?
        .register_count;
    Executor32::new(register_count).run_module(module)
}

pub fn execute_module32_with_globals(module: &Module32, globals: Vec<RuntimeVal>) -> Result<Exec32Result> {
    let register_count = module
        .entry_function()
        .ok_or_else(|| anyhow!("module entry function {} out of bounds", module.entry))?
        .register_count;
    Executor32::new(register_count).run_module_with_globals(module, globals)
}

pub fn execute_module32_with_globals_and_ctx(
    module: &Module32,
    globals: Vec<RuntimeVal>,
    ctx: &mut VmContext,
) -> Result<Exec32Result> {
    execute_module32_with_globals_heap_and_ctx(module, globals, HeapStore::new(), ctx)
}

pub fn execute_module32_with_globals_heap_and_ctx(
    module: &Module32,
    globals: Vec<RuntimeVal>,
    heap: HeapStore,
    ctx: &mut VmContext,
) -> Result<Exec32Result> {
    let register_count = module
        .entry_function()
        .ok_or_else(|| anyhow!("module entry function {} out of bounds", module.entry))?
        .register_count;
    Executor32::new(register_count).run_module_with_globals_and_ctx(module, globals, heap, ctx)
}

#[cfg(test)]
#[path = "exec32_tests/mod.rs"]
mod exec32_tests;
