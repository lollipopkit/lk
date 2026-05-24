use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::val::{HeapStore, HeapValue, RuntimeMapKey, RuntimeVal, ShortStr, TypedList, TypedMap};
use crate::vm::Instr32;

use super::Executor32;

enum RuntimeListSnapshot {
    Mixed(Vec<RuntimeVal>),
    Int(Vec<i64>),
    Float(Vec<f64>),
    Bool(Vec<bool>),
    String(Vec<Arc<str>>),
}

impl RuntimeListSnapshot {
    fn from_typed(list: &TypedList) -> Self {
        match list {
            TypedList::Mixed(values) => Self::Mixed(copy_slice(values)),
            TypedList::Int(values) => Self::Int(copy_slice(values)),
            TypedList::Float(values) => Self::Float(copy_slice(values)),
            TypedList::Bool(values) => Self::Bool(copy_slice(values)),
            TypedList::String(values) => Self::String(copy_slice(values)),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Mixed(values) => values.len(),
            Self::Int(values) => values.len(),
            Self::Float(values) => values.len(),
            Self::Bool(values) => values.len(),
            Self::String(values) => values.len(),
        }
    }

    fn append_to_mixed_output(self, out: &mut Vec<RuntimeVal>, heap: &mut HeapStore) {
        match self {
            Self::Mixed(values) => out.extend(values),
            Self::Int(values) => out.extend(values.into_iter().map(RuntimeVal::Int)),
            Self::Float(values) => out.extend(values.into_iter().map(RuntimeVal::Float)),
            Self::Bool(values) => out.extend(values.into_iter().map(RuntimeVal::Bool)),
            Self::String(values) => append_string_list_to_mixed_output(values, out, heap),
        }
    }
}

enum ListAddOperand {
    List(RuntimeListSnapshot),
    Value(RuntimeVal),
}

fn append_string_list_to_mixed_output(values: Vec<Arc<str>>, out: &mut Vec<RuntimeVal>, heap: &mut HeapStore) {
    out.extend(values.into_iter().map(|value| match ShortStr::new(&value) {
        Some(short) => RuntimeVal::ShortStr(short),
        None => RuntimeVal::Obj(heap.alloc(HeapValue::String(value))),
    }));
}

fn copy_concat_owned<T>(left: Vec<T>, right: Vec<T>) -> Vec<T> {
    let mut out = Vec::with_capacity(left.len() + right.len());
    out.extend(left);
    out.extend(right);
    out
}

fn copy_slice<T: Clone>(values: &[T]) -> Vec<T> {
    let mut out = Vec::with_capacity(values.len());
    out.extend_from_slice(values);
    out
}

