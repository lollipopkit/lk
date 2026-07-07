#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use anyhow::Result;

use crate::{
    expr::Expr,
    util::fast_map::FastHashMap,
    val::{LiteralVal, RuntimeMapKey},
    vm::{ConstHeapValue, ConstRuntimeValue},
};

use super::{
    Compiler, Instr, Opcode,
    support::{
        checked_u8, const_heap_map_from_expr_literals, const_runtime_map_key_from_literal, simple_local_expr_name,
    },
};

impl Compiler {
    pub(super) fn record_const_map_local_from_expr(&mut self, name: &str, value: &Expr) -> Result<()> {
        if !self.loops.is_empty() {
            self.const_map_locals.remove(name);
            return Ok(());
        }
        let Some(entries) = const_heap_map_from_expr_literals_from_expr(value)? else {
            self.const_map_locals.remove(name);
            return Ok(());
        };
        self.const_map_locals.insert(name.to_string(), entries);
        Ok(())
    }

    pub(super) fn clear_const_map_local(&mut self, name: &str) {
        self.const_map_locals.remove(name);
    }

    pub(super) fn clear_const_map_target(&mut self, target: &Expr) {
        if let Some(name) = simple_local_expr_name(target) {
            self.clear_const_map_local(name);
        }
    }

    pub(super) fn lower_const_map_get(&mut self, target: &Expr, key: &Expr) -> Result<Option<u16>> {
        let Some(name) = simple_local_expr_name(target) else {
            return Ok(None);
        };
        let Some(map) = self.const_map_locals.get(name) else {
            return Ok(None);
        };
        let Some(key) = const_map_key_from_expr(key)? else {
            return Ok(None);
        };
        if key.as_str().is_none() {
            return Ok(None);
        }
        let Some(value) = map.get(&key).cloned() else {
            return Ok(None);
        };
        if !matches!(value, ConstRuntimeValue::Int(_)) {
            return Ok(None);
        }
        self.emit_const_runtime_value(&value).map(Some)
    }

    fn emit_const_runtime_value(&mut self, value: &ConstRuntimeValue) -> Result<u16> {
        match value {
            ConstRuntimeValue::Nil => self.lower_val(&LiteralVal::Nil),
            ConstRuntimeValue::Bool(value) => self.lower_val(&LiteralVal::Bool(*value)),
            ConstRuntimeValue::Int(value) => self.lower_val(&LiteralVal::Int(*value)),
            ConstRuntimeValue::Float(value) => self.lower_val(&LiteralVal::Float(*value)),
            ConstRuntimeValue::ShortStr(value) => self.lower_val(&LiteralVal::ShortStr(*value)),
            ConstRuntimeValue::Heap(value) => {
                let dst = self.alloc_reg();
                let k = self.push_heap_value((**value).clone())?;
                self.emit(Instr::abx(
                    Opcode::LoadHeapConst,
                    checked_u8("const map value dst", dst)?,
                    k,
                ));
                Ok(dst)
            }
        }
    }
}

fn const_heap_map_from_expr_literals_from_expr(
    expr: &Expr,
) -> Result<Option<FastHashMap<RuntimeMapKey, ConstRuntimeValue>>> {
    let Expr::Map(entries) = expr else {
        return Ok(None);
    };
    match const_heap_map_from_expr_literals(entries)? {
        Some(ConstHeapValue::Map(entries)) => Ok(Some(entries)),
        _ => Ok(None),
    }
}

fn const_map_key_from_expr(expr: &Expr) -> Result<Option<RuntimeMapKey>> {
    match expr {
        Expr::Paren(inner) => const_map_key_from_expr(inner),
        Expr::Literal(value) => const_runtime_map_key_from_literal(value),
        _ => Ok(None),
    }
}
