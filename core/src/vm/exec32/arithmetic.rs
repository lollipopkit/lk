use std::sync::Arc;

use anyhow::{Result, bail};

use crate::val::{HeapValue, RuntimeVal, TypedList};
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

    pub(super) fn dynamic_numeric_binary(
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
    pub(super) fn float_binary(&mut self, instr: Instr32, op: impl FnOnce(f64, f64) -> f64) -> Result<()> {
        let lhs = self.read_number(instr.b())?;
        let rhs = self.read_number(instr.c())?;
        self.write(instr.a(), RuntimeVal::Float(op(lhs, rhs)))?;
        self.pc += 1;
        Ok(())
    }

    #[inline]
    pub(super) fn int_compare(&mut self, instr: Instr32, op: impl FnOnce(i64, i64) -> bool) -> Result<()> {
        let lhs = self.read_int(instr.b())?;
        let rhs = self.read_int(instr.c())?;
        self.write(instr.a(), RuntimeVal::Bool(op(lhs, rhs)))?;
        self.pc += 1;
        Ok(())
    }

    pub(super) fn values_equal(&self, lhs: u8, rhs: u8) -> Result<bool> {
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
}
