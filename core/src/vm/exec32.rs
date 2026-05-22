//! Minimal safe executor for the new `Instr32` VM path.

mod call;
mod const_load;
mod imports;
mod legacy_bridge;
mod named_call;
mod runtime_callable;
mod support;
mod value_ops;

pub use super::RuntimeCallable32;
pub use runtime_callable::{
    call_runtime_callable32, call_runtime_callable32_named, call_runtime_callable32_named_raw,
    call_runtime_callable32_raw, call_runtime_callable32_runtime, runtime_value_to_callable32,
};

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow, bail};

use crate::{
    stmt::{
        Program,
        import::{collect_program_imports, execute_imports},
    },
    val::{
        CallableValue, HeapStore, HeapValue, RuntimeMapKey, RuntimeObject, RuntimeVal, ShortStr, TypedList, TypedMap,
        Val, runtime_val_to_val,
    },
};

use super::{
    CallWindow32, Compiler32, Frame32, Function32, GlobalSlot32, Instr32, Module32, Opcode32, RegisterIndex,
    RuntimeExport32, RuntimeModuleState32, VmContext,
};
use imports::import_runtime_export;
use legacy_bridge::legacy_val_to_runtime_val;
use support::*;

#[derive(Clone, Debug)]
pub struct Exec32Result {
    pub returns: Vec<RuntimeVal>,
    pub frame: Frame32,
    pub state: RuntimeModuleState32,
}

#[derive(Clone, Debug)]
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

    pub fn first_return_to_val(&self) -> Result<Val> {
        runtime_val_to_val(self.first_return(), &self.state.heap)
    }

    pub fn first_return_function(&self) -> Option<RuntimeCallable32> {
        runtime_value_to_callable32(
            self.first_return(),
            &self.state.heap,
            &self.state.globals,
            Arc::clone(&self.module),
        )
    }

    pub fn exports(&self) -> RuntimeExport32 {
        let mut heap = self.state.heap.clone();
        let mut entries = BTreeMap::new();
        for (slot, value) in self.module.globals.iter().zip(self.state.globals.iter()) {
            entries.insert(
                RuntimeMapKey::String(Arc::<str>::from(slot.name.as_str())),
                value.clone(),
            );
        }
        let value = RuntimeVal::Obj(heap.alloc(HeapValue::Map(TypedMap::from_runtime_entries(entries))));
        RuntimeExport32 {
            value,
            state: Arc::new(std::sync::Mutex::new(RuntimeModuleState32 {
                heap,
                globals: self.state.globals.clone(),
            })),
            module: Arc::clone(&self.module),
        }
    }
}

#[derive(Debug)]
pub struct Executor32 {
    frame: Frame32,
    state: RuntimeModuleState32,
    captures: Vec<RuntimeVal>,
    pc: usize,
}

impl Executor32 {
    #[inline]
    pub fn new(register_count: u16) -> Self {
        Self {
            frame: Frame32::new(register_count),
            state: RuntimeModuleState32::default(),
            captures: Vec::new(),
            pc: 0,
        }
    }

    pub fn run_function(self, function: &Function32) -> Result<Exec32Result> {
        let mut ctx = None;
        let mut this = self;
        let returns = this.run_function_inner(function, None, &mut ctx)?;
        Ok(this.finish(returns))
    }

    pub fn run_module(self, module: &Module32) -> Result<Exec32Result> {
        let entry = module
            .entry_function()
            .ok_or_else(|| anyhow!("module entry function {} out of bounds", module.entry))?;
        let mut this = self;
        this.state.globals = vec![RuntimeVal::Nil; module.globals.len()];
        let mut ctx = None;
        let returns = this.run_function_inner(entry, Some(module), &mut ctx)?;
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
        let mut ctx = None;
        let returns = self.run_function_inner(entry, Some(module), &mut ctx)?;
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
        let mut ctx = Some(ctx);
        let returns = self.run_function_inner(entry, Some(module), &mut ctx)?;
        Ok(self.finish(returns))
    }

