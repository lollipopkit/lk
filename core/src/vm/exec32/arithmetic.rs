use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Context, Result, bail};

use crate::val::{HeapValue, RuntimeMapKey, RuntimeVal, TypedList, TypedMap};
use crate::vm::Instr32;

use super::Executor32;

impl Executor32 {
    pub(super) fn dynamic_add(&mut self, instr: Instr32) -> Result<()> {
        let lhs = self.read(instr.b())?.clone();
        let rhs = self.read(instr.c())?.clone();
        let value = match (&lhs, &rhs) {
            (RuntimeVal::Int(lhs), RuntimeVal::Int(rhs)) => RuntimeVal::Int(lhs.wrapping_add(*rhs)),
            (RuntimeVal::Int(lhs), RuntimeVal::Float(rhs)) => RuntimeVal::Float(*lhs as f64 + *rhs),
            (RuntimeVal::Float(lhs), RuntimeVal::Int(rhs)) => RuntimeVal::Float(*lhs + *rhs as f64),
            (RuntimeVal::Float(lhs), RuntimeVal::Float(rhs)) => RuntimeVal::Float(*lhs + *rhs),
            _ if self.runtime_value_is_map(&lhs)? && self.runtime_value_is_map(&rhs)? => {
                let mut entries = self.runtime_value_to_map_entries(&lhs)?.expect("checked map");
                entries.extend(self.runtime_value_to_map_entries(&rhs)?.expect("checked map"));
                RuntimeVal::Obj(
                    self.state
                        .heap
                        .alloc(HeapValue::Map(TypedMap::from_runtime_entries(entries))),
                )
            }
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

    pub(super) fn dynamic_sub(&mut self, instr: Instr32) -> Result<()> {
        let lhs = self.read(instr.b())?.clone();
        let rhs = self.read(instr.c())?.clone();
        let value = match (&lhs, &rhs) {
            (RuntimeVal::Int(lhs), RuntimeVal::Int(rhs)) => RuntimeVal::Int(lhs.wrapping_sub(*rhs)),
            (RuntimeVal::Int(lhs), RuntimeVal::Float(rhs)) => RuntimeVal::Float(*lhs as f64 - *rhs),
            (RuntimeVal::Float(lhs), RuntimeVal::Int(rhs)) => RuntimeVal::Float(*lhs - *rhs as f64),
            (RuntimeVal::Float(lhs), RuntimeVal::Float(rhs)) => RuntimeVal::Float(*lhs - *rhs),
            _ if self.runtime_value_is_heap_list(&lhs)? && self.runtime_value_is_heap_list(&rhs)? => {
                let lhs_values = self.runtime_value_to_list_values(&lhs)?.expect("checked list");
                let rhs_values = self.runtime_value_to_list_values(&rhs)?.expect("checked list");
                let mut values = Vec::with_capacity(lhs_values.len());
                'outer: for lhs_value in lhs_values {
                    for rhs_value in &rhs_values {
                        if self.runtime_values_equal(&lhs_value, rhs_value)? {
                            continue 'outer;
                        }
                    }
                    values.push(lhs_value);
                }
                RuntimeVal::Obj(self.state.heap.alloc(HeapValue::List(TypedList::from_runtime_values(
                    values,
                    &self.state.heap,
                ))))
            }
            _ if self.runtime_value_is_heap_list(&lhs)? => {
                let lhs_values = self.runtime_value_to_list_values(&lhs)?.expect("checked list");
                let mut values = Vec::with_capacity(lhs_values.len());
                let mut removed = false;
                for lhs_value in lhs_values {
                    if !removed && self.runtime_values_equal(&lhs_value, &rhs)? {
                        removed = true;
                        continue;
                    }
                    values.push(lhs_value);
                }
                RuntimeVal::Obj(self.state.heap.alloc(HeapValue::List(TypedList::from_runtime_values(
                    values,
                    &self.state.heap,
                ))))
            }
            _ if self.runtime_value_is_map(&lhs)? && self.runtime_value_is_map(&rhs)? => {
                let mut entries = self.runtime_value_to_map_entries(&lhs)?.expect("checked map");
                for key in self.runtime_value_to_map_entries(&rhs)?.expect("checked map").keys() {
                    remove_runtime_map_key(&mut entries, key);
                }
                RuntimeVal::Obj(
                    self.state
                        .heap
                        .alloc(HeapValue::Map(TypedMap::from_runtime_entries(entries))),
                )
            }
            _ if self.runtime_value_is_map(&lhs)? => {
                let mut entries = self.runtime_value_to_map_entries(&lhs)?.expect("checked map");
                let key = self.runtime_map_key_from_value(&rhs)?;
                remove_runtime_map_key(&mut entries, &key);
                RuntimeVal::Obj(
                    self.state
                        .heap
                        .alloc(HeapValue::Map(TypedMap::from_runtime_entries(entries))),
                )
            }
            _ => bail!(
                "Sub expected numbers or list/map lhs, got {:?} and {:?}",
                lhs.kind(),
                rhs.kind()
            ),
        };
        self.write(instr.a(), value)?;
        self.pc += 1;
        Ok(())
    }

    pub(super) fn dynamic_numeric_binary(
        &mut self,
        instr: Instr32,
        int_op: impl FnOnce(i64, i64) -> i64,
        float_op: impl FnOnce(f64, f64) -> f64,
    ) -> Result<()> {
        let lhs = self
            .read(instr.b())
            .with_context(|| format!("{:?} at pc {} lhs register {}", instr.opcode(), self.pc, instr.b()))?;
        let rhs = self
            .read(instr.c())
            .with_context(|| format!("{:?} at pc {} rhs register {}", instr.opcode(), self.pc, instr.c()))?;
        let value = match (lhs, rhs) {
            (RuntimeVal::Int(lhs), RuntimeVal::Int(rhs)) => RuntimeVal::Int(int_op(*lhs, *rhs)),
            _ => RuntimeVal::Float(float_op(
                self.number_value(lhs)
                    .with_context(|| format!("{:?} at pc {} lhs register {}", instr.opcode(), self.pc, instr.b()))?,
                self.number_value(rhs)
                    .with_context(|| format!("{:?} at pc {} rhs register {}", instr.opcode(), self.pc, instr.c()))?,
            )),
        };
        self.write(instr.a(), value)?;
        self.pc += 1;
        Ok(())
    }

    #[inline]
    pub(super) fn float_binary(&mut self, instr: Instr32, op: impl FnOnce(f64, f64) -> f64) -> Result<()> {
        let lhs = self.read_number(instr.b())?;
        let rhs = self.read_number(instr.c())?;
        self.write(instr.a(), RuntimeVal::Float(op(lhs, rhs)))?;
        self.pc += 1;
        Ok(())
    }

    #[inline]
    pub(super) fn int_compare(&mut self, instr: Instr32, op: impl FnOnce(f64, f64) -> bool) -> Result<()> {
        let lhs = self.read_number(instr.b())?;
        let rhs = self.read_number(instr.c())?;
        self.write(instr.a(), RuntimeVal::Bool(op(lhs, rhs)))?;
        self.pc += 1;
        Ok(())
    }

    pub(super) fn values_equal(&self, lhs: u8, rhs: u8) -> Result<bool> {
        let lhs = self.read(lhs)?.clone();
        let rhs = self.read(rhs)?.clone();
        self.runtime_values_equal(&lhs, &rhs)
    }

    fn runtime_values_equal(&self, lhs: &RuntimeVal, rhs: &RuntimeVal) -> Result<bool> {
        Ok(match (lhs, rhs) {
            (RuntimeVal::Nil, RuntimeVal::Nil) => true,
            (RuntimeVal::Bool(lhs), RuntimeVal::Bool(rhs)) => lhs == rhs,
            (RuntimeVal::Int(lhs), RuntimeVal::Int(rhs)) => lhs == rhs,
            (RuntimeVal::Float(lhs), RuntimeVal::Float(rhs)) => lhs == rhs,
            (RuntimeVal::Int(lhs), RuntimeVal::Float(rhs)) => *lhs as f64 == *rhs,
            (RuntimeVal::Float(lhs), RuntimeVal::Int(rhs)) => *lhs == *rhs as f64,
            (RuntimeVal::Obj(lhs), RuntimeVal::Obj(rhs)) if lhs == rhs => true,
            (RuntimeVal::Obj(lhs), RuntimeVal::Obj(rhs)) => {
                let lhs = self
                    .state
                    .heap
                    .get(*lhs)
                    .ok_or_else(|| anyhow::anyhow!("heap object {} out of bounds", lhs.index()))?;
                let rhs = self
                    .state
                    .heap
                    .get(*rhs)
                    .ok_or_else(|| anyhow::anyhow!("heap object {} out of bounds", rhs.index()))?;
                self.heap_values_equal(lhs, rhs)?
            }
            _ => match (self.runtime_value_to_string(&lhs)?, self.runtime_value_to_string(&rhs)?) {
                (Some(lhs), Some(rhs)) => lhs == rhs,
                _ => false,
            },
        })
    }

    fn heap_values_equal(&self, lhs: &HeapValue, rhs: &HeapValue) -> Result<bool> {
        Ok(match (lhs, rhs) {
            (HeapValue::String(lhs), HeapValue::String(rhs)) => lhs == rhs,
            (HeapValue::List(lhs), HeapValue::List(rhs)) => self.typed_lists_equal(lhs, rhs)?,
            (HeapValue::Map(lhs), HeapValue::Map(rhs)) => self.typed_maps_equal(lhs, rhs)?,
            _ => false,
        })
    }

    fn typed_lists_equal(&self, lhs: &TypedList, rhs: &TypedList) -> Result<bool> {
        if lhs.len() != rhs.len() {
            return Ok(false);
        }
        match (lhs, rhs) {
            (TypedList::Int(lhs), TypedList::Int(rhs)) => return Ok(lhs == rhs),
            (TypedList::Float(lhs), TypedList::Float(rhs)) => return Ok(lhs == rhs),
            (TypedList::Bool(lhs), TypedList::Bool(rhs)) => return Ok(lhs == rhs),
            (TypedList::String(lhs), TypedList::String(rhs)) => return Ok(lhs == rhs),
            _ => {}
        }
        for index in 0..lhs.len() {
            if !self.typed_list_items_equal(lhs, index, rhs, index)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn typed_list_items_equal(
        &self,
        lhs: &TypedList,
        lhs_index: usize,
        rhs: &TypedList,
        rhs_index: usize,
    ) -> Result<bool> {
        match (lhs, rhs) {
            (TypedList::Mixed(lhs), TypedList::Mixed(rhs)) => {
                self.runtime_values_equal(&lhs[lhs_index], &rhs[rhs_index])
            }
            (TypedList::Mixed(lhs), TypedList::String(rhs)) => {
                self.runtime_value_equals_string(&lhs[lhs_index], &rhs[rhs_index])
            }
            (TypedList::String(lhs), TypedList::Mixed(rhs)) => {
                self.runtime_value_equals_string(&rhs[rhs_index], &lhs[lhs_index])
            }
            (TypedList::Int(lhs), _) => {
                self.typed_list_runtime_item_equal(RuntimeVal::Int(lhs[lhs_index]), rhs, rhs_index)
            }
            (TypedList::Float(lhs), _) => {
                self.typed_list_runtime_item_equal(RuntimeVal::Float(lhs[lhs_index]), rhs, rhs_index)
            }
            (TypedList::Bool(lhs), _) => {
                self.typed_list_runtime_item_equal(RuntimeVal::Bool(lhs[lhs_index]), rhs, rhs_index)
            }
            (TypedList::String(lhs), _) => self.typed_list_string_item_equal(&lhs[lhs_index], rhs, rhs_index),
            (TypedList::Mixed(lhs), _) => self.typed_list_runtime_item_equal(lhs[lhs_index].clone(), rhs, rhs_index),
        }
    }

    fn typed_list_runtime_item_equal(&self, lhs: RuntimeVal, rhs: &TypedList, rhs_index: usize) -> Result<bool> {
        match rhs {
            TypedList::Mixed(rhs) => self.runtime_values_equal(&lhs, &rhs[rhs_index]),
            TypedList::Int(rhs) => self.runtime_values_equal(&lhs, &RuntimeVal::Int(rhs[rhs_index])),
            TypedList::Float(rhs) => self.runtime_values_equal(&lhs, &RuntimeVal::Float(rhs[rhs_index])),
            TypedList::Bool(rhs) => self.runtime_values_equal(&lhs, &RuntimeVal::Bool(rhs[rhs_index])),
            TypedList::String(rhs) => self.runtime_value_equals_string(&lhs, &rhs[rhs_index]),
        }
    }

    fn typed_list_string_item_equal(&self, lhs: &Arc<str>, rhs: &TypedList, rhs_index: usize) -> Result<bool> {
        match rhs {
            TypedList::Mixed(rhs) => self.runtime_value_equals_string(&rhs[rhs_index], lhs),
            TypedList::String(rhs) => Ok(lhs == &rhs[rhs_index]),
            _ => Ok(false),
        }
    }

    fn runtime_value_equals_string(&self, value: &RuntimeVal, expected: &str) -> Result<bool> {
        Ok(match value {
            RuntimeVal::ShortStr(value) => value.as_str() == expected,
            RuntimeVal::Obj(handle) => matches!(
                self.state
                    .heap
                    .get(*handle)
                    .ok_or_else(|| anyhow::anyhow!("heap object {} out of bounds", handle.index()))?,
                HeapValue::String(value) if value.as_ref() == expected
            ),
            _ => false,
        })
    }

    fn typed_maps_equal(&self, lhs: &TypedMap, rhs: &TypedMap) -> Result<bool> {
        if lhs.len() != rhs.len() {
            return Ok(false);
        }
        let rhs_entries = rhs.entries().into_iter().collect::<BTreeMap<_, _>>();
        for (key, lhs_value) in lhs.entries() {
            let Some(rhs_value) = rhs_entries.get(&key) else {
                return Ok(false);
            };
            if !self.runtime_values_equal(&lhs_value, rhs_value)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn runtime_value_to_map_entries(&self, value: &RuntimeVal) -> Result<Option<BTreeMap<RuntimeMapKey, RuntimeVal>>> {
        let RuntimeVal::Obj(handle) = value else {
            return Ok(None);
        };
        let Some(HeapValue::Map(map)) = self.state.heap.get(*handle) else {
            return Ok(None);
        };
        Ok(Some(map.entries().into_iter().collect()))
    }
}

fn remove_runtime_map_key(entries: &mut BTreeMap<RuntimeMapKey, RuntimeVal>, key: &RuntimeMapKey) {
    entries.remove(key);
    let Some(key) = key.as_arc_str() else {
        return;
    };
    entries.remove(&RuntimeMapKey::String(key.clone()));
    if let Some(short) = crate::val::ShortStr::new(key.as_ref()) {
        entries.remove(&RuntimeMapKey::ShortStr(short));
    }
}
