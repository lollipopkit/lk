#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use anyhow::{Result, anyhow};

use crate::{
    expr::Expr,
    val::LiteralVal,
    vm::analysis::{PerfContainerBuildFact, PerfContainerFact, PerfContainerMoveFact, PerfValueKind},
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
        // `NewList` names its element window as (u8 base, u8 len), so a longer
        // literal is not one instruction — and 256 live registers would not fit
        // the same operand anyway. This used to `bail!`, and only when constant
        // folding did not apply: an all-constant literal of any length becomes a
        // heap constant above, so `[0, …, 399]` compiled and `[0, …, 398, x]`
        // did not. 255 was the opcode's operand width, never a rule about lists.
        // A literal that fits keeps the window path exactly as it was; a longer
        // one builds empty and pushes everything, because the window competes
        // with `dst` for the same 256 registers — a 255-element window leaves
        // `dst` at 256, which the operand cannot name either.
        let len = elements.len();
        let window = if len > u8::MAX as usize { 0 } else { len };
        let base = self.alloc_regs(window)?;
        for (offset, element) in elements[..window].iter().enumerate() {
            self.lower_expr_to_register(base + offset as u16, element, "list element")?;
        }
        let dst = self.alloc_reg();
        let pc = self.function.code.len();
        self.emit(Instr::abc(
            Opcode::NewList,
            checked_u8("list dst", dst)?,
            checked_u8("list base", base)?,
            checked_u8("list len", window as u16)?,
        ));
        self.function.performance.set_container_build_fact(
            pc,
            PerfContainerBuildFact {
                move_keys: false,
                move_values: true,
            },
        );
        // The tail goes in one element at a time, each through a scratch
        // register that is handed straight back — holding all of them at once
        // is what runs into the register ceiling from the other side.
        for element in &elements[window..] {
            let watermark = self.next_reg;
            let scratch = self.alloc_reg();
            self.lower_expr_to_register(scratch, element, "list element")?;
            let push_pc = self.function.code.len();
            self.emit(Instr::abc(
                Opcode::ListPush,
                checked_u8("list dst", dst)?,
                checked_u8("list element", scratch)?,
                0,
            ));
            self.function.performance.set_container_move_fact(
                push_pc,
                PerfContainerMoveFact {
                    move_key: false,
                    move_value: true,
                },
            );
            self.next_reg = self.live_register_floor().max(watermark);
        }
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
        // Same ceiling as `lower_list`, one bit tighter: `NewMap` names its
        // key/value window as (u8 base, u8 len) and each entry costs two
        // registers, so a literal past 127 entries builds empty and sets the
        // rest. It used to `bail!`, and — like the list — only when constant
        // folding did not apply, so `{"a": 1, …}` of any length compiled until
        // one value stopped being a literal.
        let len = entries.len();
        let window = if len > i8::MAX as usize { 0 } else { len };
        let base = self.alloc_regs(
            window
                .checked_mul(2)
                .ok_or_else(|| anyhow!("Compiler map entry overflow"))?,
        )?;
        for (offset, (key, value)) in entries[..window].iter().enumerate() {
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
            checked_u8("map len", window as u16)?,
        ));
        self.function.performance.set_container_build_fact(
            pc,
            PerfContainerBuildFact {
                move_keys: true,
                move_values: true,
            },
        );
        // The tail, one entry at a time through two scratch registers that are
        // handed back. A later duplicate key overwrites an earlier one here just
        // as it does inside `NewMap`, so the route does not change the answer.
        for (key, value) in &entries[window..] {
            let watermark = self.next_reg;
            let key_reg = self.alloc_reg();
            self.lower_expr_to_register(key_reg, key, "map key")?;
            let value_reg = self.alloc_reg();
            self.lower_expr_to_register(value_reg, value, "map value")?;
            let set_pc = self.function.code.len();
            self.emit(Instr::abc(
                Opcode::SetIndex,
                checked_u8("map dst", dst)?,
                checked_u8("map key", key_reg)?,
                checked_u8("map value", value_reg)?,
            ));
            self.function.performance.set_container_move_fact(
                set_pc,
                PerfContainerMoveFact {
                    move_key: true,
                    move_value: true,
                },
            );
            self.next_reg = self.live_register_floor().max(watermark);
        }
        self.set_register_map_fact(dst, map_fact_from_exprs(entries));
        Ok(dst)
    }

    /// `Name { field: value, … }`, however many fields it has.
    ///
    /// `NewObject` reads its fields from a contiguous window of *two* registers
    /// each plus one for the type name, so the window is what runs out first —
    /// and the diagnostics disagreed about where. The guard said "max 127", but
    /// 127 fields need 255 window registers plus `dst`, so 127 was never
    /// reachable: at ~85 fields the surrounding locals already pushed the
    /// allocator over and the failure came out as "this function needs more than
    /// 256 registers", blaming the function for a limit belonging to one
    /// literal. Two messages, one real ceiling, and neither of them named it.
    ///
    /// So the window is sized against what is actually free, and every field it
    /// cannot hold is set afterwards on the finished object, one at a time,
    /// through a scratch register that is handed straight back. Same shape as the
    /// list and map literals next door.
    pub(super) fn lower_struct_literal(&mut self, name: &str, fields: &[(String, Box<Expr>)]) -> Result<u16> {
        let len = fields.len();
        // `1 + 2 * window` for the window itself, one more for `dst`, and every
        // register must still be nameable in 8 bits.
        let free = (u8::MAX as usize).saturating_sub(self.next_reg as usize);
        let window = len.min(free.saturating_sub(2) / 2).min(i8::MAX as usize);

        let base = self.alloc_regs(1 + window * 2)?;
        self.emit_literal_to_register(base, &LiteralVal::from_str(name))?;
        for (offset, (key, value)) in fields[..window].iter().enumerate() {
            let key_dst = base + 1 + (offset as u16 * 2);
            self.emit_literal_to_register(key_dst, &LiteralVal::from_str(key))?;
            self.lower_expr_to_register(key_dst + 1, value, "object value")?;
        }

        let dst = self.alloc_reg();
        self.emit(Instr::abc(
            Opcode::NewObject,
            checked_u8("object dst", dst)?,
            checked_u8("object base", base)?,
            checked_u8("object len", window as u16)?,
        ));
        self.set_register_kind(dst, PerfValueKind::Object);

        for (key, value) in &fields[window..] {
            let watermark = self.next_reg;
            let const_key = self.push_string(key)?;
            let scratch = self.alloc_reg();
            self.lower_expr_to_register(scratch, value, "object value")?;
            let set_pc = self.function.code.len();
            if const_key <= u8::MAX as u16 {
                self.emit(Instr::abc(
                    Opcode::SetFieldK,
                    checked_u8("object dst", dst)?,
                    checked_u8("object value", scratch)?,
                    const_key as u8,
                ));
            } else {
                // The const pool outgrew the `c` operand; the key goes in a
                // register instead. Rare, and the only alternative is refusing a
                // program for how many strings it happens to contain.
                let key_reg = self.alloc_reg();
                self.emit_literal_to_register(key_reg, &LiteralVal::from_str(key))?;
                self.emit(Instr::abc(
                    Opcode::SetIndex,
                    checked_u8("object dst", dst)?,
                    checked_u8("object key", key_reg)?,
                    checked_u8("object value", scratch)?,
                ));
            }
            self.function.performance.set_container_move_fact(
                set_pc,
                PerfContainerMoveFact {
                    move_key: false,
                    move_value: true,
                },
            );
            self.next_reg = self.live_register_floor().max(watermark);
        }
        Ok(dst)
    }

    pub(super) fn lower_range_expr(
        &mut self,
        start: Option<&Expr>,
        end: Option<&Expr>,
        inclusive: bool,
        step: Option<&Expr>,
    ) -> Result<u16> {
        // `let r = 0..;` — a range materializes eagerly here (it *is* a list),
        // so an endless one has no value to build. Same rule, said the same way
        // as the `for` form.
        let end = end.ok_or_else(|| {
            anyhow!(
                "a range needs an end — `0..n` — because a range is built as a list of its elements, and \
                 `0..` has no last element"
            )
        })?;
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