    pub(crate) fn run_module_function_with_state_recoverable<F>(
        mut self,
        module: &Module32,
        function_index: u32,
        captures: Vec<RuntimeVal>,
        state: RuntimeModuleState32,
        ctx: &mut VmContext,
        seed_args: F,
    ) -> std::result::Result<Exec32Result, Exec32Failure>
    where
        F: FnOnce(&mut Self) -> Result<u16>,
    {
        let function = module
            .functions
            .get(function_index as usize)
            .ok_or_else(|| anyhow!("function index {} out of bounds", function_index))
            .map_err(|error| Exec32Failure {
                error,
                state: state.clone(),
            })?;
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
        let arg_count = seed_args(&mut self).map_err(|error| Exec32Failure {
            error,
            state: self.state.clone(),
        })?;
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
            Ok(returns) => Ok(self.finish(returns)),
            Err(error) => Err(Exec32Failure {
                error,
                state: self.state,
            }),
        }
    }

    fn finish(self, returns: Vec<RuntimeVal>) -> Exec32Result {
        Exec32Result {
            returns,
            frame: self.frame,
            state: self.state,
        }
    }

    fn run_function_inner(
        &mut self,
        function: &Function32,
        module: Option<&Module32>,
        ctx: &mut Option<&mut VmContext>,
    ) -> Result<Vec<RuntimeVal>> {
        if self.frame.len() < function.register_count as usize {
            bail!(
                "executor frame has {} registers, function requires {}",
                self.frame.len(),
                function.register_count
            );
        }

        while self.pc < function.code.len() {
            let instr = function.code[self.pc];
            if self.try_load_const_instr(function, instr)? {
                continue;
            }
            match instr.opcode() {
                Opcode32::Nop => self.pc += 1,
                Opcode32::Move => {
                    let value = self.read(instr.b())?.clone();
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
                Opcode32::LoadFunction => {
                    let function_index = instr.bx() as u32;
                    let module = module.ok_or_else(|| anyhow!("LoadFunction requires Module32 execution"))?;
                    if module.functions.get(function_index as usize).is_none() {
                        bail!("LoadFunction index {} out of bounds", function_index);
                    }
                    let value = RuntimeVal::Obj(self.state.heap.alloc(HeapValue::Callable(CallableValue::Closure {
                        function_index,
                        captures: Vec::new(),
                    })));
                    self.write(instr.a(), value)?;
                    self.pc += 1;
                }
                Opcode32::MakeClosure => {
                    let function_index = instr.b() as u32;
                    let module = module.ok_or_else(|| anyhow!("MakeClosure requires Module32 execution"))?;
                    let function = module
                        .functions
                        .get(function_index as usize)
                        .ok_or_else(|| anyhow!("MakeClosure index {} out of bounds", function_index))?;
                    let captures = self.read_register_range(instr.c(), checked_u8_count(function.capture_count)?)?;
                    let value = RuntimeVal::Obj(self.state.heap.alloc(HeapValue::Callable(CallableValue::Closure {
                        function_index,
                        captures,
                    })));
                    self.write(instr.a(), value)?;
                    self.pc += 1;
                }
                Opcode32::LoadNative => {
                    let native_index = instr.bx() as usize;
                    let module = module.ok_or_else(|| anyhow!("LoadNative requires Module32 execution"))?;
                    let native = module
                        .natives
                        .get(native_index)
                        .ok_or_else(|| anyhow!("LoadNative index {} out of bounds", native_index))?;
                    let value = RuntimeVal::Obj(self.state.heap.alloc(HeapValue::Callable(CallableValue::Native {
                        function_index: native_index as u32,
                        arity: native.arity,
                    })));
                    self.write(instr.a(), value)?;
                    self.pc += 1;
                }
                Opcode32::AddInt => self.dynamic_add(instr)?,
                Opcode32::SubInt => {
                    self.dynamic_numeric_binary(instr, |lhs, rhs| lhs.wrapping_sub(rhs), |lhs, rhs| lhs - rhs)?
                }
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
                    bail!("{message}");
                }
                Opcode32::Test => {
                    let truthy = self.truthy(instr.a())?;
                    if truthy == (instr.b() != 0) {
                        self.pc += 1;
                    } else {
                        self.pc = self.relative_pc(instr.c() as i8 as i32)?;
                    }
                }
                Opcode32::Jmp => {
                    self.pc = self.relative_pc(instr.sj_arg())?;
                }
                Opcode32::NewList => {
                    let values = self.read_register_range(instr.b(), instr.c())?;
                    let list = HeapValue::List(TypedList::from_runtime_values(values, &self.state.heap));
                    let handle = self.state.heap.alloc(list);
                    self.write(instr.a(), RuntimeVal::Obj(handle))?;
                    self.pc += 1;
                }
                Opcode32::NewMap => {
                    let map = self.read_map_entries(instr.b(), instr.c())?;
                    let handle = self
                        .state
                        .heap
                        .alloc(HeapValue::Map(TypedMap::from_runtime_entries(map)));
                    self.write(instr.a(), RuntimeVal::Obj(handle))?;
                    self.pc += 1;
                }
                Opcode32::NewObject => {
                    let object = self.read_object_fields(instr.b(), instr.c())?;
                    let handle = self.state.heap.alloc(HeapValue::Object(object));
                    self.write(instr.a(), RuntimeVal::Obj(handle))?;
                    self.pc += 1;
                }
                Opcode32::NewRange => {
                    let list = self.build_int_range(instr.b(), instr.c() != 0)?;
                    let handle = self.state.heap.alloc(HeapValue::List(TypedList::Int(list)));
                    self.write(instr.a(), RuntimeVal::Obj(handle))?;
                    self.pc += 1;
                }
                Opcode32::Len => {
                    let len = self.len_value(instr.b())?;
                    self.write(instr.a(), RuntimeVal::Int(len as i64))?;
                    self.pc += 1;
                }
                Opcode32::ToIter => {
                    let iter = self.to_iter(instr.b())?;
                    self.write(instr.a(), iter)?;
                    self.pc += 1;
                }
                Opcode32::GetIndex => {
                    let value = self.get_index(instr.b(), instr.c())?;
                    self.write(instr.a(), value)?;
                    self.pc += 1;
                }
                Opcode32::SetIndex => {
                    self.set_index(instr.a(), instr.b(), instr.c())?;
                    self.pc += 1;
                }
                Opcode32::Call => {
                    if instr.a() != instr.b() {
                        bail!(
                            "Call return base {} must match call window base {}",
                            instr.a(),
                            instr.b()
                        );
                    }
                    let window = CallWindow32::new(RegisterIndex::new(instr.b() as u16), instr.c() as u16, 1);
                    let value = self.call_function(module, window, ctx)?;
                    self.frame.write_returns(window, [value]);
                    self.pc += 1;
                }
                Opcode32::CallNamed => {
                    let payload = instr.bx();
                    let positional_count = (payload & 0x7f) as u16;
                    let named_count = (payload >> 7) as u16;
                    let window = CallWindow32::new(RegisterIndex::new(instr.a() as u16), positional_count, 1);
                    let value = self.call_function_named(module, window, named_count, ctx)?;
                    self.frame.write_returns(window, [value]);
                    self.pc += 1;
                }
                Opcode32::GetGlobal => {
                    let value = self.read_global(instr.bx())?;
                    self.write(instr.a(), value)?;
                    self.pc += 1;
                }
                Opcode32::SetGlobal => {
                    let value = self.read(instr.a())?.clone();
                    self.write_global(instr.bx(), value)?;
                    self.pc += 1;
                }
                Opcode32::Return => {
                    let base = instr.a() as usize;
                    let count = instr.b() as usize;
                    if base + count > self.frame.len() {
                        bail!("Return range out of bounds");
                    }
                    let returns = (0..count)
                        .map(|offset| {
                            self.frame
                                .read(RegisterIndex::new((base + offset) as u16))
                                .cloned()
                                .expect("return bounds checked")
                        })
                        .collect();
                    return Ok(returns);
                }
                other => bail!("Opcode32 {:?} is not implemented in Executor32 yet", other),
            }
        }

        Ok(Vec::new())
    }

    fn dynamic_add(&mut self, instr: Instr32) -> Result<()> {
        let lhs = self.read(instr.b())?.clone();
        let rhs = self.read(instr.c())?.clone();
        let value = match (&lhs, &rhs) {
            (RuntimeVal::Int(lhs), RuntimeVal::Int(rhs)) => RuntimeVal::Int(lhs.wrapping_add(*rhs)),
            (RuntimeVal::Int(lhs), RuntimeVal::Float(rhs)) => RuntimeVal::Float(*lhs as f64 + *rhs),
            (RuntimeVal::Float(lhs), RuntimeVal::Int(rhs)) => RuntimeVal::Float(*lhs + *rhs as f64),
            (RuntimeVal::Float(lhs), RuntimeVal::Float(rhs)) => RuntimeVal::Float(*lhs + *rhs),
            _ if self.runtime_value_is_heap_list(&lhs)? || self.runtime_value_is_heap_list(&rhs)? => {
                let mut values = match self.runtime_value_to_list_values(&lhs)? {
                    Some(values) => values,
                    None => vec![lhs.clone()],
                };
                match self.runtime_value_to_list_values(&rhs)? {
                    Some(rhs) => values.extend(rhs),
                    None => values.push(rhs.clone()),
                }
                let list = TypedList::from_runtime_values(values, &self.state.heap);
                RuntimeVal::Obj(self.state.heap.alloc(HeapValue::List(list)))
            }
            _ if self.runtime_value_to_string(&lhs)?.is_some() || self.runtime_value_to_string(&rhs)?.is_some() => {
                let lhs = self.runtime_value_display_string(&lhs)?;
                let rhs = self.runtime_value_display_string(&rhs)?;
                self.runtime_value_from_string(Arc::<str>::from(format!("{lhs}{rhs}")))
            }
            _ => bail!(
                "Add expected numbers or strings, got {:?} and {:?}",
                lhs.kind(),
                rhs.kind()
            ),
        };
        self.write(instr.a(), value)?;
        self.pc += 1;
        Ok(())
    }

    fn dynamic_numeric_binary(
        &mut self,
        instr: Instr32,
        int_op: impl FnOnce(i64, i64) -> i64,
        float_op: impl FnOnce(f64, f64) -> f64,
    ) -> Result<()> {
        let lhs = self.read(instr.b())?;
        let rhs = self.read(instr.c())?;
        let value = match (lhs, rhs) {
            (RuntimeVal::Int(lhs), RuntimeVal::Int(rhs)) => RuntimeVal::Int(int_op(*lhs, *rhs)),
            _ => RuntimeVal::Float(float_op(self.number_value(lhs)?, self.number_value(rhs)?)),
        };
        self.write(instr.a(), value)?;
        self.pc += 1;
        Ok(())
    }

    #[inline]
    fn float_binary(&mut self, instr: Instr32, op: impl FnOnce(f64, f64) -> f64) -> Result<()> {
        let lhs = self.read_number(instr.b())?;
        let rhs = self.read_number(instr.c())?;
        self.write(instr.a(), RuntimeVal::Float(op(lhs, rhs)))?;
        self.pc += 1;
        Ok(())
    }

    #[inline]
    fn int_compare(&mut self, instr: Instr32, op: impl FnOnce(i64, i64) -> bool) -> Result<()> {
        let lhs = self.read_int(instr.b())?;
        let rhs = self.read_int(instr.c())?;
        self.write(instr.a(), RuntimeVal::Bool(op(lhs, rhs)))?;
        self.pc += 1;
        Ok(())
    }

    fn values_equal(&self, lhs: u8, rhs: u8) -> Result<bool> {
        let lhs = self.read(lhs)?.clone();
        let rhs = self.read(rhs)?.clone();
        Ok(match (&lhs, &rhs) {
            (RuntimeVal::Nil, RuntimeVal::Nil) => true,
            (RuntimeVal::Bool(lhs), RuntimeVal::Bool(rhs)) => lhs == rhs,
            (RuntimeVal::Int(lhs), RuntimeVal::Int(rhs)) => lhs == rhs,
            (RuntimeVal::Float(lhs), RuntimeVal::Float(rhs)) => lhs == rhs,
            (RuntimeVal::Int(lhs), RuntimeVal::Float(rhs)) => *lhs as f64 == *rhs,
            (RuntimeVal::Float(lhs), RuntimeVal::Int(rhs)) => *lhs == *rhs as f64,
            (RuntimeVal::Obj(lhs), RuntimeVal::Obj(rhs)) if lhs == rhs => true,
            _ => match (self.runtime_value_to_string(&lhs)?, self.runtime_value_to_string(&rhs)?) {
                (Some(lhs), Some(rhs)) => lhs == rhs,
                _ => false,
            },
        })
    }

    #[inline]
    fn read(&self, register: u8) -> Result<&RuntimeVal> {
        self.frame
            .read(RegisterIndex::new(register as u16))
            .ok_or_else(|| anyhow!("register {} out of bounds", register))
    }

    #[inline]
    fn write(&mut self, register: u8, value: RuntimeVal) -> Result<()> {
        if self.frame.read(RegisterIndex::new(register as u16)).is_none() {
            bail!("register {} out of bounds", register);
        }
        self.frame.write(RegisterIndex::new(register as u16), value);
        Ok(())
    }

    #[inline]
    pub(crate) fn heap_mut(&mut self) -> &mut HeapStore {
        &mut self.state.heap
    }

    #[inline]
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

    fn build_int_range(&self, base: u8, inclusive: bool) -> Result<Vec<i64>> {
        let start = self.read_int(base)?;
        let end = self.read_int(base.checked_add(1).ok_or_else(|| anyhow!("range base overflow"))?)?;
        let step = self.read_int(base.checked_add(2).ok_or_else(|| anyhow!("range base overflow"))?)?;
        if step == 0 {
            bail!("Range step cannot be zero");
        }

        let mut out = Vec::new();
        let mut current = start;
        if step > 0 {
            while if inclusive { current <= end } else { current < end } {
                out.push(current);
                current = current
                    .checked_add(step)
                    .ok_or_else(|| anyhow!("Range step overflow"))?;
            }
        } else {
            while if inclusive { current >= end } else { current > end } {
                out.push(current);
                current = current
                    .checked_add(step)
                    .ok_or_else(|| anyhow!("Range step overflow"))?;
            }
        }
        Ok(out)
    }

    #[inline]
    fn truthy(&self, register: u8) -> Result<bool> {
        Ok(!matches!(
            self.read(register)?,
            RuntimeVal::Nil | RuntimeVal::Bool(false)
        ))
    }

    fn read_register_range(&self, base: u8, count: u8) -> Result<Vec<RuntimeVal>> {
        let base = base as usize;
        let count = count as usize;
        if base + count > self.frame.len() {
            bail!("register range {}..{} out of bounds", base, base + count);
        }
        (0..count)
            .map(|offset| {
                self.frame
                    .read(RegisterIndex::new((base + offset) as u16))
                    .cloned()
                    .ok_or_else(|| anyhow!("register {} out of bounds", base + offset))
            })
            .collect()
    }

    fn read_global(&self, slot: u16) -> Result<RuntimeVal> {
        self.state
            .globals
            .get(slot as usize)
            .cloned()
            .ok_or_else(|| anyhow!("global slot {} out of bounds", slot))
    }

    fn write_global(&mut self, slot: u16, value: RuntimeVal) -> Result<()> {
        let Some(target) = self.state.globals.get_mut(slot as usize) else {
            bail!("global slot {} out of bounds", slot);
        };
        *target = value;
        Ok(())
    }

    fn read_map_entries(&self, base: u8, count: u8) -> Result<BTreeMap<RuntimeMapKey, RuntimeVal>> {
        let mut values = BTreeMap::new();
        for entry in 0..count {
            let key_reg = base
                .checked_add(entry.checked_mul(2).expect("map entry register overflow"))
                .ok_or_else(|| anyhow!("map key register overflow"))?;
            let value_reg = key_reg
                .checked_add(1)
                .ok_or_else(|| anyhow!("map value register overflow"))?;
            let key = self.map_key_from_register(key_reg)?;
            let value = self.read(value_reg)?.clone();
            values.insert(key, value);
        }
        Ok(values)
    }

    fn read_object_fields(&self, base: u8, count: u8) -> Result<RuntimeObject> {
        let type_name = Arc::<str>::from(self.to_runtime_string(base)?);
        let field_base = base
            .checked_add(1)
            .ok_or_else(|| anyhow!("object field base overflow"))?;
        let mut fields = BTreeMap::new();
        for entry in 0..count {
            let offset = entry
                .checked_mul(2)
                .ok_or_else(|| anyhow!("object field register overflow"))?;
            let key_reg = field_base
                .checked_add(offset)
                .ok_or_else(|| anyhow!("object key register overflow"))?;
            let value_reg = key_reg
                .checked_add(1)
                .ok_or_else(|| anyhow!("object value register overflow"))?;
            fields.insert(
                Arc::<str>::from(self.to_runtime_string(key_reg)?),
                self.read(value_reg)?.clone(),
            );
        }
        Ok(RuntimeObject { type_name, fields })
    }

    fn get_index(&mut self, target_reg: u8, key_reg: u8) -> Result<RuntimeVal> {
        match self.read(target_reg)?.clone() {
            RuntimeVal::ShortStr(value) => {
                let value = Arc::<str>::from(value.as_str());
                self.index_string(&value, key_reg)
            }
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(handle)
                .cloned()
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::List(list) => self.index_list(&list, key_reg),
                HeapValue::Map(map) => {
                    let key = self.map_key_from_register(key_reg)?;
                    Ok(self.lookup_map(&map, &key).unwrap_or(RuntimeVal::Nil))
                }
                HeapValue::Object(object) => {
                    let key = self.object_key_from_register(key_reg)?;
                    Ok(object.fields.get(&key).cloned().unwrap_or(RuntimeVal::Nil))
                }
                HeapValue::String(value) => self.index_string(&value, key_reg),
                other => bail!("GetIndex target object is not indexable: {:?}", heap_kind(&other)),
            },
            other => bail!("GetIndex target expected Obj, got {:?}", other.kind()),
        }
    }

    fn len_value(&self, register: u8) -> Result<usize> {
        match self.read(register)? {
            RuntimeVal::ShortStr(value) => Ok(value.as_str().chars().count()),
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::String(value) => Ok(value.chars().count()),
                HeapValue::List(value) => Ok(value.len()),
                HeapValue::Map(value) => Ok(value.len()),
                other => bail!("Len target object is not sized: {:?}", heap_kind(other)),
            },
            other => bail!("Len target expected string/list/map, got {:?}", other.kind()),
        }
    }

    fn contains_value(&self, needle_reg: u8, haystack_reg: u8) -> Result<bool> {
        let needle = self.read(needle_reg)?.clone();
        match self.read(haystack_reg)?.clone() {
            RuntimeVal::ShortStr(haystack) => {
                let Some(needle) = self.runtime_value_to_string(&needle)? else {
                    return Ok(false);
                };
                Ok(haystack.as_str().contains(needle.as_ref()))
            }
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::String(haystack) => {
                    let Some(needle) = self.runtime_value_to_string(&needle)? else {
                        return Ok(false);
                    };
                    Ok(haystack.contains(needle.as_ref()))
                }
                HeapValue::List(values) => self.list_contains(values, &needle),
                HeapValue::Map(values) => self.map_contains(values, &needle),
                other => bail!("Contains haystack object is not searchable: {:?}", heap_kind(other)),
            },
            other => bail!("Contains haystack expected string/list/map, got {:?}", other.kind()),
        }
    }

    fn slice_from(&mut self, target_reg: u8, start_reg: u8) -> Result<RuntimeVal> {
        let start = usize::try_from(self.read_int(start_reg)?)
            .map_err(|_| anyhow!("SliceFrom start index must be non-negative"))?;
        match self.read(target_reg)?.clone() {
            RuntimeVal::ShortStr(value) => self.slice_string_from(Arc::<str>::from(value.as_str()), start),
            RuntimeVal::Obj(handle) => {
                let value = self
                    .state
                    .heap
                    .get(handle)
                    .cloned()
                    .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
                match value {
                    HeapValue::List(values) => Ok(RuntimeVal::Obj(
                        self.state.heap.alloc(HeapValue::List(values.slice_from(start))),
                    )),
                    HeapValue::String(value) => self.slice_string_from(value.clone(), start),
                    other => bail!("SliceFrom target object is not sliceable: {:?}", heap_kind(&other)),
                }
            }
            other => bail!("SliceFrom target expected string/list object, got {:?}", other.kind()),
        }
    }

    fn slice_string_from(&mut self, value: Arc<str>, start: usize) -> Result<RuntimeVal> {
        let suffix = value.chars().skip(start).collect::<String>();
        Ok(self.runtime_value_from_string(Arc::<str>::from(suffix)))
    }

    fn map_rest(&mut self, base: u8, key_count: u8) -> Result<RuntimeVal> {
        let RuntimeVal::Obj(handle) = self.read(base)?.clone() else {
            bail!("MapRest base expected map object");
        };
        let source = self
            .state
            .heap
            .get(handle)
            .cloned()
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?;
        let HeapValue::Map(map) = &source else {
            bail!("MapRest source object is not a map: {:?}", heap_kind(&source));
        };

        let mut entries = map.entries().into_iter().collect::<BTreeMap<_, _>>();
        for offset in 0..key_count {
            let key_reg = base
                .checked_add(1)
                .and_then(|reg| reg.checked_add(offset))
                .ok_or_else(|| anyhow!("MapRest key register overflow"))?;
            let key = self.map_key_from_register(key_reg)?;
            remove_runtime_entry(&mut entries, &key);
        }
        Ok(RuntimeVal::Obj(
            self.state
                .heap
                .alloc(HeapValue::Map(TypedMap::from_runtime_entries(entries))),
        ))
    }

    fn list_contains(&self, values: &TypedList, needle: &RuntimeVal) -> Result<bool> {
        Ok(match values {
            TypedList::Mixed(values) => values.iter().any(|value| value == needle),
            TypedList::Int(values) => matches!(needle, RuntimeVal::Int(needle) if values.contains(needle)),
            TypedList::Float(values) => matches!(needle, RuntimeVal::Float(needle) if values.contains(needle)),
            TypedList::Bool(values) => matches!(needle, RuntimeVal::Bool(needle) if values.contains(needle)),
            TypedList::String(values) => {
                let Some(needle) = self.runtime_value_to_string(needle)? else {
                    return Ok(false);
                };
                values.iter().any(|value| value.as_ref() == needle.as_ref())
            }
        })
    }

    fn map_contains(&self, values: &TypedMap, needle: &RuntimeVal) -> Result<bool> {
        Ok(match values {
            TypedMap::Mixed(values) => {
                let key = self.runtime_map_key_from_value(needle)?;
                values.contains_key(&key)
            }
            TypedMap::StringMixed(values) => self.string_map_contains_key(values, needle)?,
            TypedMap::StringInt(values) => self.string_map_contains_key(values, needle)?,
            TypedMap::StringFloat(values) => self.string_map_contains_key(values, needle)?,
            TypedMap::StringBool(values) => self.string_map_contains_key(values, needle)?,
        })
    }

    fn to_iter(&mut self, register: u8) -> Result<RuntimeVal> {
        match self.read(register)?.clone() {
            RuntimeVal::ShortStr(value) => {
                let list = value
                    .as_str()
                    .chars()
                    .map(|ch| Arc::<str>::from(ch.to_string()))
                    .collect();
                Ok(RuntimeVal::Obj(
                    self.state.heap.alloc(HeapValue::List(TypedList::String(list))),
                ))
            }
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(handle)
                .cloned()
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::List(_) => Ok(RuntimeVal::Obj(handle)),
                HeapValue::String(value) => {
                    let list = value.chars().map(|ch| Arc::<str>::from(ch.to_string())).collect();
                    Ok(RuntimeVal::Obj(
                        self.state.heap.alloc(HeapValue::List(TypedList::String(list))),
                    ))
                }
                HeapValue::Map(map) => self.map_to_iter_list(&map),
                other => bail!("ToIter target object is not iterable: {:?}", heap_kind(&other)),
            },
            other => bail!("ToIter target expected string/list/map, got {:?}", other.kind()),
        }
    }

    fn map_to_iter_list(&mut self, map: &TypedMap) -> Result<RuntimeVal> {
        let mut pairs = Vec::with_capacity(map.len());
        for (key, value) in map.entries() {
            let key = self.runtime_map_key_to_value(key);
            let pair = HeapValue::List(TypedList::from_runtime_values(vec![key, value], &self.state.heap));
            pairs.push(RuntimeVal::Obj(self.state.heap.alloc(pair)));
        }
        Ok(RuntimeVal::Obj(
            self.state.heap.alloc(HeapValue::List(TypedList::Mixed(pairs))),
        ))
    }

    fn runtime_map_key_to_value(&mut self, key: RuntimeMapKey) -> RuntimeVal {
        match key {
            RuntimeMapKey::Nil => RuntimeVal::Nil,
            RuntimeMapKey::Bool(value) => RuntimeVal::Bool(value),
            RuntimeMapKey::Int(value) => RuntimeVal::Int(value),
            RuntimeMapKey::ShortStr(value) => RuntimeVal::ShortStr(value),
            RuntimeMapKey::String(value) => {
                if let Some(short) = ShortStr::new(&value) {
                    RuntimeVal::ShortStr(short)
                } else {
                    RuntimeVal::Obj(self.state.heap.alloc(HeapValue::String(value)))
                }
            }
        }
    }

    fn set_index(&mut self, target_reg: u8, key_reg: u8, value_reg: u8) -> Result<()> {
        let target = self.read(target_reg)?.clone();
        let value = self.read(value_reg)?.clone();
        let RuntimeVal::Obj(handle) = target else {
            bail!("SetIndex target expected Obj, got {:?}", target.kind());
        };
        let key = self.map_key_from_register(key_reg)?;

        if let Some(done) = self.try_set_string_list(handle, &key, value.clone())? {
            return Ok(done);
        }

        match self
            .state
            .heap
            .get_mut(handle)
            .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
        {
            HeapValue::List(list) => {
                let RuntimeMapKey::Int(index) = key else {
                    bail!("SetIndex list key must be Int");
                };
                let index = usize::try_from(index).map_err(|_| anyhow!("list index must be non-negative"))?;
                set_list_value(list, index, value)
            }
            HeapValue::Map(map) => {
                map.set(key, value);
                Ok(())
            }
            HeapValue::Object(object) => {
                let Some(key) = key.as_arc_str() else {
                    bail!("SetIndex object key must be string");
                };
                object.fields.insert(key, value);
                Ok(())
            }
            other => bail!("SetIndex target object is not writable: {:?}", heap_kind(other)),
        }
    }

    fn object_key_from_register(&self, register: u8) -> Result<Arc<str>> {
        match self.read(register)? {
            RuntimeVal::ShortStr(value) => Ok(Arc::<str>::from(value.as_str())),
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::String(value) => Ok(value.clone()),
                other => bail!("object field key cannot be object: {:?}", heap_kind(other)),
            },
            other => bail!("object field key must be string, got {:?}", other.kind()),
        }
    }

    fn try_set_string_list(
        &mut self,
        handle: crate::val::HeapRef,
        key: &RuntimeMapKey,
        value: RuntimeVal,
    ) -> Result<Option<()>> {
        let RuntimeMapKey::Int(index) = key else {
            return Ok(None);
        };
        let Some(HeapValue::List(TypedList::String(values))) = self.state.heap.get(handle) else {
            return Ok(None);
        };
        let index = usize::try_from(*index).map_err(|_| anyhow!("list index must be non-negative"))?;
        if index >= values.len() {
            bail!("list index {} out of bounds", index);
        }

        if let Some(value) = self.runtime_value_to_string(&value)? {
            let Some(HeapValue::List(TypedList::String(values))) = self.state.heap.get_mut(handle) else {
                bail!("heap object {} changed while writing string list", handle.index());
            };
            values[index] = value;
            return Ok(Some(()));
        }

        let strings = values.clone();
        let mut mixed = Vec::with_capacity(strings.len());
        for value in strings {
            mixed.push(self.runtime_value_from_string(value));
        }
        mixed[index] = value;
        let Some(HeapValue::List(list)) = self.state.heap.get_mut(handle) else {
            bail!("heap object {} changed while materializing string list", handle.index());
        };
        *list = TypedList::Mixed(mixed);
        Ok(Some(()))
    }

    fn index_list(&mut self, list: &TypedList, key_reg: u8) -> Result<RuntimeVal> {
        let index = usize::try_from(self.read_int(key_reg)?).map_err(|_| anyhow!("list index must be non-negative"))?;
        Ok(match list {
            TypedList::Mixed(values) => values.get(index).cloned(),
            TypedList::Int(values) => values.get(index).copied().map(RuntimeVal::Int),
            TypedList::Float(values) => values.get(index).copied().map(RuntimeVal::Float),
            TypedList::Bool(values) => values.get(index).copied().map(RuntimeVal::Bool),
            TypedList::String(values) => values.get(index).map(|value| {
                if let Some(short) = ShortStr::new(value) {
                    RuntimeVal::ShortStr(short)
                } else {
                    RuntimeVal::Obj(self.state.heap.alloc(HeapValue::String(value.clone())))
                }
            }),
        }
        .unwrap_or(RuntimeVal::Nil))
    }

    fn index_string(&self, value: &Arc<str>, key_reg: u8) -> Result<RuntimeVal> {
        let index =
            usize::try_from(self.read_int(key_reg)?).map_err(|_| anyhow!("string index must be non-negative"))?;
        let Some(ch) = value.chars().nth(index) else {
            return Ok(RuntimeVal::Nil);
        };
        let mut buf = [0_u8; 4];
        let ch = ch.encode_utf8(&mut buf);
        if let Some(short) = ShortStr::new(ch) {
            Ok(RuntimeVal::ShortStr(short))
        } else {
            Ok(RuntimeVal::Nil)
        }
    }

    fn lookup_map(&self, map: &TypedMap, key: &RuntimeMapKey) -> Option<RuntimeVal> {
        map.get(key)
    }

    fn map_key_from_register(&self, register: u8) -> Result<RuntimeMapKey> {
        self.runtime_map_key_from_value(self.read(register)?)
    }

    fn runtime_map_key_from_value(&self, value: &RuntimeVal) -> Result<RuntimeMapKey> {
        match value {
            RuntimeVal::Nil => Ok(RuntimeMapKey::Nil),
            RuntimeVal::Bool(value) => Ok(RuntimeMapKey::Bool(*value)),
            RuntimeVal::Int(value) => Ok(RuntimeMapKey::Int(*value)),
            RuntimeVal::ShortStr(value) => Ok(RuntimeMapKey::ShortStr(*value)),
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::String(value) => Ok(RuntimeMapKey::String(value.clone())),
                other => bail!("object cannot be used as map key: {:?}", heap_kind(other)),
            },
            RuntimeVal::Float(_) => bail!("Float cannot be used as RuntimeMapKey"),
        }
    }

    fn runtime_value_to_key_string(&self, value: &RuntimeVal) -> Result<Option<Arc<str>>> {
        Ok(match value {
            RuntimeVal::Bool(value) => Some(Arc::<str>::from(value.to_string())),
            RuntimeVal::Int(value) => Some(Arc::<str>::from(value.to_string())),
            RuntimeVal::Float(value) => Some(Arc::<str>::from(value.to_string())),
            RuntimeVal::ShortStr(value) => Some(Arc::<str>::from(value.as_str())),
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(*handle)
                .ok_or_else(|| anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::String(value) => Some(value.clone()),
                _ => None,
            },
            RuntimeVal::Nil => None,
        })
    }

    fn string_map_contains_key<T>(&self, values: &BTreeMap<Arc<str>, T>, needle: &RuntimeVal) -> Result<bool> {
        let Some(key) = self.runtime_value_to_key_string(needle)? else {
            return Ok(false);
        };
        Ok(values.contains_key(key.as_ref()))
    }

    #[inline]
    fn relative_pc(&self, offset: i32) -> Result<usize> {
        let next = self.pc as i64 + 1 + offset as i64;
        if next < 0 {
            bail!("jump before start of function");
        }
        Ok(next as usize)
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

pub fn execute_program32_to_val(program: &Program) -> Result<Val> {
    let module = Compiler32::compile_module(program)?;
    let result = execute_module32(&module)?;
    let value = result.returns.first().unwrap_or(&RuntimeVal::Nil);
    runtime_val_to_val(value, &result.state.heap)
}

pub fn execute_program32_raw_with_ctx(program: &Program, ctx: &mut super::VmContext) -> Result<Program32Result> {
    let imports = collect_program_imports(program);
    let resolver = ctx.resolver().clone();
    execute_imports(&imports, resolver.as_ref(), ctx)?;

    let mut seed_heap = HeapStore::new();
    let mut external_globals = Vec::new();
    let mut external_values = BTreeMap::new();
    let mut context_sync_globals = BTreeSet::new();
    let mut natives = Vec::new();
    for (name, value) in ctx.iter() {
        if let Ok(value) = legacy_val_to_runtime_val(name, value, &mut seed_heap, &mut natives) {
            external_globals.push(name.clone());
            context_sync_globals.insert(name.clone());
            external_values.insert(name.clone(), value);
        }
    }
    for (name, value) in ctx.runtime_globals_iter() {
        external_globals.push(name.clone());
        let value = import_runtime_export(value, &mut seed_heap, &mut natives)?;
        external_values.insert(name.clone(), value);
    }

    let module = Arc::new(Compiler32::compile_module_with_natives_and_globals(
        program,
        natives,
        external_globals,
    )?);
    let globals = seed_module_globals(&module.globals, external_values);
    let result = execute_module32_with_globals_heap_and_ctx(module.as_ref(), globals, seed_heap, ctx)?;
    sync_module_globals_to_context(&module, &result, ctx, &context_sync_globals)?;
    Ok(Program32Result {
        returns: result.returns,
        state: result.state,
        module,
    })
}

pub fn execute_program32_with_ctx(program: &Program, ctx: &mut super::VmContext) -> Result<Val> {
    execute_program32_raw_with_ctx(program, ctx)?.first_return_to_val()
}

pub fn execute_source32_to_val(source: &str) -> Result<Val> {
    let module = Compiler32::compile_source_module(source)?;
    let result = execute_module32(&module)?;
    let value = result.returns.first().unwrap_or(&RuntimeVal::Nil);
    runtime_val_to_val(value, &result.state.heap)
}

fn seed_module_globals(slots: &[GlobalSlot32], values: BTreeMap<String, RuntimeVal>) -> Vec<RuntimeVal> {
    slots
        .iter()
        .map(|slot| values.get(&slot.name).cloned().unwrap_or(RuntimeVal::Nil))
        .collect()
}

fn sync_module_globals_to_context(
    module: &Arc<Module32>,
    result: &Exec32Result,
    ctx: &mut super::VmContext,
    names: &BTreeSet<String>,
) -> Result<()> {
    for (slot, value) in module.globals.iter().zip(result.state.globals.iter()) {
        if !names.contains(&slot.name) {
            continue;
        }
        if let Ok(value) = runtime_val_to_val(value, &result.state.heap) {
            ctx.set(slot.name.clone(), value);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "exec32_tests.rs"]
mod exec32_tests;