fn copy_with_extra_owned<T>(values: Vec<T>, value: T) -> Vec<T> {
    let mut out = Vec::with_capacity(values.len() + 1);
    out.extend(values);
    out.push(value);
    out
}

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
                let lhs = self.runtime_value_to_typed_map(&lhs)?.expect("checked map");
                let rhs = self.runtime_value_to_typed_map(&rhs)?.expect("checked map");
                let map = merge_typed_maps(lhs, rhs);
                RuntimeVal::Obj(self.state.heap.alloc(HeapValue::Map(map)))
            }
            _ if self.runtime_value_is_heap_list(&lhs)? || self.runtime_value_is_heap_list(&rhs)? => {
                let lhs = self.runtime_value_to_list_add_operand(&lhs)?;
                let rhs = self.runtime_value_to_list_add_operand(&rhs)?;
                let list = self.add_list_values(lhs, rhs)?;
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
                let lhs_values = self.runtime_value_to_list_snapshot(&lhs)?.expect("checked list");
                let rhs_values = self.runtime_value_to_list_snapshot(&rhs)?.expect("checked list");
                let values = self.remove_list_values(lhs_values, &rhs_values)?;
                RuntimeVal::Obj(self.state.heap.alloc(HeapValue::List(values)))
            }
            _ if self.runtime_value_is_heap_list(&lhs)? => {
                let lhs_values = self.runtime_value_to_list_snapshot(&lhs)?.expect("checked list");
                let values = self.remove_first_list_value(lhs_values, &rhs)?;
                RuntimeVal::Obj(self.state.heap.alloc(HeapValue::List(values)))
            }
            _ if self.runtime_value_is_map(&lhs)? && self.runtime_value_is_map(&rhs)? => {
                let lhs = self.runtime_value_to_typed_map(&lhs)?.expect("checked map");
                let rhs = self.runtime_value_to_typed_map(&rhs)?.expect("checked map");
                let map = remove_typed_map_keys(lhs, rhs);
                RuntimeVal::Obj(self.state.heap.alloc(HeapValue::Map(map)))
            }
            _ if self.runtime_value_is_map(&lhs)? => {
                let key = self.runtime_map_key_from_value(&rhs)?;
                let lhs = self.runtime_value_to_typed_map(&lhs)?.expect("checked map");
                let map = typed_map_without_key(lhs, &key);
                RuntimeVal::Obj(self.state.heap.alloc(HeapValue::Map(map)))
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

    fn runtime_value_to_list_snapshot(&self, value: &RuntimeVal) -> Result<Option<RuntimeListSnapshot>> {
        let RuntimeVal::Obj(handle) = value else {
            return Ok(None);
        };
        let Some(HeapValue::List(list)) = self.state.heap.get(*handle) else {
            return Ok(None);
        };
        Ok(Some(RuntimeListSnapshot::from_typed(list)))
    }

    fn runtime_value_to_list_add_operand(&self, value: &RuntimeVal) -> Result<ListAddOperand> {
        let RuntimeVal::Obj(handle) = value else {
            return Ok(ListAddOperand::Value(value.clone()));
        };
        let Some(value) = self.state.heap.get(*handle) else {
            bail!("heap object {} out of bounds", handle.index());
        };
        match value {
            HeapValue::List(list) => Ok(ListAddOperand::List(RuntimeListSnapshot::from_typed(list))),
            _ => Ok(ListAddOperand::Value(RuntimeVal::Obj(*handle))),
        }
    }

    fn add_list_values(&mut self, lhs: ListAddOperand, rhs: ListAddOperand) -> Result<TypedList> {
        Ok(match (lhs, rhs) {
            (ListAddOperand::List(lhs), ListAddOperand::List(rhs)) => self.concat_list_snapshots(lhs, rhs)?,
            (ListAddOperand::List(lhs), ListAddOperand::Value(rhs)) => self.push_list_snapshot(lhs, rhs)?,
            (ListAddOperand::Value(lhs), ListAddOperand::List(rhs)) => self.prepend_list_snapshot(lhs, rhs)?,
            (ListAddOperand::Value(_), ListAddOperand::Value(_)) => {
                unreachable!("list add branch requires at least one list")
            }
        })
    }

    fn concat_list_snapshots(&mut self, lhs: RuntimeListSnapshot, rhs: RuntimeListSnapshot) -> Result<TypedList> {
        Ok(match (lhs, rhs) {
            (RuntimeListSnapshot::Int(lhs), RuntimeListSnapshot::Int(rhs)) => {
                TypedList::Int(copy_concat_owned(lhs, rhs))
            }
            (RuntimeListSnapshot::Float(lhs), RuntimeListSnapshot::Float(rhs)) => {
                TypedList::Float(copy_concat_owned(lhs, rhs))
            }
            (RuntimeListSnapshot::Bool(lhs), RuntimeListSnapshot::Bool(rhs)) => {
                TypedList::Bool(copy_concat_owned(lhs, rhs))
            }
            (RuntimeListSnapshot::String(lhs), RuntimeListSnapshot::String(rhs)) => {
                TypedList::String(copy_concat_owned(lhs, rhs))
            }
            (lhs, rhs) => {
                let mut values = Vec::with_capacity(lhs.len() + rhs.len());
                lhs.append_to_mixed_output(&mut values, &mut self.state.heap);
                rhs.append_to_mixed_output(&mut values, &mut self.state.heap);
                TypedList::Mixed(values)
            }
        })
    }

    fn push_list_snapshot(&mut self, list: RuntimeListSnapshot, value: RuntimeVal) -> Result<TypedList> {
        Ok(match list {
            RuntimeListSnapshot::Int(values) => match value {
                RuntimeVal::Int(value) => TypedList::Int(copy_with_extra_owned(values, value)),
                value => {
                    let mut mixed = Vec::with_capacity(values.len() + 1);
                    mixed.extend(values.into_iter().map(RuntimeVal::Int));
                    mixed.push(value);
                    TypedList::Mixed(mixed)
                }
            },
            RuntimeListSnapshot::Float(values) => match value {
                RuntimeVal::Float(value) => TypedList::Float(copy_with_extra_owned(values, value)),
                value => {
                    let mut mixed = Vec::with_capacity(values.len() + 1);
                    mixed.extend(values.into_iter().map(RuntimeVal::Float));
                    mixed.push(value);
                    TypedList::Mixed(mixed)
                }
            },
            RuntimeListSnapshot::Bool(values) => match value {
                RuntimeVal::Bool(value) => TypedList::Bool(copy_with_extra_owned(values, value)),
                value => {
                    let mut mixed = Vec::with_capacity(values.len() + 1);
                    mixed.extend(values.into_iter().map(RuntimeVal::Bool));
                    mixed.push(value);
                    TypedList::Mixed(mixed)
                }
            },
            RuntimeListSnapshot::String(values) => match self.runtime_string_value(&value)? {
                Some(value) => TypedList::String(copy_with_extra_owned(values, value)),
                None => {
                    let mut mixed = Vec::with_capacity(values.len() + 1);
                    append_string_list_to_mixed_output(values, &mut mixed, &mut self.state.heap);
                    mixed.push(value);
                    TypedList::Mixed(mixed)
                }
            },
            RuntimeListSnapshot::Mixed(values) => TypedList::Mixed(copy_with_extra_owned(values, value)),
        })
    }

    fn prepend_list_snapshot(&mut self, value: RuntimeVal, list: RuntimeListSnapshot) -> Result<TypedList> {
        Ok(match list {
            RuntimeListSnapshot::Int(values) => match value {
                RuntimeVal::Int(value) => {
                    let mut out = Vec::with_capacity(values.len() + 1);
                    out.push(value);
                    out.extend(values);
                    TypedList::Int(out)
                }
                value => {
                    let mut mixed = Vec::with_capacity(values.len() + 1);
                    mixed.push(value);
                    mixed.extend(values.into_iter().map(RuntimeVal::Int));
                    TypedList::Mixed(mixed)
                }
            },
            RuntimeListSnapshot::Float(values) => match value {
                RuntimeVal::Float(value) => {
                    let mut out = Vec::with_capacity(values.len() + 1);
                    out.push(value);
                    out.extend(values);
                    TypedList::Float(out)
                }
                value => {
                    let mut mixed = Vec::with_capacity(values.len() + 1);
                    mixed.push(value);
                    mixed.extend(values.into_iter().map(RuntimeVal::Float));
                    TypedList::Mixed(mixed)
                }
            },
            RuntimeListSnapshot::Bool(values) => match value {
                RuntimeVal::Bool(value) => {
                    let mut out = Vec::with_capacity(values.len() + 1);
                    out.push(value);
                    out.extend(values);
                    TypedList::Bool(out)
                }
                value => {
                    let mut mixed = Vec::with_capacity(values.len() + 1);
                    mixed.push(value);
                    mixed.extend(values.into_iter().map(RuntimeVal::Bool));
                    TypedList::Mixed(mixed)
                }
            },
            RuntimeListSnapshot::String(values) => match self.runtime_string_value(&value)? {
                Some(value) => {
                    let mut out = Vec::with_capacity(values.len() + 1);
                    out.push(value);
                    out.extend(values);
                    TypedList::String(out)
                }
                None => {
                    let mut mixed = Vec::with_capacity(values.len() + 1);
                    mixed.push(value);
                    append_string_list_to_mixed_output(values, &mut mixed, &mut self.state.heap);
                    TypedList::Mixed(mixed)
                }
            },
            RuntimeListSnapshot::Mixed(values) => {
                let mut mixed = Vec::with_capacity(values.len() + 1);
                mixed.push(value);
                mixed.extend(values);
                TypedList::Mixed(mixed)
            }
        })
    }

    fn remove_list_values(&mut self, lhs: RuntimeListSnapshot, rhs: &RuntimeListSnapshot) -> Result<TypedList> {
        Ok(match (lhs, rhs) {
            (RuntimeListSnapshot::Int(lhs), RuntimeListSnapshot::Int(rhs)) => {
                let mut out = Vec::with_capacity(lhs.len());
                for value in lhs {
                    if !rhs.contains(&value) {
                        out.push(value);
                    }
                }
                TypedList::Int(out)
            }
            (RuntimeListSnapshot::Float(lhs), RuntimeListSnapshot::Float(rhs)) => {
                let mut out = Vec::with_capacity(lhs.len());
                for value in lhs {
                    if !rhs.contains(&value) {
                        out.push(value);
                    }
                }
                TypedList::Float(out)
            }
            (RuntimeListSnapshot::Bool(lhs), RuntimeListSnapshot::Bool(rhs)) => {
                let mut out = Vec::with_capacity(lhs.len());
                for value in lhs {
                    if !rhs.contains(&value) {
                        out.push(value);
                    }
                }
                TypedList::Bool(out)
            }
            (RuntimeListSnapshot::String(lhs), RuntimeListSnapshot::String(rhs)) => {
                let mut out = Vec::with_capacity(lhs.len());
                for value in lhs {
                    if !rhs.iter().any(|rhs| rhs.as_ref() == value.as_ref()) {
                        out.push(value);
                    }
                }
                TypedList::String(out)
            }
            (lhs, rhs) => self.remove_list_values_preserving_lhs_backing(lhs, rhs)?,
        })
    }

    fn remove_first_list_value(&mut self, lhs: RuntimeListSnapshot, rhs: &RuntimeVal) -> Result<TypedList> {
        Ok(match lhs {
            RuntimeListSnapshot::Int(values) => match rhs {
                RuntimeVal::Int(rhs) => {
                    let mut removed = false;
                    let mut out = Vec::with_capacity(values.len());
                    for value in values {
                        if !removed && value == *rhs {
                            removed = true;
                        } else {
                            out.push(value);
                        }
                    }
                    TypedList::Int(out)
                }
                rhs => self.remove_first_runtime_value(RuntimeListSnapshot::Int(values), rhs)?,
            },
            RuntimeListSnapshot::Float(values) => match rhs {
                RuntimeVal::Float(rhs) => {
                    let mut removed = false;
                    let mut out = Vec::with_capacity(values.len());
                    for value in values {
                        if !removed && value == *rhs {
                            removed = true;
                        } else {
                            out.push(value);
                        }
                    }
                    TypedList::Float(out)
                }
                rhs => self.remove_first_runtime_value(RuntimeListSnapshot::Float(values), rhs)?,
            },
            RuntimeListSnapshot::Bool(values) => match rhs {
                RuntimeVal::Bool(rhs) => {
                    let mut removed = false;
                    let mut out = Vec::with_capacity(values.len());
                    for value in values {
                        if !removed && value == *rhs {
                            removed = true;
                        } else {
                            out.push(value);
                        }
                    }
                    TypedList::Bool(out)
                }
                rhs => self.remove_first_runtime_value(RuntimeListSnapshot::Bool(values), rhs)?,
            },
            RuntimeListSnapshot::String(values) => match self.runtime_string_value(rhs)? {
                Some(rhs) => {
                    let mut removed = false;
                    let mut out = Vec::with_capacity(values.len());
                    for value in values {
                        if !removed && value.as_ref() == rhs.as_ref() {
                            removed = true;
                        } else {
                            out.push(value);
                        }
                    }
                    TypedList::String(out)
                }
                None => TypedList::String(values),
            },
            RuntimeListSnapshot::Mixed(values) => {
                self.remove_first_runtime_value(RuntimeListSnapshot::Mixed(values), rhs)?
            }
        })
    }

    fn remove_first_runtime_value(&mut self, lhs: RuntimeListSnapshot, rhs: &RuntimeVal) -> Result<TypedList> {
        let mut removed = false;
        match lhs {
            RuntimeListSnapshot::Mixed(values) => {
                let mut out = Vec::with_capacity(values.len());
                for value in values {
                    if !removed && self.runtime_values_equal(&value, rhs)? {
                        removed = true;
                    } else {
                        out.push(value);
                    }
                }
                Ok(TypedList::Mixed(out))
            }
            RuntimeListSnapshot::Int(values) => {
                let mut out = Vec::with_capacity(values.len());
                for value in values {
                    if !removed && self.runtime_values_equal(&RuntimeVal::Int(value), rhs)? {
                        removed = true;
                    } else {
                        out.push(value);
                    }
                }
                Ok(TypedList::Int(out))
            }
            RuntimeListSnapshot::Float(values) => {
                let mut out = Vec::with_capacity(values.len());
                for value in values {
                    if !removed && self.runtime_values_equal(&RuntimeVal::Float(value), rhs)? {
                        removed = true;
                    } else {
                        out.push(value);
                    }
                }
                Ok(TypedList::Float(out))
            }
            RuntimeListSnapshot::Bool(values) => {
                let mut out = Vec::with_capacity(values.len());
                for value in values {
                    if !removed && self.runtime_values_equal(&RuntimeVal::Bool(value), rhs)? {
                        removed = true;
                    } else {
                        out.push(value);
                    }
                }
                Ok(TypedList::Bool(out))
            }
            RuntimeListSnapshot::String(values) => {
                let Some(rhs) = self.runtime_string_value(rhs)? else {
                    return Ok(TypedList::String(values));
                };
                let mut out = Vec::with_capacity(values.len());
                for value in values {
                    if !removed && value.as_ref() == rhs.as_ref() {
                        removed = true;
                    } else {
                        out.push(value);
                    }
                }
                Ok(TypedList::String(out))
            }
        }
    }

    fn remove_list_values_preserving_lhs_backing(
        &self,
        lhs: RuntimeListSnapshot,
        rhs: &RuntimeListSnapshot,
    ) -> Result<TypedList> {
        Ok(match lhs {
            RuntimeListSnapshot::Mixed(values) => {
                let mut out = Vec::with_capacity(values.len());
                'outer_mixed: for value in values {
                    for rhs_index in 0..rhs.len() {
                        if self.list_snapshot_runtime_item_equal(value.clone(), rhs, rhs_index)? {
                            continue 'outer_mixed;
                        }
                    }
                    out.push(value);
                }
                TypedList::Mixed(out)
            }
            RuntimeListSnapshot::Int(values) => {
                let mut out = Vec::with_capacity(values.len());
                'outer_int: for value in values {
                    for rhs_index in 0..rhs.len() {
                        if self.list_snapshot_runtime_item_equal(RuntimeVal::Int(value), rhs, rhs_index)? {
                            continue 'outer_int;
                        }
                    }
                    out.push(value);
                }
                TypedList::Int(out)
            }
            RuntimeListSnapshot::Float(values) => {
                let mut out = Vec::with_capacity(values.len());
                'outer_float: for value in values {
                    for rhs_index in 0..rhs.len() {
                        if self.list_snapshot_runtime_item_equal(RuntimeVal::Float(value), rhs, rhs_index)? {
                            continue 'outer_float;
                        }
                    }
                    out.push(value);
                }
                TypedList::Float(out)
            }
            RuntimeListSnapshot::Bool(values) => {
                let mut out = Vec::with_capacity(values.len());
                'outer_bool: for value in values {
                    for rhs_index in 0..rhs.len() {
                        if self.list_snapshot_runtime_item_equal(RuntimeVal::Bool(value), rhs, rhs_index)? {
                            continue 'outer_bool;
                        }
                    }
                    out.push(value);
                }
                TypedList::Bool(out)
            }
            RuntimeListSnapshot::String(values) => {
                let mut out = Vec::with_capacity(values.len());
                'outer_string: for value in values {
                    for rhs_index in 0..rhs.len() {
                        if self.list_snapshot_string_item_equal(&value, rhs, rhs_index)? {
                            continue 'outer_string;
                        }
                    }
                    out.push(value);
                }
                TypedList::String(out)
            }
        })
    }

    fn list_snapshot_runtime_item_equal(
        &self,
        lhs: RuntimeVal,
        rhs: &RuntimeListSnapshot,
        rhs_index: usize,
    ) -> Result<bool> {
        match rhs {
            RuntimeListSnapshot::Mixed(rhs) => self.runtime_values_equal(&lhs, &rhs[rhs_index]),
            RuntimeListSnapshot::Int(rhs) => self.runtime_values_equal(&lhs, &RuntimeVal::Int(rhs[rhs_index])),
            RuntimeListSnapshot::Float(rhs) => self.runtime_values_equal(&lhs, &RuntimeVal::Float(rhs[rhs_index])),
            RuntimeListSnapshot::Bool(rhs) => self.runtime_values_equal(&lhs, &RuntimeVal::Bool(rhs[rhs_index])),
            RuntimeListSnapshot::String(rhs) => self.runtime_value_equals_string(&lhs, &rhs[rhs_index]),
        }
    }

    fn list_snapshot_string_item_equal(
        &self,
        lhs: &Arc<str>,
        rhs: &RuntimeListSnapshot,
        rhs_index: usize,
    ) -> Result<bool> {
        match rhs {
            RuntimeListSnapshot::Mixed(rhs) => self.runtime_value_equals_string(&rhs[rhs_index], lhs),
            RuntimeListSnapshot::String(rhs) => Ok(lhs == &rhs[rhs_index]),
            _ => Ok(false),
        }
    }

    fn runtime_string_value(&self, value: &RuntimeVal) -> Result<Option<Arc<str>>> {
        match value {
            RuntimeVal::ShortStr(value) => Ok(Some(Arc::<str>::from(value.as_str()))),
            RuntimeVal::Obj(handle) => match self
                .state
                .heap
                .get(*handle)
                .ok_or_else(|| anyhow::anyhow!("heap object {} out of bounds", handle.index()))?
            {
                HeapValue::String(value) => Ok(Some(value.clone())),
                _ => Ok(None),
            },
            _ => Ok(None),
        }
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
        match lhs {
            TypedMap::Mixed(entries) => {
                for (key, value) in entries {
                    if !self.typed_map_value_equal(rhs, key, value)? {
                        return Ok(false);
                    }
                }
            }
            TypedMap::StringMixed(entries) => {
                for (key, value) in entries {
                    let key = RuntimeMapKey::String(key.clone());
                    if !self.typed_map_value_equal(rhs, &key, value)? {
                        return Ok(false);
                    }
                }
            }
            TypedMap::StringInt(entries) => {
                for (key, value) in entries {
                    let key = RuntimeMapKey::String(key.clone());
                    if !self.typed_map_value_equal(rhs, &key, &RuntimeVal::Int(*value))? {
                        return Ok(false);
                    }
                }
            }
            TypedMap::StringFloat(entries) => {
                for (key, value) in entries {
                    let key = RuntimeMapKey::String(key.clone());
                    if !self.typed_map_value_equal(rhs, &key, &RuntimeVal::Float(*value))? {
                        return Ok(false);
                    }
                }
            }
            TypedMap::StringBool(entries) => {
                for (key, value) in entries {
                    let key = RuntimeMapKey::String(key.clone());
                    if !self.typed_map_value_equal(rhs, &key, &RuntimeVal::Bool(*value))? {
                        return Ok(false);
                    }
                }
            }
        }
        Ok(true)
    }

    fn typed_map_value_equal(&self, rhs: &TypedMap, key: &RuntimeMapKey, lhs_value: &RuntimeVal) -> Result<bool> {
        let Some(rhs_value) = rhs.get(key) else {
            return Ok(false);
        };
        self.runtime_values_equal(lhs_value, &rhs_value)
    }

    fn runtime_value_to_typed_map(&self, value: &RuntimeVal) -> Result<Option<&TypedMap>> {
        let RuntimeVal::Obj(handle) = value else {
            return Ok(None);
        };
        let Some(HeapValue::Map(map)) = self.state.heap.get(*handle) else {
            return Ok(None);
        };
        Ok(Some(map))
    }
}

