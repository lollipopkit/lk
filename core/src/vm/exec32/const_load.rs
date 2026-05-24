use anyhow::{Result, anyhow};

use std::sync::Arc;

use crate::{
    val::{HeapValue, RuntimeVal, ShortStr, TypedList, typed_map_from_entries},
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
                let list = self.materialize_const_list(values)?;
                HeapValue::List(list)
            }
            ConstHeapValue32::Map(values) => {
                let mut runtime_entries = std::collections::BTreeMap::new();
                for (key, value) in values {
                    runtime_entries.insert(key, self.materialize_const_value(value)?);
                }
                HeapValue::Map(typed_map_from_entries(runtime_entries))
            }
            ConstHeapValue32::UpvalCell(value) => HeapValue::UpvalCell(self.materialize_const_value(*value)?),
        })
    }

    fn materialize_const_list(&mut self, values: Vec<ConstRuntimeValue32>) -> Result<TypedList> {
        let mut original = Vec::with_capacity(values.len());
        let mut shape = ConstListShape::Empty;
        for value in values {
            let value = self.materialize_const_value(value)?;
            shape = append_const_list_shape(shape, &value, &self.state.heap);
            original.push(value);
        }
        Ok(match shape {
            ConstListShape::Empty => TypedList::Mixed(original),
            ConstListShape::Int(values) => TypedList::Int(values),
            ConstListShape::Float(values) => TypedList::Float(values),
            ConstListShape::Bool(values) => TypedList::Bool(values),
            ConstListShape::String(values) => TypedList::String(values),
            ConstListShape::Mixed => TypedList::Mixed(original),
        })
    }
}

enum ConstListShape {
    Empty,
    Int(Vec<i64>),
    Float(Vec<f64>),
    Bool(Vec<bool>),
    String(Vec<Arc<str>>),
    Mixed,
}

fn append_const_list_shape(shape: ConstListShape, value: &RuntimeVal, heap: &crate::val::HeapStore) -> ConstListShape {
    match (shape, value) {
        (ConstListShape::Empty, RuntimeVal::Int(value)) => ConstListShape::Int(vec![*value]),
        (ConstListShape::Empty, RuntimeVal::Float(value)) => ConstListShape::Float(vec![*value]),
        (ConstListShape::Empty, RuntimeVal::Bool(value)) => ConstListShape::Bool(vec![*value]),
        (ConstListShape::Empty, RuntimeVal::ShortStr(value)) => {
            ConstListShape::String(vec![Arc::<str>::from(value.as_str())])
        }
        (ConstListShape::Empty, RuntimeVal::Obj(handle)) => match heap.get(*handle) {
            Some(HeapValue::String(value)) => ConstListShape::String(vec![Arc::clone(value)]),
            _ => ConstListShape::Mixed,
        },
        (ConstListShape::Int(mut values), RuntimeVal::Int(value)) => {
            values.push(*value);
            ConstListShape::Int(values)
        }
        (ConstListShape::Float(mut values), RuntimeVal::Float(value)) => {
            values.push(*value);
            ConstListShape::Float(values)
        }
        (ConstListShape::Bool(mut values), RuntimeVal::Bool(value)) => {
            values.push(*value);
            ConstListShape::Bool(values)
        }
        (ConstListShape::String(mut values), RuntimeVal::ShortStr(value)) => {
            values.push(Arc::<str>::from(value.as_str()));
            ConstListShape::String(values)
        }
        (ConstListShape::String(mut values), RuntimeVal::Obj(handle)) => match heap.get(*handle) {
            Some(HeapValue::String(value)) => {
                values.push(Arc::clone(value));
                ConstListShape::String(values)
            }
            _ => ConstListShape::Mixed,
        },
        (ConstListShape::Mixed, _) => ConstListShape::Mixed,
        _ => ConstListShape::Mixed,
    }
}
