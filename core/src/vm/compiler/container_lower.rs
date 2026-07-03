#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use anyhow::{Result, anyhow, bail};

use crate::{
    expr::Expr,
    val::LiteralVal,
    vm::analysis::{PerfContainerBuildFact, PerfContainerFact, PerfValueKind},
};

use super::{
    Compiler, Instr, Opcode,
    facts::{list_fact_from_exprs, map_fact_from_exprs},
    support::{checked_u8, const_heap_list_from_expr_literals, const_heap_map_from_expr_literals},
};

impl Compiler {
    pub(super) fn lower_list(&mut self, elements: &[Box<Expr>]) -> Result<u16> {
        if let Some(value) = const_heap_list_from_expr_literals(elements)? {
            let dst = self.alloc_reg();
            let k = self.push_heap_value(value)?;
            self.emit(Instr::abx(Opcode::LoadHeapConst, checked_u8("list dst", dst)?, k));
            self.set_register_list_fact(dst, list_fact_from_exprs(elements));
            return Ok(dst);
        }
        let len = elements.len();
        if len > u8::MAX as usize {
            bail!("Compiler list literal has {} elements, max {}", len, u8::MAX);
        }
        let base = self.alloc_regs(len)?;
        for (offset, element) in elements.iter().enumerate() {
            self.lower_expr_to_register(base + offset as u16, element, "list element")?;
        }
        let dst = self.alloc_reg();
        let pc = self.function.code.len();
        self.emit(Instr::abc(
            Opcode::NewList,
            checked_u8("list dst", dst)?,
            checked_u8("list base", base)?,
            checked_u8("list len", len as u16)?,
        ));
        self.function.performance.set_container_build_fact(
            pc,
            PerfContainerBuildFact {
                move_keys: false,
                move_values: true,
            },
        );
        self.set_register_list_fact(dst, list_fact_from_exprs(elements));
        Ok(dst)
    }

    pub(super) fn lower_map(&mut self, entries: &[(Box<Expr>, Box<Expr>)]) -> Result<u16> {
        if let Some(value) = const_heap_map_from_expr_literals(entries)? {
            let dst = self.alloc_reg();
            let k = self.push_heap_value(value)?;
            self.emit(Instr::abx(Opcode::LoadHeapConst, checked_u8("map dst", dst)?, k));
            self.set_register_map_fact(dst, map_fact_from_exprs(entries));
            return Ok(dst);
        }
        let len = entries.len();
        if len > i8::MAX as usize {
            bail!("Compiler map literal has {} entries, max {}", len, i8::MAX);
        }
        let base = self.alloc_regs(
            len.checked_mul(2)
                .ok_or_else(|| anyhow!("Compiler map entry overflow"))?,
        )?;
        for (offset, (key, value)) in entries.iter().enumerate() {
            let key_dst = base + (offset as u16 * 2);
            self.lower_expr_to_register(key_dst, key, "map key")?;
            self.lower_expr_to_register(key_dst + 1, value, "map value")?;
        }
        let dst = self.alloc_reg();
        let pc = self.function.code.len();
        self.emit(Instr::abc(
            Opcode::NewMap,
            checked_u8("map dst", dst)?,
            checked_u8("map base", base)?,
            checked_u8("map len", len as u16)?,
        ));
        self.function.performance.set_container_build_fact(
            pc,
            PerfContainerBuildFact {
                move_keys: true,
                move_values: true,
            },
        );
        self.set_register_map_fact(dst, map_fact_from_exprs(entries));
        Ok(dst)
    }

    pub(super) fn lower_struct_literal(&mut self, name: &str, fields: &[(String, Box<Expr>)]) -> Result<u16> {
        let len = fields.len();
        if len > i8::MAX as usize {
            bail!("Compiler object literal has {} fields, max {}", len, i8::MAX);
        }
        let base = self.alloc_regs(
            len.checked_mul(2)
                .and_then(|slots| slots.checked_add(1))
                .ok_or_else(|| anyhow!("Compiler object field overflow"))?,
        )?;
        self.emit_literal_to_register(base, &LiteralVal::from_str(name))?;
        for (offset, (key, value)) in fields.iter().enumerate() {
            let key_dst = base + 1 + (offset as u16 * 2);
            self.emit_literal_to_register(key_dst, &LiteralVal::from_str(key))?;
            self.lower_expr_to_register(key_dst + 1, value, "object value")?;
        }

        let dst = self.alloc_reg();
        self.emit(Instr::abc(
            Opcode::NewObject,
            checked_u8("object dst", dst)?,
            checked_u8("object base", base)?,
            checked_u8("object len", len as u16)?,
        ));
        self.set_register_kind(dst, PerfValueKind::Object);
        Ok(dst)
    }

    pub(super) fn lower_range_expr(
        &mut self,
        start: Option<&Expr>,
        end: Option<&Expr>,
        inclusive: bool,
        step: Option<&Expr>,
    ) -> Result<u16> {
        let end = end.ok_or_else(|| anyhow!("Compiler open-ended range expression is not supported"))?;
        let base = self.alloc_regs(3)?;
        match start {
            Some(start) => self.lower_expr_to_register(base, start, "range start")?,
            None => self.emit_literal_to_register(base, &LiteralVal::Int(0))?,
        }
        self.lower_expr_to_register(base + 1, end, "range end")?;
        match step {
            Some(step) => self.lower_expr_to_register(base + 2, step, "range step")?,
            None => self.emit_literal_to_register(base + 2, &LiteralVal::Int(1))?,
        }

        let dst = self.alloc_reg();
        self.emit(Instr::abc(
            Opcode::NewRange,
            checked_u8("range dst", dst)?,
            checked_u8("range base", base)?,
            u8::from(inclusive),
        ));
        self.set_register_list_fact(
            dst,
            PerfContainerFact {
                value_kind: PerfValueKind::Int,
                known_len: None,
                adoptable: false,
            },
        );
        Ok(dst)
    }
}