fn merge_typed_maps(lhs: &TypedMap, rhs: &TypedMap) -> TypedMap {
    let mut replaced_keys = Vec::with_capacity(rhs.len());
    for_each_typed_map_key(rhs, |key| replaced_keys.push(key));
    let mut out = typed_map_without_merge_keys(lhs, &replaced_keys);
    for_each_typed_map_entry(rhs, |key, value| out.set(key, value));
    out
}

fn remove_typed_map_keys(lhs: &TypedMap, rhs: &TypedMap) -> TypedMap {
    let mut removed_keys = Vec::with_capacity(rhs.len());
    for_each_typed_map_key(rhs, |key| removed_keys.push(key));
    typed_map_without_keys(lhs, &removed_keys)
}

fn for_each_typed_map_entry(map: &TypedMap, mut visit: impl FnMut(RuntimeMapKey, RuntimeVal)) {
    match map {
        TypedMap::Mixed(entries) => {
            for (key, value) in entries {
                visit(key.clone(), value.clone());
            }
        }
        TypedMap::StringMixed(entries) => {
            for (key, value) in entries {
                visit(RuntimeMapKey::String(key.clone()), value.clone());
            }
        }
        TypedMap::StringInt(entries) => {
            for (key, value) in entries {
                visit(RuntimeMapKey::String(key.clone()), RuntimeVal::Int(*value));
            }
        }
        TypedMap::StringFloat(entries) => {
            for (key, value) in entries {
                visit(RuntimeMapKey::String(key.clone()), RuntimeVal::Float(*value));
            }
        }
        TypedMap::StringBool(entries) => {
            for (key, value) in entries {
                visit(RuntimeMapKey::String(key.clone()), RuntimeVal::Bool(*value));
            }
        }
    }
}

