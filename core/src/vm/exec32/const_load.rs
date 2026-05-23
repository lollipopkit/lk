use anyhow::{Result, anyhow};

use crate::{
    val::{HeapValue, RuntimeVal, ShortStr, TypedList, TypedMap},
    vm::{ConstHeapValue32, ConstRuntimeValue32, Function32, Instr32, Opcode32},
};

use super::Executor32;

impl Executor32 {
    pub(super) fn try_load_const_instr(&mut self, function: &Function32, instr: Instr32) -> Result<bool> {
        let dead_write = function.performance.is_dead_write(self.pc);
        match instr.opcode() {
            Opcode32::LoadNil => {
                if !dead_write {
                    self.write(instr.a(), RuntimeVal::Nil)?;
                }
            }
            Opcode32::LoadBool => {
                if !dead_write {
                    self.write(instr.a(), RuntimeVal::Bool(instr.b() != 0))?;
                }
            }
            Opcode32::LoadInt => {
                let value = function
                    .consts
                    .int(instr.bx())
                    .ok_or_else(|| anyhow!("LoadInt const index {} out of bounds", instr.bx()))?;
                if !dead_write {
                    self.write(instr.a(), RuntimeVal::Int(value))?;
                }
            }
            Opcode32::LoadFloat => {
                let value = function
                    .consts
                    .float(instr.bx())
                    .ok_or_else(|| anyhow!("LoadFloat const index {} out of bounds", instr.bx()))?;
                if !dead_write {
                    self.write(instr.a(), RuntimeVal::Float(value))?;
                }
            }
            Opcode32::LoadString => {
                let value = function
                    .consts
                    .string(instr.bx())
                    .ok_or_else(|| anyhow!("LoadString const index {} out of bounds", instr.bx()))?;
                let value = if let Some(short) = ShortStr::new(value) {
                    RuntimeVal::ShortStr(short)
                } else {
                    RuntimeVal::Obj(self.state.heap.alloc(HeapValue::String(value.into())))
                };
                if !dead_write {
                    self.write(instr.a(), value)?;
                }
            }
            Opcode32::LoadHeapConst => {
                let value = function
                    .consts
                    .heap_value(instr.bx())
                    .ok_or_else(|| anyhow!("LoadHeapConst const index {} out of bounds", instr.bx()))?;
                let value = self.materialize_heap_const(value.clone())?;
                if !dead_write {
                    let handle = self.state.heap.alloc(value);
                    self.write(instr.a(), RuntimeVal::Obj(handle))?;
                }
            }
            _ => return Ok(false),
        }

        self.pc += 1;
        Ok(true)
    }

    fn materialize_const_value(&mut self, value: ConstRuntimeValue32) -> Result<RuntimeVal> {
        Ok(match value {
            ConstRuntimeValue32::Nil => RuntimeVal::Nil,
            ConstRuntimeValue32::Bool(value) => RuntimeVal::Bool(value),
            ConstRuntimeValue32::Int(value) => RuntimeVal::Int(value),
            ConstRuntimeValue32::Float(value) => RuntimeVal::Float(value),
            ConstRuntimeValue32::ShortStr(value) => RuntimeVal::ShortStr(value),
            ConstRuntimeValue32::Heap(value) => {
                let value = self.materialize_heap_const(*value)?;
                RuntimeVal::Obj(self.state.heap.alloc(value))
            }
        })
    }

    fn materialize_heap_const(&mut self, value: ConstHeapValue32) -> Result<HeapValue> {
        Ok(match value {
            ConstHeapValue32::LongString(value) => HeapValue::String(value),
            ConstHeapValue32::List(values) => {
                let mut runtime_values = Vec::with_capacity(values.len());
                for value in values {
                    runtime_values.push(self.materialize_const_value(value)?);
                }
                HeapValue::List(TypedList::from_runtime_values(runtime_values, &self.state.heap))
            }
            ConstHeapValue32::Map(values) => {
                let mut runtime_entries = std::collections::BTreeMap::new();
                for (key, value) in values {
                    runtime_entries.insert(key, self.materialize_const_value(value)?);
                }
                HeapValue::Map(TypedMap::from_runtime_entries(runtime_entries))
            }
            ConstHeapValue32::UpvalCell(value) => HeapValue::UpvalCell(self.materialize_const_value(*value)?),
        })
    }
}