fn for_each_typed_map_key(map: &TypedMap, mut visit: impl FnMut(RuntimeMapKey)) {
    match map {
        TypedMap::Mixed(entries) => {
            for key in entries.keys() {
                visit(key.clone());
            }
        }
        TypedMap::StringMixed(entries) => {
            for key in entries.keys() {
                visit(RuntimeMapKey::String(key.clone()));
            }
        }
        TypedMap::StringInt(entries) => {
            for key in entries.keys() {
                visit(RuntimeMapKey::String(key.clone()));
            }
        }
        TypedMap::StringFloat(entries) => {
            for key in entries.keys() {
                visit(RuntimeMapKey::String(key.clone()));
            }
        }
        TypedMap::StringBool(entries) => {
            for key in entries.keys() {
                visit(RuntimeMapKey::String(key.clone()));
            }
        }
    }
}

fn typed_map_without_key(map: &TypedMap, removed_key: &RuntimeMapKey) -> TypedMap {
    typed_map_without_keys(map, std::slice::from_ref(removed_key))
}

fn typed_map_without_merge_keys(map: &TypedMap, replaced_keys: &[RuntimeMapKey]) -> TypedMap {
    match map {
        TypedMap::Mixed(entries) => {
            let mut out = BTreeMap::new();
            for (key, value) in entries {
                if !replaced_keys.contains(key) {
                    out.insert(key.clone(), value.clone());
                }
            }
            TypedMap::Mixed(out)
        }
        TypedMap::StringMixed(entries) => {
            let mut out = BTreeMap::new();
            for (key, value) in entries {
                if !string_map_key_removed(key, replaced_keys) {
                    out.insert(key.clone(), value.clone());
                }
            }
            TypedMap::StringMixed(out)
        }
        TypedMap::StringInt(entries) => {
            let mut out = BTreeMap::new();
            for (key, value) in entries {
                if !string_map_key_removed(key, replaced_keys) {
                    out.insert(key.clone(), *value);
                }
            }
            TypedMap::StringInt(out)
        }
        TypedMap::StringFloat(entries) => {
            let mut out = BTreeMap::new();
            for (key, value) in entries {
                if !string_map_key_removed(key, replaced_keys) {
                    out.insert(key.clone(), *value);
                }
            }
            TypedMap::StringFloat(out)
        }
        TypedMap::StringBool(entries) => {
            let mut out = BTreeMap::new();
            for (key, value) in entries {
                if !string_map_key_removed(key, replaced_keys) {
                    out.insert(key.clone(), *value);
                }
            }
            TypedMap::StringBool(out)
        }
    }
}

fn typed_map_without_keys(map: &TypedMap, removed_keys: &[RuntimeMapKey]) -> TypedMap {
    match map {
        TypedMap::Mixed(entries) => {
            let mut out = BTreeMap::new();
            for (key, value) in entries {
                if !runtime_map_key_removed(key, removed_keys) {
                    out.insert(key.clone(), value.clone());
                }
            }
            TypedMap::Mixed(out)
        }
        TypedMap::StringMixed(entries) => {
            let mut out = BTreeMap::new();
            for (key, value) in entries {
                if !string_map_key_removed(key, removed_keys) {
                    out.insert(key.clone(), value.clone());
                }
            }
            TypedMap::StringMixed(out)
        }
        TypedMap::StringInt(entries) => {
            let mut out = BTreeMap::new();
            for (key, value) in entries {
                if !string_map_key_removed(key, removed_keys) {
                    out.insert(key.clone(), *value);
                }
            }
            TypedMap::StringInt(out)
        }
        TypedMap::StringFloat(entries) => {
            let mut out = BTreeMap::new();
            for (key, value) in entries {
                if !string_map_key_removed(key, removed_keys) {
                    out.insert(key.clone(), *value);
                }
            }
            TypedMap::StringFloat(out)
        }
        TypedMap::StringBool(entries) => {
            let mut out = BTreeMap::new();
            for (key, value) in entries {
                if !string_map_key_removed(key, removed_keys) {
                    out.insert(key.clone(), *value);
                }
            }
            TypedMap::StringBool(out)
        }
    }
}

fn runtime_map_key_removed(key: &RuntimeMapKey, removed_keys: &[RuntimeMapKey]) -> bool {
    removed_keys.iter().any(|removed| runtime_map_keys_match(key, removed))
}

fn string_map_key_removed(key: &Arc<str>, removed_keys: &[RuntimeMapKey]) -> bool {
    removed_keys
        .iter()
        .any(|removed| removed.as_str().is_some_and(|removed| removed == key.as_ref()))
}

fn runtime_map_keys_match(left: &RuntimeMapKey, right: &RuntimeMapKey) -> bool {
    left == right
        || left
            .as_str()
            .zip(right.as_str())
            .is_some_and(|(left, right)| left == right)
}
