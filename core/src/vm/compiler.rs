//! Minimal compiler for the new `Function` IR.
//!
//! This is the first migration point from AST to the new VM path. It is
//! deliberately small and independent from the previous `FunctionBuilder`.

#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
mod assign;
mod builder;
mod call;
mod const_maps;
mod container_lower;
mod entry;
mod facts;
#[cfg(test)]
mod facts_tests;
mod for_value_usage;
mod free_vars;
mod inline;
mod loop_consts;
mod lower_into;
mod match_expr;
mod pattern_bind;
mod pattern_control;
mod range_loop;
mod support;
#[cfg(test)]
mod tests;

use crate::compat::collections::{HashMap, HashSet};
use alloc::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::{
    expr::{Expr, Pattern, TemplateStringPart},
    operator::{BinOp, UnaryOp},
    stmt::{ForPattern, Program, Stmt},
    util::fast_map::FastHashMap,
    val::{FunctionNamedParamType, LiteralVal, RuntimeMapKey, ShortStr, Type},
};

use super::{ConstHeapValue, ConstRuntimeValue, Function, GlobalSlot, Instr, Module, NativeEntry, Opcode};
use crate::vm::analysis::{
    PerfCallTargetKind, PerfContainerBuildFact, PerfGlobalFact, PerfKeyFact, PerfRegisterFact, PerfStringIntKeyFact,
    PerfValueKind,
};
use facts::*;
use for_value_usage::{stmt_shadows_name_deep, stmt_uses_for_binding_value};
use free_vars::{collect_expr_closure_captures, collect_expr_free_vars, collect_stmt_closure_captures};
use loop_consts::ScalarLoopConstKey;
use support::*;

#[derive(Debug, Default)]
pub struct Compiler {
    function: Function,
    next_reg: u16,
    peak_reg: u16, // highest next_reg ever reached — used for register_count
    locals: HashMap<String, u16>,
    function_names: HashMap<String, u32>,
    function_signatures: HashMap<String, FunctionSignature>,
    function_bodies: HashMap<String, FunctionInlineBody>,
    native_names: HashMap<String, u32>,
    global_names: HashMap<String, u32>,
    capture_names: HashMap<String, u16>,
    capture_cells: HashSet<String>,
    cell_locals: HashSet<String>,
    /// Loop-pattern variables of the enclosing `for` loops: the fused loop
    /// opcodes own the raw register, so a capture takes a fresh snapshot cell
    /// per capture site instead of re-binding the register (per-iteration
    /// binding semantics). `slot` is filled when the pattern binds; a
    /// same-named local whose binding differs (a fresh `let` in the body) is
    /// an ordinary local, not the loop variable.
    loop_snapshot_vars: Vec<LoopSnapshotVar>,
    dynamic_function_base: u32,
    pending_functions: Vec<Function>,
    inline_stack: Vec<String>,
    loops: Vec<LoopPatch>,
    loop_const_scopes: Vec<HashMap<ScalarLoopConstKey, u16>>,
    single_char_string_locals: HashMap<String, u16>,
    const_map_locals: HashMap<String, FastHashMap<RuntimeMapKey, ConstRuntimeValue>>,
    local_rebind_suppression: u16,
    top_level: bool,
    emitted_return: bool,
}

impl Compiler {
    fn lower_expr(&mut self, expr: &Expr) -> Result<u16> {
        self.record_expr_analysis(expr);
        match expr {
            Expr::Paren(inner) => self.lower_expr(inner),
            Expr::Literal(value) => self.lower_val(value),
            Expr::Var(name) => self.lower_var(name),
            Expr::List(elements) => self.lower_list(elements),
            Expr::Map(entries) => self.lower_map(entries),
            Expr::StructLiteral { name, fields } => self.lower_struct_literal(name, fields),
            Expr::Access(target, key) => self.lower_access(target, key),
            Expr::Call(name, args) => self.lower_named_call(name, args),
            Expr::CallExpr(callee, args) => self.lower_call_expr(callee, args),
            Expr::CallNamed(callee, positional, named) => self.lower_named_arg_call(callee, positional, named),
            Expr::Closure { params, body } => self.lower_closure(params, body),
            Expr::Unary(op, inner) => self.lower_unary(op, inner),
            Expr::And(lhs, rhs) => self.lower_short_circuit(lhs, rhs, ShortCircuitKind::And),
            Expr::Or(lhs, rhs) => self.lower_short_circuit(lhs, rhs, ShortCircuitKind::Or),
            Expr::NullishCoalescing(lhs, rhs) => self.lower_short_circuit(lhs, rhs, ShortCircuitKind::Nullish),
            Expr::OptionalAccess(target, key) => self.lower_optional_access(target, key),
            Expr::TemplateString(parts) => self.lower_template_string(parts),
            Expr::Block(statements) => self.lower_block_expr(statements),
            Expr::Range {
                start,
                end,
                inclusive,
                step,
            } => self.lower_range_expr(start.as_deref(), end.as_deref(), *inclusive, step.as_deref()),
            Expr::Match { value, arms } => self.lower_match_expr(value, arms),
            Expr::Bin(lhs, op, rhs) => self.lower_bin(lhs, op, rhs),
            Expr::Conditional(condition, then_expr, else_expr) => {
                self.lower_conditional(condition, then_expr, else_expr)
            }
            Expr::Yield(inner) => self.lower_yield(inner),
            other => bail!("Compiler does not support expression yet: {:?}", expr_kind(other)),
        }
    }

    fn record_expr_analysis(&mut self, expr: &Expr) {
        if let Some(analysis) = super::ssa::pipeline::analyze_expr(expr) {
            self.function.analyses.push(analysis);
        }
    }

    fn lower_stmt(&mut self, stmt: &Stmt) -> Result<()> {
        match stmt {
            Stmt::Attributed { item, .. } => self.lower_stmt(item)?,
            Stmt::Empty => {}
            Stmt::Expr(expr) => {
                let watermark = self.next_reg;
                if !self.try_lower_rewritten_set_index_expr(expr)?
                    && !self.try_lower_builtin_method_statement(expr)?
                    && !self.try_lower_dead_literal_expr(expr)?
                {
                    self.lower_readonly_operand(expr)?;
                }
                self.next_reg = watermark;
            }
            Stmt::Return { value } => {
                if let Some(value) = value {
                    let value = self.lower_readonly_operand(value)?;
                    self.emit_return(value)?;
                } else {
                    self.emit_empty_return();
                }
            }
            Stmt::Let { pattern, value, .. } => self.lower_let(pattern, value)?,
            Stmt::Define { name, value } => self.lower_define(name, value)?,
            Stmt::Assign { name, value, .. } => {
                let watermark = self.next_reg;
                self.lower_assign(name, value)?;
                self.next_reg = self.live_register_floor().max(watermark);
            }
            Stmt::CompoundAssign { name, op, value, .. } => {
                let watermark = self.next_reg;
                self.lower_compound_assign(name, op, value)?;
                self.next_reg = self.live_register_floor().max(watermark);
            }
            Stmt::If {
                condition,
                then_stmt,
                else_stmt,
            } => self.lower_if(condition, then_stmt, else_stmt.as_deref())?,
            Stmt::IfLet {
                pattern,
                value,
                then_stmt,
                else_stmt,
            } => self.lower_if_let(pattern, value, then_stmt, else_stmt.as_deref())?,
            Stmt::While { condition, body } => self.lower_while(condition, body)?,
            Stmt::WhileLet { pattern, value, body } => self.lower_while_let(pattern, value, body)?,
            Stmt::For {
                pattern,
                iterable,
                body,
            } => self.lower_for(pattern, iterable, body)?,
            Stmt::Break => self.lower_break()?,
            Stmt::Continue => self.lower_continue()?,
            Stmt::Import(_) | Stmt::Struct { .. } | Stmt::TypeAlias { .. } => {}
            Stmt::Trait { name, methods } => self.lower_trait_decl(name, methods)?,
            Stmt::Impl {
                trait_name,
                target_type,
                methods,
            } => self.lower_impl_decl(trait_name, target_type, methods)?,
            Stmt::Function { name, .. } => self.lower_function_decl(name)?,
            Stmt::Block { statements } => {
                let watermark = self.next_reg;
                let locals = self.locals.clone();
                let cell_locals = self.cell_locals.clone();
                let const_map_locals = self.const_map_locals.clone();
                self.local_rebind_suppression += 1;
                self.lower_stmt_sequence(statements)?;
                self.local_rebind_suppression -= 1;
                // In-block promotions of *outer* locals must survive the
                // scope restore (the register now holds the cell); dropping
                // them left later reads loading the raw cell object.
                self.cell_locals = self.scope_restored_cell_locals(&locals, cell_locals);
                self.locals = locals;
                self.const_map_locals = const_map_locals;
                if !self.emitted_return {
                    self.next_reg = self.live_register_floor().max(watermark);
                }
            }
        }
        Ok(())
    }

    fn lower_stmt_sequence(&mut self, statements: &[Box<Stmt>]) -> Result<()> {
        let mut index = 0;
        while index < statements.len() {
            if index + 1 < statements.len()
                && (self
                    .try_lower_default_assign_if_chain(statements[index].as_ref(), statements[index + 1].as_ref())?
                    || self.try_lower_move2_assign_pair(statements[index].as_ref(), statements[index + 1].as_ref())?)
            {
                index += 2;
            } else {
                self.lower_stmt(statements[index].as_ref())?;
                index += 1;
            }
            if self.emitted_return {
                break;
            }
        }
        Ok(())
    }

    fn try_lower_default_assign_if_chain(&mut self, first: &Stmt, second: &Stmt) -> Result<bool> {
        let Some((name, default_value, is_let)) = default_assign_candidate(first) else {
            return Ok(false);
        };
        if self.cell_locals.contains(name)
            || !pure_default_expr(default_value)
            || expr_mentions_name(default_value, name)
            || !if_chain_assigns_only_target(second, name)
            || if_chain_condition_mentions_name(second, name)
        {
            return Ok(false);
        }
        let Stmt::If {
            condition,
            then_stmt,
            else_stmt,
        } = second
        else {
            return Ok(false);
        };

        let watermark = self.next_reg;
        let target = if let Some(reg) = self.locals.get(name).copied() {
            // A re-`let` over a promoted cell or over a live loop counter
            // must not write the old register in place — the generic path
            // allocates the fresh binding.
            if is_let && (self.cell_locals.contains(name) || self.active_loop_binding_slot(name) == Some(reg)) {
                return Ok(false);
            }
            reg
        } else if is_let {
            let reg = self.alloc_reg();
            self.insert_fresh_local(name.to_string(), reg);
            reg
        } else {
            return Ok(false);
        };
        self.function
            .performance
            .set_register_kind(target, facts::expr_static_value_kind(default_value));
        self.lower_defaulted_if_chain(name, default_value, condition, then_stmt, else_stmt.as_deref())?;
        self.next_reg = self.live_register_floor().max(watermark).max(target + 1);
        Ok(true)
    }

    fn lower_defaulted_if_chain(
        &mut self,
        name: &str,
        default_value: &Expr,
        condition: &Expr,
        then_stmt: &Stmt,
        else_stmt: Option<&Stmt>,
    ) -> Result<()> {
        let false_jumps = self.emit_condition_false_jumps(condition)?;

        self.emitted_return = false;
        self.local_rebind_suppression += 1;
        self.lower_stmt(then_stmt)?;
        self.local_rebind_suppression -= 1;
        let then_returns = self.emitted_return;

        let jmp_end = (!then_returns).then(|| self.emit_jmp_placeholder());
        let else_start = self.function.code.len();
        self.patch_condition_false_jumps(false_jumps, else_start)?;

        self.emitted_return = false;
        if let Some(Stmt::If {
            condition,
            then_stmt,
            else_stmt,
        }) = else_stmt
        {
            self.lower_defaulted_if_chain(name, default_value, condition, then_stmt, else_stmt.as_deref())?;
        } else {
            debug_assert!(else_stmt.is_none());
            self.local_rebind_suppression += 1;
            self.lower_assign(name, default_value)?;
            self.local_rebind_suppression -= 1;
        }
        let else_returns = self.emitted_return;

        if let Some(jmp_end) = jmp_end {
            let end = self.function.code.len();
            self.patch_jmp(jmp_end, end)?;
        }
        self.emitted_return = then_returns && else_returns;
        Ok(())
    }

    fn try_lower_move2_assign_pair(&mut self, first: &Stmt, second: &Stmt) -> Result<bool> {
        let (
            Stmt::Assign {
                name: first_dst,
                value: first_value,
                ..
            },
            Stmt::Assign {
                name: second_dst,
                value: second_value,
                ..
            },
        ) = (first, second)
        else {
            return Ok(false);
        };
        let Some(first_src) = simple_local_expr_name(first_value) else {
            return Ok(false);
        };
        if first_src != second_dst {
            return Ok(false);
        }
        let Some(second_src) = simple_local_expr_name(second_value) else {
            return Ok(false);
        };
        if first_dst == first_src
            || self.cell_locals.contains(first_dst)
            || self.cell_locals.contains(first_src)
            || self.cell_locals.contains(second_src)
            || self.const_map_locals.contains_key(first_dst)
            || self.const_map_locals.contains_key(first_src)
            || self.const_map_locals.contains_key(second_src)
        {
            return Ok(false);
        }
        let Some(first_dst_reg) = self.locals.get(first_dst).copied() else {
            return Ok(false);
        };
        let Some(first_src_reg) = self.locals.get(first_src).copied() else {
            return Ok(false);
        };
        let Some(second_src_reg) = self.locals.get(second_src).copied() else {
            return Ok(false);
        };

        self.emit(Instr::abc(
            Opcode::Move2,
            checked_u8("move2 first dst", first_dst_reg)?,
            checked_u8("move2 shared slot", first_src_reg)?,
            checked_u8("move2 second src", second_src_reg)?,
        ));
        self.function
            .performance
            .copy_register_fact(first_dst_reg, first_src_reg);
        self.function
            .performance
            .copy_register_fact(first_src_reg, second_src_reg);
        self.clear_const_map_local(first_dst);
        self.clear_const_map_local(first_src);
        Ok(true)
    }

    fn lower_define(&mut self, name: &str, value: &Expr) -> Result<()> {
        if !self.top_level
            && !self.cell_locals.contains(name)
            && let Some(reg) = self.cached_loop_literal_expr(value)
        {
            self.clear_const_map_local(name);
            self.insert_local(name.to_string(), reg);
            self.next_reg = self.live_register_floor().max(self.next_reg);
            return Ok(());
        }
        let watermark = self.next_reg;
        let slot = if let Some(slot) = self.locals.get(name).copied() {
            if self.active_loop_binding_slot(name) == Some(slot) || self.cell_locals.contains(name) {
                // A fresh binding must not write the old register in place:
                // it would clobber the counter the fused loop opcodes drive
                // (`for i { let i = …; }`), or overwrite a promoted cell that
                // earlier-emitted reads (a loop condition or a statement
                // before this `let`, re-executed on the back edge) still
                // load through.
                self.alloc_reg()
            } else {
                self.local_write_slot(slot).0
            }
        } else {
            self.alloc_reg()
        };
        if !self.try_lower_expr_to_register(slot, value)? {
            let value = self.lower_expr(value)?;
            let move_source = !self.is_current_local_slot(value);
            self.emit_move_with_policy(slot, value, "define local", move_source)?;
        }
        if self.top_level
            && let Some(global_slot) = self.global_names.get(name).copied()
        {
            self.emit_set_global(slot, global_slot)?;
        }
        self.record_const_map_local_from_expr(name, value)?;
        self.insert_fresh_local(name.to_string(), slot);
        self.next_reg = self.live_register_floor().max(watermark).max(slot + 1);
        Ok(())
    }

    fn lower_val(&mut self, value: &LiteralVal) -> Result<u16> {
        if let Some(reg) = self.cached_loop_literal(value) {
            return Ok(reg);
        }
        let dst = self.alloc_reg();
        match value {
            LiteralVal::Nil => {
                self.emit(Instr::abc(Opcode::LoadNil, checked_u8("dst", dst)?, 0, 0));
                self.set_register_kind(dst, PerfValueKind::Nil);
            }
            LiteralVal::Bool(value) => self.emit(Instr::abc(
                Opcode::LoadBool,
                checked_u8("dst", dst)?,
                u8::from(*value),
                0,
            )),
            LiteralVal::Int(value) => {
                let k = self.push_int(*value)?;
                self.emit(Instr::abx(Opcode::LoadInt, checked_u8("dst", dst)?, k));
                self.set_register_kind(dst, PerfValueKind::Int);
            }
            LiteralVal::Float(value) => {
                let k = self.push_float(*value)?;
                self.emit(Instr::abx(Opcode::LoadFloat, checked_u8("dst", dst)?, k));
                self.set_register_kind(dst, PerfValueKind::Float);
            }
            value if value.as_str().is_some() => {
                let value = value.as_str().expect("checked string");
                if ShortStr::new(value).is_some() {
                    let k = self.push_string(value)?;
                    self.emit(Instr::abx(Opcode::LoadString, checked_u8("dst", dst)?, k));
                } else {
                    let k = self.push_heap_value(ConstHeapValue::LongString(value.into()))?;
                    self.emit(Instr::abx(Opcode::LoadHeapConst, checked_u8("dst", dst)?, k));
                }
                self.set_register_kind(dst, PerfValueKind::String);
            }
            other => {
                bail!(
                    "Compiler cannot materialize AST literal value yet: {}",
                    ast_literal_kind(other)
                );
            }
        }
        if matches!(value, LiteralVal::Bool(_)) {
            self.set_register_kind(dst, PerfValueKind::Bool);
        }
        Ok(dst)
    }

    fn try_lower_dead_literal_expr(&mut self, expr: &Expr) -> Result<bool> {
        match expr {
            Expr::Paren(inner) => self.try_lower_dead_literal_expr(inner),
            Expr::Literal(value) if literal_dead_write_is_safe(value) => {
                self.lower_val(value)?;
                self.mark_last_dead_write();
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn lower_access(&mut self, target: &Expr, key: &Expr) -> Result<u16> {
        let dst = self.alloc_reg();
        self.lower_access_to_register(dst, target, key)?;
        Ok(dst)
    }

    pub(super) fn lower_access_to_register(&mut self, dst: u16, target: &Expr, key: &Expr) -> Result<()> {
        let target = self.lower_readonly_access_target(target)?;
        let index_fact = index_fact_from_target(&self.function.performance, target);
        if let Some((suffix, key_fact)) = self.try_lower_string_int_key_for_map(index_fact, key)? {
            let pc = self.function.code.len();
            self.emit(Instr::abc(
                Opcode::GetIndexStrI,
                checked_u8("string-int index dst", dst)?,
                checked_u8("string-int index target", target)?,
                checked_u8("string-int index suffix", suffix)?,
            ));
            self.function.performance.set_key_fact(pc, key_fact);
            self.function.performance.clear_register(dst);
            if let Some(fact) = index_fact {
                self.function.performance.set_index_fact(pc, fact);
            }
            return Ok(());
        }
        let (key, key_fact) = self.lower_index_key_for_target(target, index_fact, key)?;
        let pc = self.function.code.len();
        if list_int_key(index_fact, &self.function.performance, key) {
            self.emit(Instr::abc(
                Opcode::GetList,
                checked_u8("list get dst", dst)?,
                checked_u8("list get target", target)?,
                checked_u8("list get key", key)?,
            ));
        } else if let Some(const_key) = get_field_key(index_fact, key_fact) {
            self.emit(Instr::abc(
                Opcode::GetFieldK,
                checked_u8("field dst", dst)?,
                checked_u8("field target", target)?,
                checked_u8("field key", const_key)?,
            ));
        } else {
            self.emit(Instr::abc(
                Opcode::GetIndex,
                checked_u8("index dst", dst)?,
                checked_u8("index target", target)?,
                checked_u8("index key", key)?,
            ));
            if let Some(fact) = key_fact {
                self.function.performance.set_key_fact(pc, fact);
            }
        }
        self.function.performance.clear_register(dst);
        if let Some(fact) = index_fact {
            self.function.performance.set_index_fact(pc, fact);
        }
        Ok(())
    }

    fn lower_readonly_access_target(&mut self, target: &Expr) -> Result<u16> {
        if let Expr::Var(name) = target
            && let Some(local) = self.locals.get(name).copied()
            && !self.cell_locals.contains(name)
        {
            return Ok(local);
        }
        self.lower_expr(target)
    }

    fn lower_index_key_for_target(
        &mut self,
        target: u16,
        index_fact: Option<crate::vm::analysis::PerfIndexFact>,
        key: &Expr,
    ) -> Result<(u16, Option<crate::vm::analysis::PerfKeyFact>)> {
        if let Some(text) = short_string_literal_key(key) {
            let const_key = self.push_string(text)?;
            let key_fact = Some(crate::vm::analysis::PerfKeyFact {
                const_key: Some(const_key),
                string_int: None,
            });
            if index_fact.is_some_and(|fact| {
                matches!(
                    fact.target_kind,
                    crate::vm::analysis::PerfIndexTargetKind::Map | crate::vm::analysis::PerfIndexTargetKind::Object
                )
            }) {
                return Ok((target, key_fact));
            }
            let dst = self.alloc_reg();
            self.emit(Instr::abx(Opcode::LoadString, checked_u8("index key", dst)?, const_key));
            self.set_register_kind(dst, PerfValueKind::String);
            return Ok((dst, key_fact));
        }
        Ok((self.lower_readonly_operand(key)?, None))
    }

    pub(super) fn try_lower_string_int_key_for_map(
        &mut self,
        index_fact: Option<crate::vm::analysis::PerfIndexFact>,
        key: &Expr,
    ) -> Result<Option<(u16, PerfKeyFact)>> {
        if !index_fact.is_some_and(|fact| fact.target_kind == crate::vm::analysis::PerfIndexTargetKind::Map) {
            return Ok(None);
        }
        let Some((prefix, suffix_expr)) = string_int_template_key(key) else {
            return Ok(None);
        };
        if !string_int_key_suffix_is_int_like(suffix_expr, &self.locals, &self.function.performance) {
            return Ok(None);
        }
        let suffix = self.lower_readonly_operand(suffix_expr)?;
        if self.function.performance.value_kind(suffix) != PerfValueKind::Int {
            return Ok(None);
        }
        let prefix_key = self.push_string(prefix)?;
        Ok(Some((
            suffix,
            PerfKeyFact {
                const_key: None,
                string_int: Some(PerfStringIntKeyFact {
                    prefix_key,
                    suffix_reg: suffix,
                }),
            },
        )))
    }

    fn lower_optional_access(&mut self, target: &Expr, key: &Expr) -> Result<u16> {
        let target = self.lower_readonly_access_target(target)?;
        let dst = self.alloc_reg();
        self.emit(Instr::abc(Opcode::LoadNil, checked_u8("optional dst", dst)?, 0, 0));

        let is_nil = self.alloc_reg();
        self.emit(Instr::abc(
            Opcode::IsNil,
            checked_u8("optional test dst", is_nil)?,
            checked_u8("optional target", target)?,
            0,
        ));
        let skip_get = self.emit_test_placeholder(is_nil)?;

        let index_fact = index_fact_from_target(&self.function.performance, target);
        let (key, key_fact) = self.lower_index_key_for_target(target, index_fact, key)?;
        let pc = self.function.code.len();
        if list_int_key(index_fact, &self.function.performance, key) {
            self.emit(Instr::abc(
                Opcode::GetList,
                checked_u8("optional list dst", dst)?,
                checked_u8("optional list target", target)?,
                checked_u8("optional list key", key)?,
            ));
        } else if let Some(const_key) = get_field_key(index_fact, key_fact) {
            self.emit(Instr::abc(
                Opcode::GetFieldK,
                checked_u8("optional field dst", dst)?,
                checked_u8("optional field target", target)?,
                checked_u8("optional field key", const_key)?,
            ));
        } else {
            self.emit(Instr::abc(
                Opcode::GetIndex,
                checked_u8("optional get dst", dst)?,
                checked_u8("optional get target", target)?,
                checked_u8("optional get key", key)?,
            ));
            if let Some(fact) = key_fact {
                self.function.performance.set_key_fact(pc, fact);
            }
        }
        self.function.performance.clear_register(dst);
        if let Some(fact) = index_fact {
            self.function.performance.set_index_fact(pc, fact);
        }
        let end = self.function.code.len();
        self.patch_test_true_jump(skip_get, end)?;
        Ok(dst)
    }

    /// `yield expr`: lower the value into a *fresh* register (never an
    /// existing local's own slot — `Yield` overwrites it in place with the
    /// resumed value, and aliasing a local would silently clobber it across
    /// the suspend point) and emit the single-register in/out `Yield` opcode.
    /// The register's static-type fact must be reset: after resuming, it can
    /// hold any type, not whatever `inner` produced.
    fn lower_yield(&mut self, inner: &Expr) -> Result<u16> {
        let dst = self.alloc_reg();
        self.lower_expr_to_register(dst, inner, "yield value")?;
        self.emit(Instr::abc(Opcode::Yield, checked_u8("yield reg", dst)?, 0, 0));
        self.set_register_kind(dst, PerfValueKind::Unknown);
        Ok(dst)
    }

    fn lower_unary(&mut self, op: &UnaryOp, inner: &Expr) -> Result<u16> {
        let src = self.lower_readonly_operand(inner)?;
        let dst = self.alloc_reg();
        let opcode = match op {
            UnaryOp::Not => Opcode::Not,
        };
        self.emit(Instr::abc(
            opcode,
            checked_u8("unary dst", dst)?,
            checked_u8("unary src", src)?,
            0,
        ));
        Ok(dst)
    }

    fn lower_short_circuit(&mut self, lhs: &Expr, rhs: &Expr, kind: ShortCircuitKind) -> Result<u16> {
        let lhs = self.lower_readonly_operand(lhs)?;
        let dst = self.alloc_reg();
        let move_source = !self.is_current_local_slot(lhs);
        self.emit_move_with_policy(dst, lhs, "short circuit lhs", move_source)?;

        let test_reg = match kind {
            ShortCircuitKind::And | ShortCircuitKind::Or => dst,
            ShortCircuitKind::Nullish => {
                let is_nil = self.alloc_reg();
                self.emit(Instr::abc(
                    Opcode::IsNil,
                    checked_u8("nullish test dst", is_nil)?,
                    checked_u8("nullish lhs", dst)?,
                    0,
                ));
                is_nil
            }
        };

        let test_pc = self.emit_test_placeholder(test_reg)?;
        match kind {
            ShortCircuitKind::And | ShortCircuitKind::Nullish => {
                self.lower_expr_to_register(dst, rhs, "short circuit rhs")?;
                let end = self.function.code.len();
                self.patch_test_false_jump(test_pc, end)?;
            }
            ShortCircuitKind::Or => {
                self.lower_expr_to_register(dst, rhs, "short circuit rhs")?;
                let end = self.function.code.len();
                self.patch_test_true_jump(test_pc, end)?;
            }
        }
        Ok(dst)
    }

    pub(super) fn emit_condition_false_jumps(&mut self, condition: &Expr) -> Result<Vec<usize>> {
        match condition {
            Expr::And(lhs, rhs) => {
                if ENABLE_COMPARE_TEST_PAIR_IMMEDIATE_LOWERING
                    && let Some(pc) = self.try_emit_compare_test_pair_immediate_placeholder(lhs, rhs)?
                {
                    return Ok(vec![pc]);
                }
                let mut jumps = self.emit_condition_false_jumps(lhs)?;
                jumps.extend(self.emit_condition_false_jumps(rhs)?);
                Ok(jumps)
            }
            Expr::Or(lhs, rhs) => {
                let lhs = self.lower_readonly_operand(lhs)?;
                let skip_rhs = self.emit_test_placeholder(lhs)?;
                let jumps = self.emit_condition_false_jumps(rhs)?;
                let end = self.function.code.len();
                self.patch_test_true_jump(skip_rhs, end)?;
                Ok(jumps)
            }
            Expr::Bin(lhs, BinOp::Eq, rhs) if expr_is_nil_literal(lhs) => {
                let value = self.lower_readonly_operand(rhs)?;
                Ok(vec![self.emit_branch_placeholder(Opcode::BrNotNil, value)?])
            }
            Expr::Bin(lhs, BinOp::Eq, rhs) if expr_is_nil_literal(rhs) => {
                let value = self.lower_readonly_operand(lhs)?;
                Ok(vec![self.emit_branch_placeholder(Opcode::BrNotNil, value)?])
            }
            Expr::Bin(lhs, BinOp::Ne, rhs) if expr_is_nil_literal(lhs) => {
                let value = self.lower_readonly_operand(rhs)?;
                Ok(vec![self.emit_branch_placeholder(Opcode::BrNil, value)?])
            }
            Expr::Bin(lhs, BinOp::Ne, rhs) if expr_is_nil_literal(rhs) => {
                let value = self.lower_readonly_operand(lhs)?;
                Ok(vec![self.emit_branch_placeholder(Opcode::BrNil, value)?])
            }
            Expr::Bin(lhs, op, rhs) if compare_test_opcode(op).is_some() => {
                if let Some((opcode, value, immediate)) = self.lower_mod_zero_i4_branch_operands(lhs, op, rhs)? {
                    return Ok(vec![self.emit_i4_branch_placeholder(opcode, value, immediate)?]);
                }
                if let Some((opcode, value)) = self.lower_zero_branch_operands(lhs, op, rhs)? {
                    return Ok(vec![self.emit_branch_placeholder(opcode, value)?]);
                }
                if let Some((opcode, value, immediate)) = self.lower_i4_branch_operands(lhs, op, rhs)? {
                    return Ok(vec![self.emit_i4_branch_placeholder(opcode, value, immediate)?]);
                }
                if ENABLE_COMPARE_TEST_IMMEDIATE_LOWERING
                    && let Some((opcode, lhs, rhs)) = self.lower_compare_test_immediate_operands(lhs, op, rhs)?
                {
                    return Ok(vec![
                        self.emit_compare_test_immediate_placeholder(opcode, lhs, rhs, false)?,
                    ]);
                }
                let lhs = self.lower_readonly_operand(lhs)?;
                let rhs = self.lower_readonly_operand(rhs)?;
                if ENABLE_COMPARE_TEST_LOWERING && compare_test_operands_are_int(&self.function.performance, lhs, rhs) {
                    let opcode = compare_test_opcode(op).expect("checked compare-test opcode");
                    return Ok(vec![self.emit_compare_test_placeholder(opcode, lhs, rhs, false)?]);
                }
                let dst = self.alloc_reg();
                let condition = self.emit_bin_op_to_register(dst, op, lhs, rhs)?;
                Ok(vec![self.emit_test_placeholder(condition)?])
            }
            _ => {
                let condition = self.lower_readonly_operand(condition)?;
                Ok(vec![self.emit_test_placeholder(condition)?])
            }
        }
    }

    fn try_emit_compare_test_pair_immediate_placeholder(&mut self, lhs: &Expr, rhs: &Expr) -> Result<Option<usize>> {
        let Some((first_name, first_value)) = equality_u4_local_immediate(lhs) else {
            return Ok(None);
        };
        let Some((second_name, second_value)) = equality_u4_local_immediate(rhs) else {
            return Ok(None);
        };
        let Some(first_reg) = self.locals.get(first_name).copied() else {
            return Ok(None);
        };
        let Some(second_reg) = self.locals.get(second_name).copied() else {
            return Ok(None);
        };
        if self.cell_locals.contains(first_name)
            || self.cell_locals.contains(second_name)
            || self.function.performance.value_kind(first_reg) != PerfValueKind::Int
            || self.function.performance.value_kind(second_reg) != PerfValueKind::Int
        {
            return Ok(None);
        }
        self.emit_compare_test_pair_immediate_placeholder(first_reg, first_value, second_reg, second_value)
            .map(Some)
    }

    fn lower_compare_test_immediate_operands(
        &mut self,
        lhs: &Expr,
        op: &BinOp,
        rhs: &Expr,
    ) -> Result<Option<(Opcode, u16, i8)>> {
        if let Some(immediate) = compare_test_immediate_operand(rhs) {
            let lhs = self.lower_readonly_operand(lhs)?;
            if self.function.performance.value_kind(lhs) == PerfValueKind::Int {
                return Ok(compare_test_immediate_opcode(op).map(|opcode| (opcode, lhs, immediate)));
            }
            return Ok(None);
        }
        if let Some(immediate) = compare_test_immediate_operand(lhs) {
            let rhs = self.lower_readonly_operand(rhs)?;
            if self.function.performance.value_kind(rhs) == PerfValueKind::Int {
                return Ok(reverse_compare_test_immediate_opcode(op).map(|opcode| (opcode, rhs, immediate)));
            }
        }
        Ok(None)
    }

    fn lower_zero_branch_operands(&mut self, lhs: &Expr, op: &BinOp, rhs: &Expr) -> Result<Option<(Opcode, u16)>> {
        let value_expr = if zero_int_literal(rhs) {
            lhs
        } else if zero_int_literal(lhs) {
            rhs
        } else {
            return Ok(None);
        };
        let value = self.lower_readonly_operand(value_expr)?;
        if self.function.performance.value_kind(value) != PerfValueKind::Int {
            return Ok(None);
        }
        let opcode = match op {
            BinOp::Eq => Opcode::BrNeZeroInt,
            BinOp::Ne => Opcode::BrEqZeroInt,
            _ => return Ok(None),
        };
        Ok(Some((opcode, value)))
    }

    fn lower_mod_zero_i4_branch_operands(
        &mut self,
        lhs: &Expr,
        op: &BinOp,
        rhs: &Expr,
    ) -> Result<Option<(Opcode, u16, u8)>> {
        let Some((value_expr, divisor)) = mod_i4_zero_operands(lhs, rhs) else {
            return Ok(None);
        };
        let value = self.lower_readonly_operand(value_expr)?;
        if self.function.performance.value_kind(value) != PerfValueKind::Int {
            return Ok(None);
        }
        let opcode = match op {
            BinOp::Eq => Opcode::BrModNeZeroIntI4,
            BinOp::Ne => Opcode::BrModEqZeroIntI4,
            _ => return Ok(None),
        };
        Ok(Some((opcode, value, divisor)))
    }

    fn lower_i4_branch_operands(&mut self, lhs: &Expr, op: &BinOp, rhs: &Expr) -> Result<Option<(Opcode, u16, u8)>> {
        let (value_expr, immediate) = if let Some(immediate) = u4_literal(rhs) {
            (lhs, immediate)
        } else if let Some(immediate) = u4_literal(lhs) {
            (rhs, immediate)
        } else {
            return Ok(None);
        };
        let value = self.lower_readonly_operand(value_expr)?;
        if self.function.performance.value_kind(value) != PerfValueKind::Int {
            return Ok(None);
        }
        let opcode = match op {
            BinOp::Eq => Opcode::BrNeIntI4,
            BinOp::Ne => Opcode::BrEqIntI4,
            _ => return Ok(None),
        };
        Ok(Some((opcode, value, immediate)))
    }

    pub(super) fn patch_condition_false_jumps(&mut self, jumps: Vec<usize>, target: usize) -> Result<()> {
        for jump in jumps {
            match self.function.code.get(jump).copied().map(Instr::opcode) {
                Some(
                    Opcode::BrNil
                    | Opcode::BrNotNil
                    | Opcode::BrFalse
                    | Opcode::BrTrue
                    | Opcode::BrEqZeroInt
                    | Opcode::BrNeZeroInt,
                ) => {
                    self.patch_branch(jump, target)?;
                }
                Some(Opcode::BrEqIntI4 | Opcode::BrNeIntI4 | Opcode::BrModEqZeroIntI4 | Opcode::BrModNeZeroIntI4) => {
                    self.patch_i4_branch(jump, target)?
                }
                Some(opcode) if opcode.is_compare_test() => self.patch_compare_test_jump(jump, target)?,
                _ => self.patch_test_false_jump(jump, target)?,
            }
        }
        Ok(())
    }

    fn lower_template_string(&mut self, parts: &[TemplateStringPart]) -> Result<u16> {
        let parts = parts
            .iter()
            .filter(|part| !matches!(part, TemplateStringPart::Literal(value) if value.is_empty()))
            .collect::<Vec<_>>();
        if parts.is_empty() {
            return self.lower_val(&LiteralVal::from_str(""));
        }
        let force_single_expr_string = parts.len() == 1;

        // Use ConcatN when we have 3+ parts and they fit in C operand (max 255).
        // 2-part templates still use ConcatString which is well-supported by LLVM lowering.
        // ConcatN A B C: concatenate values r[B]..r[B+C-1] into r[A]
        if parts.len() >= 3 && parts.len() <= 255 {
            let start_reg = self.alloc_reg();
            // Allocate contiguous registers for the remaining parts
            for _ in 1..parts.len() {
                self.alloc_reg();
            }

            // Lower each part into its register
            for (i, part) in parts.iter().enumerate() {
                let target_reg = start_reg + i as u16;
                self.lower_template_string_part_to_register(target_reg, part, force_single_expr_string)?;
            }

            let dst = self.alloc_reg();
            self.emit(Instr::abc(
                Opcode::ConcatN,
                checked_u8("template concatn dst", dst)?,
                checked_u8("template concatn start", start_reg)?,
                checked_u8("template concatn count", parts.len() as u16)?,
            ));
            self.set_register_kind(dst, PerfValueKind::String);
            return Ok(dst);
        }

        // Fallback: chain ConcatString for 1 part or 255+ parts
        let mut acc = None;
        for part in parts {
            let part_reg = self.lower_template_string_part(part, force_single_expr_string)?;
            let Some(lhs) = acc else {
                acc = Some(part_reg);
                continue;
            };
            let dst = self.alloc_reg();
            self.emit(Instr::abc(
                Opcode::ConcatString,
                checked_u8("template concat dst", dst)?,
                checked_u8("template concat lhs", lhs)?,
                checked_u8("template concat rhs", part_reg)?,
            ));
            self.set_register_kind(dst, PerfValueKind::String);
            acc = Some(dst);
        }
        acc.map_or_else(|| self.lower_val(&LiteralVal::from_str("")), Ok)
    }

    pub(super) fn lower_template_string_to_register(&mut self, dst: u16, parts: &[TemplateStringPart]) -> Result<()> {
        let parts = parts
            .iter()
            .filter(|part| !matches!(part, TemplateStringPart::Literal(value) if value.is_empty()))
            .collect::<Vec<_>>();
        if parts.is_empty() {
            self.emit_literal_to_register(dst, &LiteralVal::from_str(""))?;
            return Ok(());
        }
        let force_single_expr_string = parts.len() == 1;

        if parts.len() == 1 {
            self.lower_template_string_part_to_register(dst, parts[0], force_single_expr_string)?;
            self.set_register_kind(dst, PerfValueKind::String);
            return Ok(());
        }

        if parts.len() >= 3 && parts.len() <= 255 {
            let start_reg = self.alloc_reg();
            for _ in 1..parts.len() {
                self.alloc_reg();
            }
            for (index, part) in parts.iter().enumerate() {
                self.lower_template_string_part_to_register(start_reg + index as u16, part, force_single_expr_string)?;
            }
            self.emit(Instr::abc(
                Opcode::ConcatN,
                checked_u8("template concatn dst", dst)?,
                checked_u8("template concatn start", start_reg)?,
                checked_u8("template concatn count", parts.len() as u16)?,
            ));
            self.set_register_kind(dst, PerfValueKind::String);
            return Ok(());
        }

        let mut acc = self.lower_template_string_part(parts[0], force_single_expr_string)?;
        for (index, part) in parts.iter().enumerate().skip(1) {
            let part_reg = self.lower_template_string_part(part, force_single_expr_string)?;
            let concat_dst = if index == parts.len() - 1 {
                dst
            } else {
                self.alloc_reg()
            };
            self.emit(Instr::abc(
                Opcode::ConcatString,
                checked_u8("template concat dst", concat_dst)?,
                checked_u8("template concat lhs", acc)?,
                checked_u8("template concat rhs", part_reg)?,
            ));
            self.set_register_kind(concat_dst, PerfValueKind::String);
            acc = concat_dst;
        }
        Ok(())
    }

    fn lower_template_string_part(&mut self, part: &TemplateStringPart, force_expr_string: bool) -> Result<u16> {
        match part {
            TemplateStringPart::Literal(value) => self.lower_val(&LiteralVal::from_str(value)),
            TemplateStringPart::Expr(expr) => {
                let value = self.lower_readonly_operand(expr)?;
                if !force_expr_string || self.function.performance.value_kind(value) == PerfValueKind::String {
                    return Ok(value);
                }
                let dst = self.alloc_reg();
                self.emit(Instr::abc(
                    Opcode::ToString,
                    checked_u8("template string dst", dst)?,
                    checked_u8("template string src", value)?,
                    0,
                ));
                self.set_register_kind(dst, PerfValueKind::String);
                Ok(dst)
            }
        }
    }

    fn lower_template_string_part_to_register(
        &mut self,
        dst: u16,
        part: &TemplateStringPart,
        force_expr_string: bool,
    ) -> Result<()> {
        match part {
            TemplateStringPart::Literal(value) => self.emit_literal_to_register(dst, &LiteralVal::from_str(value)),
            TemplateStringPart::Expr(expr) => {
                if !force_expr_string {
                    return self.lower_expr_to_register(dst, expr, "template part");
                }
                let value = self.lower_readonly_operand(expr)?;
                if self.function.performance.value_kind(value) == PerfValueKind::String {
                    let move_source = !self.is_current_local_slot(value);
                    return self.emit_move_with_policy(dst, value, "template string part", move_source);
                }
                self.emit(Instr::abc(
                    Opcode::ToString,
                    checked_u8("template string dst", dst)?,
                    checked_u8("template string src", value)?,
                    0,
                ));
                self.set_register_kind(dst, PerfValueKind::String);
                Ok(())
            }
        }
    }

    fn lower_block_expr(&mut self, statements: &[Box<Stmt>]) -> Result<u16> {
        let mut last = None;
        for stmt in statements {
            match stmt.as_ref() {
                Stmt::Expr(expr) => {
                    last = Some(self.lower_expr(expr)?);
                }
                Stmt::Return { .. } => {
                    self.lower_stmt(stmt)?;
                    let nil = self.alloc_reg();
                    self.emit(Instr::abc(
                        Opcode::LoadNil,
                        checked_u8("block after return", nil)?,
                        0,
                        0,
                    ));
                    return Ok(nil);
                }
                stmt => {
                    self.lower_stmt(stmt)?;
                    if self.emitted_return {
                        let nil = self.alloc_reg();
                        self.emit(Instr::abc(Opcode::LoadNil, checked_u8("block returned", nil)?, 0, 0));
                        return Ok(nil);
                    }
                }
            }
        }
        if let Some(last) = last {
            Ok(last)
        } else {
            let nil = self.alloc_reg();
            self.emit(Instr::abc(Opcode::LoadNil, checked_u8("empty block", nil)?, 0, 0));
            Ok(nil)
        }
    }

    fn lower_closure(&mut self, params: &[String], body: &Expr) -> Result<u16> {
        let captures = self.collect_closure_captures(params, body);
        let function_index = self
            .dynamic_function_base
            .checked_add(self.pending_functions.len() as u32)
            .ok_or_else(|| anyhow!("Compiler dynamic function index overflow"))?;
        let capture_base = self.alloc_regs(captures.len())?;
        let mut capture_names = HashMap::new();
        let mut capture_cells = HashSet::new();
        for (index, name) in captures.iter().enumerate() {
            let (value, is_cell) = self.lower_capture_value(name)?;
            self.emit_move(capture_base + index as u16, value, "closure capture")?;
            capture_names.insert(name.clone(), index as u16);
            if is_cell {
                capture_cells.insert(name.clone());
            }
        }

        let mut compiled =
            self.compile_closure_function(params, body, capture_names, capture_cells, function_index + 1)?;
        let dst = self.alloc_reg();
        self.emit(Instr::abc(
            Opcode::MakeClosure,
            checked_u8("closure dst", dst)?,
            checked_u8("closure function", function_index as u16)?,
            checked_u8("closure capture base", capture_base)?,
        ));
        self.pending_functions.push(compiled.function);
        self.pending_functions.append(&mut compiled.pending_functions);
        Ok(dst)
    }

    fn compile_closure_function(
        &self,
        params: &[String],
        body: &Expr,
        capture_names: HashMap<String, u16>,
        capture_cells: HashSet<String>,
        dynamic_function_base: u32,
    ) -> Result<CompiledFunction> {
        if params.len() > u16::MAX as usize {
            bail!("Compiler closure has too many params: {}", params.len());
        }
        let mut compiler = Self::with_names(
            self.function_names.clone(),
            self.function_signatures.clone(),
            self.function_bodies.clone(),
            self.native_names.clone(),
            self.global_names.clone(),
            false,
        );
        compiler.capture_names = capture_names;
        compiler.capture_cells = capture_cells;
        compiler.dynamic_function_base = dynamic_function_base;
        compiler.function.param_count = params.len() as u16;
        compiler.function.positional_param_count = params.len() as u16;
        compiler.function.param_names = Vec::with_capacity(params.len());
        for name in params {
            compiler.function.param_names.push(Arc::<str>::from(name.as_str()));
        }
        compiler.function.capture_count = compiler.capture_names.len() as u16;
        compiler.next_reg = params.len() as u16;
        compiler.peak_reg = params.len() as u16;
        for (index, param) in params.iter().enumerate() {
            compiler.insert_local(param.clone(), index as u16);
        }
        match body {
            Expr::Block(statements) => {
                for stmt in statements {
                    compiler.lower_stmt(stmt)?;
                    if compiler.emitted_return {
                        break;
                    }
                }
                if !compiler.emitted_return {
                    let nil = compiler.alloc_reg();
                    compiler.emit(Instr::abc(Opcode::LoadNil, checked_u8("dst", nil)?, 0, 0));
                    compiler.emit_return(nil)?;
                }
            }
            body => {
                let value = compiler.lower_expr(body)?;
                compiler.emit_return(value)?;
            }
        }
        Ok(CompiledFunction {
            function: compiler.finish()?,
            pending_functions: compiler.pending_functions,
        })
    }

    fn lower_conditional(&mut self, condition: &Expr, then_expr: &Expr, else_expr: &Expr) -> Result<u16> {
        let dst = self.alloc_reg();
        let false_jumps = self.emit_condition_false_jumps(condition)?;

        self.lower_expr_to_register(dst, then_expr, "conditional then")?;
        let jmp_end = self.emit_jmp_placeholder();

        let else_start = self.function.code.len();
        self.patch_condition_false_jumps(false_jumps, else_start)?;
        self.lower_expr_to_register(dst, else_expr, "conditional else")?;

        let end = self.function.code.len();
        self.patch_jmp(jmp_end, end)?;
        Ok(dst)
    }

    fn materialize_list(&mut self, values: Vec<u16>) -> Result<u16> {
        let len = values.len();
        if len > u8::MAX as usize {
            bail!("Compiler list literal has {} elements, max {}", len, u8::MAX);
        }

        let base = self.alloc_regs(len)?;
        for (offset, value) in values.into_iter().enumerate() {
            let move_source = !self.is_current_local_slot(value);
            self.emit_move_with_policy(base + offset as u16, value, "list element", move_source)?;
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
        Ok(dst)
    }

    fn lower_var(&mut self, name: &str) -> Result<u16> {
        if let Some(src) = self.locals.get(name).copied() {
            if self.cell_locals.contains(name) {
                return self.emit_load_cell_value(src);
            }
            let dst = self.alloc_reg();
            self.emit_move(dst, src, "var")?;
            return Ok(dst);
        }
        if let Some(capture) = self.capture_names.get(name).copied() {
            let cell_or_value = self.emit_load_capture(capture)?;
            if self.capture_cells.contains(name) {
                return self.emit_load_cell_value(cell_or_value);
            }
            return Ok(cell_or_value);
        }
        if let Some(slot) = self.global_names.get(name).copied() {
            return self.emit_get_global(slot);
        }
        Err(anyhow!("Compiler undefined local/global `{name}`"))
    }

    fn lower_if(&mut self, condition: &Expr, then_stmt: &Stmt, else_stmt: Option<&Stmt>) -> Result<()> {
        if self.try_lower_min_max_if(condition, then_stmt, else_stmt)? {
            return Ok(());
        }
        let watermark = self.next_reg;
        let false_jumps = self.emit_condition_false_jumps(condition)?;

        self.emitted_return = false;
        self.local_rebind_suppression += 1;
        self.lower_stmt(then_stmt)?;
        self.local_rebind_suppression -= 1;
        let then_returns = self.emitted_return;
        self.next_reg = watermark; // recycle registers from then-branch

        if let Some(else_stmt) = else_stmt {
            let jmp_end = (!then_returns).then(|| self.emit_jmp_placeholder());
            let else_start = self.function.code.len();
            self.patch_condition_false_jumps(false_jumps, else_start)?;

            self.emitted_return = false;
            self.local_rebind_suppression += 1;
            self.lower_stmt(else_stmt)?;
            self.local_rebind_suppression -= 1;
            let else_returns = self.emitted_return;
            self.next_reg = watermark; // recycle registers from else-branch

            if let Some(jmp_end) = jmp_end {
                let end = self.function.code.len();
                self.patch_jmp(jmp_end, end)?;
            }
            self.emitted_return = then_returns && else_returns;
        } else {
            let end = self.function.code.len();
            self.patch_condition_false_jumps(false_jumps, end)?;
            self.emitted_return = false;
        }

        Ok(())
    }

    fn try_lower_min_max_if(&mut self, condition: &Expr, then_stmt: &Stmt, else_stmt: Option<&Stmt>) -> Result<bool> {
        if else_stmt.is_some() {
            return Ok(false);
        }
        let Some((name, value)) = single_assign_stmt(then_stmt) else {
            return Ok(false);
        };
        if self.cell_locals.contains(name) {
            return Ok(false);
        }
        let Some(dst) = self.locals.get(name).copied() else {
            return Ok(false);
        };
        if self.function.performance.value_kind(dst) != PerfValueKind::Int {
            return Ok(false);
        }
        let Some(opcode) = min_max_update_opcode(condition, name, value) else {
            return Ok(false);
        };
        let candidate = self.lower_readonly_operand(value)?;
        if self.function.performance.value_kind(candidate) != PerfValueKind::Int {
            return Ok(false);
        }
        self.emit(Instr::abc(
            opcode,
            checked_u8("min/max dst", dst)?,
            checked_u8("min/max current", dst)?,
            checked_u8("min/max candidate", candidate)?,
        ));
        self.set_register_kind(dst, PerfValueKind::Int);
        self.emitted_return = false;
        Ok(true)
    }

    /// Promotes every local a closure inside the loop captures to a cell
    /// *now*, before any loop code is emitted. A promotion emitted mid-body
    /// re-executes each iteration (re-boxing an outer variable and orphaning
    /// the shared cell) and leaves earlier-emitted reads — the condition and
    /// increment, re-executed on the back edge — reading the raw register
    /// that meanwhile holds the cell.
    fn pre_promote_loop_captures(&mut self, condition: Option<&Expr>, body: &Stmt) -> Result<()> {
        let mut captured = Vec::new();
        if let Some(condition) = condition {
            collect_expr_closure_captures(condition, &mut captured);
        }
        collect_stmt_closure_captures(body, &mut captured);
        for name in captured {
            // Inside the loop body the pattern names lexically bind the loop
            // variables, so a name-level skip is exact here.
            if self.loop_snapshot_vars.iter().any(|v| v.name == name) {
                continue;
            }
            self.promote_captured_local(&name)?;
        }
        Ok(())
    }

    /// Promotes `name` (if it is a plain local) to a capture cell right now:
    /// box the current value and re-bind the register to the cell.
    pub(super) fn promote_captured_local(&mut self, name: &str) -> Result<()> {
        let Some(local) = self.locals.get(name).copied() else {
            return Ok(());
        };
        if self.cell_locals.insert(name.to_string()) {
            let cell = self.emit_upval_cell(local)?;
            self.emit_move(local, cell, "box captured local")?;
        }
        Ok(())
    }

    fn lower_while(&mut self, condition: &Expr, body: &Stmt) -> Result<()> {
        self.pre_promote_loop_captures(Some(condition), body)?;
        let watermark = self.next_reg;
        self.begin_loop_scalar_const_scope(condition, body)?;
        let condition_start = self.function.code.len();
        let false_jumps = self.emit_condition_false_jumps(condition)?;
        // Scalar constant loads in the condition can run once before the first
        // iteration; loop-back jumps resume at the first real condition op.
        let condition_end = self.function.code.len();
        let loop_start = self.function.code[condition_start..condition_end]
            .iter()
            .enumerate()
            .find_map(|(i, instr)| {
                if !instr.opcode().is_scalar_const_load() {
                    Some(condition_start + i)
                } else {
                    None
                }
            })
            .unwrap_or(condition_start);

        self.loops.push(LoopPatch::default());
        self.emitted_return = false;
        self.local_rebind_suppression += 1;
        self.lower_stmt(body)?;
        self.local_rebind_suppression -= 1;
        let loop_patch = self.loops.pop().expect("loop patch just pushed");
        if !self.emitted_return {
            let jmp_back = self.emit_jmp_placeholder();
            self.patch_jmp(jmp_back, loop_start)?;
        }

        let end = self.function.code.len();
        self.patch_condition_false_jumps(false_jumps, end)?;
        for pc in loop_patch.breaks {
            self.patch_jmp(pc, end)?;
        }
        for pc in loop_patch.continues {
            self.patch_jmp(pc, loop_start)?;
        }
        self.emitted_return = false;
        self.end_loop_scalar_const_scope();
        self.next_reg = watermark; // recycle all loop registers
        Ok(())
    }

    fn lower_for(&mut self, pattern: &ForPattern, iterable: &Expr, body: &Stmt) -> Result<()> {
        // The pattern names register *before* the body prescan: inside the
        // body they lexically refer to the loop variables, so the prescan
        // must not pre-promote a same-named outer local.
        let snapshot_mark = self.loop_snapshot_vars.len();
        collect_for_pattern_names(pattern, &mut self.loop_snapshot_vars);
        let result = self
            .pre_promote_loop_captures(None, body)
            .and_then(|()| self.lower_for_dispatch(pattern, iterable, body));
        self.loop_snapshot_vars.truncate(snapshot_mark);
        result
    }

    fn lower_for_dispatch(&mut self, pattern: &ForPattern, iterable: &Expr, body: &Stmt) -> Result<()> {
        match iterable {
            Expr::Range {
                start,
                end,
                inclusive,
                step,
            } => self.lower_for_range(
                pattern,
                start.as_deref(),
                end.as_deref(),
                *inclusive,
                step.as_deref(),
                body,
            ),
            iterable => self.lower_for_indexed(pattern, iterable, body),
        }
    }

    fn lower_for_indexed(&mut self, pattern: &ForPattern, iterable: &Expr, body: &Stmt) -> Result<()> {
        let watermark = self.next_reg;
        let iterable_value = self.lower_readonly_access_target(iterable)?;
        let iterable_kind = self.function.performance.value_kind(iterable_value);
        let direct_iterable = matches!(iterable_kind, PerfValueKind::List | PerfValueKind::String);
        let iterable = if direct_iterable {
            iterable_value
        } else {
            let iterable = self.alloc_reg();
            self.emit(Instr::abc(
                Opcode::ToIter,
                checked_u8("for indexed iter dst", iterable)?,
                checked_u8("for indexed iter src", iterable_value)?,
                0,
            ));
            self.set_register_kind(iterable, PerfValueKind::List);
            iterable
        };
        let len = self.alloc_reg();
        self.emit(Instr::abc(
            Opcode::Len,
            checked_u8("for indexed len dst", len)?,
            checked_u8("for indexed iterable", iterable)?,
            0,
        ));
        self.set_register_kind(len, PerfValueKind::Int);
        let index = self.lower_val(&LiteralVal::Int(0))?;
        let step = self.lower_val(&LiteralVal::Int(1))?;
        let skip_value_load = matches!(iterable_kind, PerfValueKind::String)
            && matches!(pattern, ForPattern::Variable(name) if !stmt_uses_for_binding_value(body, name) && !stmt_shadows_name_deep(body, name));
        let value = if skip_value_load { step } else { self.alloc_reg() };

        let loop_start = self.function.code.len();
        let condition = self.alloc_reg();
        self.emit(Instr::abc(
            Opcode::CmpLtInt,
            checked_u8("for indexed condition dst", condition)?,
            checked_u8("for indexed index", index)?,
            checked_u8("for indexed len", len)?,
        ));
        let exit_test = self.emit_test_placeholder(condition)?;
        if !skip_value_load {
            self.emit(Instr::abc(
                Opcode::GetIndex,
                checked_u8("for indexed value", value)?,
                checked_u8("for indexed iterable", iterable)?,
                checked_u8("for indexed index", index)?,
            ));
            if let Some(fact) = index_fact_from_target(&self.function.performance, iterable) {
                let pc = self.function.code.len() - 1;
                self.function.performance.set_index_fact(pc, fact);
            }
        }
        let previous_binding = self.bind_for_pattern(pattern, value)?;
        let previous_single_char_locals = self.single_char_string_locals.clone();
        if matches!(iterable_kind, PerfValueKind::String)
            && let ForPattern::Variable(name) = pattern
        {
            self.single_char_string_locals.insert(name.clone(), step);
        }

        self.loops.push(LoopPatch::default());
        self.emitted_return = false;
        self.local_rebind_suppression += 1;
        self.lower_stmt(body)?;
        self.local_rebind_suppression -= 1;
        let loop_patch = self.loops.pop().expect("loop patch just pushed");

        let step_start = self.function.code.len();
        if !self.emitted_return {
            self.emit_bin_op_to_register(index, &BinOp::Add, index, step)?;
            let jmp_back = self.emit_jmp_placeholder();
            self.patch_jmp(jmp_back, loop_start)?;
        }

        let loop_end = self.function.code.len();
        self.patch_test_false_jump(exit_test, loop_end)?;
        for pc in loop_patch.breaks {
            self.patch_jmp(pc, loop_end)?;
        }
        for pc in loop_patch.continues {
            self.patch_jmp(pc, step_start)?;
        }
        self.restore_for_pattern(previous_binding);
        self.single_char_string_locals = previous_single_char_locals;
        self.emitted_return = false;
        self.next_reg = watermark; // recycle all loop registers
        Ok(())
    }

    fn lower_for_range(
        &mut self,
        pattern: &ForPattern,
        start: Option<&Expr>,
        end: Option<&Expr>,
        inclusive: bool,
        step: Option<&Expr>,
        body: &Stmt,
    ) -> Result<()> {
        let watermark = self.next_reg;
        self.begin_loop_scalar_const_scope_for_exprs(&[], body)?;
        let step_sign = range_step_sign(step);
        let index = self.alloc_reg();
        match start {
            Some(start) => self.lower_expr_to_register(index, start, "for range initial index")?,
            None => self.emit_literal_to_register(index, &LiteralVal::Int(0))?,
        }
        let end = end.ok_or_else(|| anyhow!("Compiler open-ended range for loop is not supported"))?;
        let body_mutations = mutated_names_in_stmt(body);
        let end = self.lower_loop_snapshot_operand(end, &body_mutations)?;

        let step = match step {
            Some(step) => self.lower_loop_snapshot_operand(step, &body_mutations)?,
            None => self.lower_val(&LiteralVal::Int(1))?,
        };

        let previous_binding = self.bind_for_pattern(pattern, index)?;

        match step_sign {
            RangeStepSign::Positive => self.lower_for_range_static_loop(index, end, step, inclusive, true, body)?,
            RangeStepSign::Negative => self.lower_for_range_static_loop(index, end, step, inclusive, false, body)?,
            RangeStepSign::Dynamic => self.lower_for_range_dynamic_loop(index, end, step, inclusive, body)?,
        }

        self.restore_for_pattern(previous_binding);
        self.emitted_return = false;
        self.end_loop_scalar_const_scope();
        self.next_reg = watermark; // recycle all loop registers
        Ok(())
    }

    fn bind_for_pattern(&mut self, pattern: &ForPattern, value: u16) -> Result<Vec<ForPatternBinding>> {
        let mut previous = Vec::new();
        self.bind_for_pattern_inner(pattern, value, &mut previous)?;
        Ok(previous)
    }

    /// A loop binding is fresh (never a cell), so binding clears any stale
    /// cell mark of a same-named outer local; the restore re-instates both
    /// the previous slot and its mark.
    fn bind_for_name(&mut self, name: &str, value: u16, previous: &mut Vec<ForPatternBinding>) {
        let was_cell = self.cell_locals.contains(name);
        previous.push(ForPatternBinding {
            name: name.to_string(),
            slot: self.insert_fresh_local(name.to_string(), value),
            was_cell,
        });
        // Fill the innermost pending snapshot entry: captures and re-`let`s
        // recognize the loop variable by this slot, not by name alone.
        if let Some(entry) = self
            .loop_snapshot_vars
            .iter_mut()
            .rev()
            .find(|entry| entry.name == name && entry.slot.is_none())
        {
            entry.slot = Some(value);
        }
    }

    /// The binding slot of the innermost enclosing `for` variable named
    /// `name`, if that loop has already bound its pattern.
    pub(super) fn active_loop_binding_slot(&self, name: &str) -> Option<u16> {
        self.loop_snapshot_vars
            .iter()
            .rev()
            .find(|entry| entry.name == name)
            .and_then(|entry| entry.slot)
    }

    fn bind_for_pattern_inner(
        &mut self,
        pattern: &ForPattern,
        value: u16,
        previous: &mut Vec<ForPatternBinding>,
    ) -> Result<()> {
        match pattern {
            ForPattern::Variable(name) => {
                self.bind_for_name(name, value, previous);
                Ok(())
            }
            ForPattern::Ignore => Ok(()),
            ForPattern::Tuple(patterns) => {
                let condition = self.lower_list_pattern_condition(value, patterns.len())?;
                self.emit_pattern_assert(condition)?;
                self.bind_for_sequence_pattern(patterns, value, previous)
            }
            ForPattern::Array { patterns, rest: None } => {
                let condition = self.lower_list_pattern_condition(value, patterns.len())?;
                self.emit_pattern_assert(condition)?;
                self.bind_for_sequence_pattern(patterns, value, previous)
            }
            ForPattern::Array {
                patterns,
                rest: Some(rest),
            } => {
                let condition = self.lower_list_pattern_condition(value, patterns.len())?;
                self.emit_pattern_assert(condition)?;
                self.bind_for_sequence_pattern(patterns, value, previous)?;
                let start = self.lower_val(&LiteralVal::Int(patterns.len() as i64))?;
                let slice = self.alloc_reg();
                self.emit(Instr::abc(
                    Opcode::SliceFrom,
                    checked_u8("for rest slice", slice)?,
                    checked_u8("for rest value", value)?,
                    checked_u8("for rest start", start)?,
                ));
                self.bind_for_name(rest, slice, previous);
                Ok(())
            }
            ForPattern::Object(entries) => {
                let condition =
                    self.lower_map_pattern_key_condition(value, entries.iter().map(|(key, _)| key.as_str()))?;
                self.emit_pattern_assert(condition)?;
                for (key, pattern) in entries {
                    let key = self.lower_val(&LiteralVal::from_str(key))?;
                    let field = self.alloc_reg();
                    self.emit(Instr::abc(
                        Opcode::GetIndex,
                        checked_u8("for object field", field)?,
                        checked_u8("for object value", value)?,
                        checked_u8("for object key", key)?,
                    ));
                    self.bind_for_pattern_inner(pattern, field, previous)?;
                }
                Ok(())
            }
        }
    }

    fn bind_for_sequence_pattern(
        &mut self,
        patterns: &[ForPattern],
        value: u16,
        previous: &mut Vec<ForPatternBinding>,
    ) -> Result<()> {
        for (index, pattern) in patterns.iter().enumerate() {
            let index = i64::try_from(index).map_err(|_| anyhow!("Compiler for pattern index overflow"))?;
            let key = self.lower_val(&LiteralVal::Int(index))?;
            let field = self.alloc_reg();
            self.emit(Instr::abc(
                Opcode::GetIndex,
                checked_u8("for sequence field", field)?,
                checked_u8("for sequence value", value)?,
                checked_u8("for sequence index", key)?,
            ));
            self.bind_for_pattern_inner(pattern, field, previous)?;
        }
        Ok(())
    }

    fn restore_for_pattern(&mut self, previous: Vec<ForPatternBinding>) {
        for binding in previous.into_iter().rev() {
            if let Some(old) = binding.slot {
                self.insert_local(binding.name.clone(), old);
            } else {
                self.locals.remove(&binding.name);
            }
            if binding.was_cell {
                self.cell_locals.insert(binding.name);
            }
        }
    }

    fn lower_break(&mut self) -> Result<()> {
        let pc = self.emit_jmp_placeholder();
        let Some(loop_patch) = self.loops.last_mut() else {
            bail!("break statement outside of loop");
        };
        loop_patch.breaks.push(pc);
        Ok(())
    }

    fn lower_continue(&mut self) -> Result<()> {
        let pc = self.emit_jmp_placeholder();
        let Some(loop_patch) = self.loops.last_mut() else {
            bail!("continue statement outside of loop");
        };
        loop_patch.continues.push(pc);
        Ok(())
    }

    fn lower_function_decl(&mut self, name: &str) -> Result<()> {
        let function = self.load_function_by_name(name)?;
        if self.top_level
            && let Some(slot) = self.global_names.get(name).copied()
        {
            self.emit_set_global(function, slot)?;
            return Ok(());
        }
        self.insert_local(name.to_string(), function);
        Ok(())
    }

    fn lower_trait_decl(&mut self, name: &str, methods: &[(String, Type)]) -> Result<()> {
        let Some(helper) = self.try_load_callable_by_name("__lk_register_trait")? else {
            return Ok(());
        };
        let name = self.lower_val(&LiteralVal::from_str(name))?;
        let mut entries = Vec::with_capacity(methods.len());
        for (method_name, method_type) in methods {
            let method_name = self.lower_val(&LiteralVal::from_str(method_name))?;
            let method_type = self.lower_val(&LiteralVal::from_str(&method_type.display()))?;
            entries.push(self.materialize_list(vec![method_name, method_type])?);
        }
        let methods = self.materialize_list(entries)?;
        self.lower_call_window_regs(helper, &[name, methods])?;
        Ok(())
    }

    fn lower_impl_decl(&mut self, trait_name: &str, target_type: &Type, methods: &[Stmt]) -> Result<()> {
        let Some(helper) = self.try_load_callable_by_name("__lk_register_trait_impl")? else {
            return Ok(());
        };
        let trait_name = self.lower_val(&LiteralVal::from_str(trait_name))?;
        let target_type_text = target_type.display();
        let target_type_reg = self.lower_val(&LiteralVal::from_str(&target_type_text))?;
        let mut entries = Vec::with_capacity(methods.len());
        for method in methods {
            let Stmt::Function {
                name,
                params,
                param_types,
                named_params,
                return_type,
                body,
            } = method
            else {
                bail!("Compiler impl block only supports function methods");
            };
            let method_name = self.lower_val(&LiteralVal::from_str(name))?;
            let method_value = self.compile_impl_method_function(params, named_params, body)?;
            let method_type = impl_method_type(target_type, params, param_types, named_params, return_type);
            let method_type = self.lower_val(&LiteralVal::from_str(&method_type.display()))?;
            entries.push(self.materialize_list(vec![method_name, method_value, method_type])?);
        }
        let methods = self.materialize_list(entries)?;
        self.lower_call_window_regs(helper, &[trait_name, target_type_reg, methods])?;
        Ok(())
    }

    fn compile_impl_method_function(
        &mut self,
        params: &[String],
        named_params: &[crate::stmt::NamedParamDecl],
        body: &Stmt,
    ) -> Result<u16> {
        let function_index = self
            .dynamic_function_base
            .checked_add(self.pending_functions.len() as u32)
            .ok_or_else(|| anyhow!("Compiler dynamic impl method index overflow"))?;
        let mut compiled = Self::compile_function_body(
            params,
            named_params,
            body,
            self.function_names.clone(),
            self.function_signatures.clone(),
            self.function_bodies.clone(),
            self.native_names.clone(),
            self.global_names.clone(),
            HashMap::new(),
            function_index + 1,
        )?;
        let dst = self.alloc_reg();
        self.emit(Instr::abx(
            Opcode::LoadFunction,
            checked_u8("impl method function dst", dst)?,
            u16::try_from(function_index)
                .map_err(|_| anyhow!("Compiler impl method index {function_index} exceeds u16"))?,
        ));
        self.function.performance.set_register_fact(
            dst,
            PerfRegisterFact {
                callable: PerfCallTargetKind::Closure,
                ..PerfRegisterFact::default()
            },
        );
        self.pending_functions.push(compiled.function);
        self.pending_functions.append(&mut compiled.pending_functions);
        Ok(dst)
    }

    fn load_callable_by_name(&mut self, name: &str) -> Result<u16> {
        self.try_load_callable_by_name(name)?
            .ok_or_else(|| anyhow!("Compiler undefined callable `{name}`"))
    }

    fn try_load_callable_by_name(&mut self, name: &str) -> Result<Option<u16>> {
        if self.function_names.contains_key(name) {
            return self.load_function_by_name(name).map(Some);
        }
        if self.native_names.contains_key(name) {
            return self.load_native_by_name(name).map(Some);
        }
        if let Some(slot) = self.global_names.get(name).copied() {
            return self.emit_get_global(slot).map(Some);
        }
        Ok(None)
    }

    fn load_function_by_name(&mut self, name: &str) -> Result<u16> {
        let function_index = *self
            .function_names
            .get(name)
            .ok_or_else(|| anyhow!("Compiler undefined function `{name}`"))?;
        let dst = self.alloc_reg();
        let function_index = u16::try_from(function_index)
            .map_err(|_| anyhow!("Compiler function index {function_index} exceeds u16"))?;
        self.emit(Instr::abx(
            Opcode::LoadFunction,
            checked_u8("function dst", dst)?,
            function_index,
        ));
        self.function.performance.set_register_fact(
            dst,
            PerfRegisterFact {
                callable: PerfCallTargetKind::Closure,
                ..PerfRegisterFact::default()
            },
        );
        Ok(dst)
    }

    fn load_native_by_name(&mut self, name: &str) -> Result<u16> {
        let native_index = *self
            .native_names
            .get(name)
            .ok_or_else(|| anyhow!("Compiler undefined native `{name}`"))?;
        let dst = self.alloc_reg();
        let native_index =
            u16::try_from(native_index).map_err(|_| anyhow!("Compiler native index {native_index} exceeds u16"))?;
        self.emit(Instr::abx(
            Opcode::LoadNative,
            checked_u8("native dst", dst)?,
            native_index,
        ));
        self.function.performance.set_register_fact(
            dst,
            PerfRegisterFact {
                callable: PerfCallTargetKind::Native,
                ..PerfRegisterFact::default()
            },
        );
        Ok(dst)
    }

    fn emit_get_global(&mut self, slot: u32) -> Result<u16> {
        let dst = self.alloc_reg();
        let slot = u16::try_from(slot).map_err(|_| anyhow!("Compiler global slot {slot} exceeds u16"))?;
        let pc = self.function.code.len();
        self.emit(Instr::abx(Opcode::GetGlobal, checked_u8("global dst", dst)?, slot));
        self.function.performance.set_global_fact(
            pc,
            PerfGlobalFact {
                slot,
                move_source: false,
            },
        );
        self.function.performance.clear_register(dst);
        Ok(dst)
    }

    fn emit_load_capture(&mut self, capture: u16) -> Result<u16> {
        let dst = self.alloc_reg();
        self.emit(Instr::abx(
            Opcode::LoadCapture,
            checked_u8("capture dst", dst)?,
            capture,
        ));
        self.function.performance.clear_register(dst);
        Ok(dst)
    }

    fn emit_load_cell_value(&mut self, cell: u16) -> Result<u16> {
        let dst = self.alloc_reg();
        self.emit(Instr::abc(
            Opcode::LoadCellVal,
            checked_u8("cell value dst", dst)?,
            checked_u8("cell value src", cell)?,
            0,
        ));
        self.function.performance.clear_register(dst);
        Ok(dst)
    }

    fn lower_capture_value(&mut self, name: &str) -> Result<(u16, bool)> {
        if let Some(local) = self.locals.get(name).copied() {
            // A `for` loop variable cannot be re-bound to a cell (the fused
            // loop opcodes drive the raw register): each capture snapshots
            // the current value into a fresh cell — per-iteration binding.
            // Only the loop's own binding slot qualifies: a same-named fresh
            // `let` in the body is an ordinary local and promotes normally.
            if self.active_loop_binding_slot(name) == Some(local) {
                let cell = self.emit_upval_cell_with_policy(local, false)?;
                return Ok((cell, true));
            }
            if self.cell_locals.insert(name.to_string()) {
                let cell = self.emit_upval_cell(local)?;
                self.emit_move(local, cell, "box captured local")?;
            }
            return Ok((local, true));
        }
        if let Some(capture) = self.capture_names.get(name).copied() {
            let value = self.emit_load_capture(capture)?;
            return Ok((value, self.capture_cells.contains(name)));
        }
        let value = self.lower_var(name)?;
        Ok((value, false))
    }

    fn emit_upval_cell(&mut self, src: u16) -> Result<u16> {
        self.emit_upval_cell_with_policy(src, true)
    }

    /// `move_value: false` keeps `src` intact — the snapshot capture of a
    /// loop variable copies the counter into the cell (the fused loop opcode
    /// keeps driving the raw register afterwards).
    fn emit_upval_cell_with_policy(&mut self, src: u16, move_value: bool) -> Result<u16> {
        let dst = self.alloc_reg();
        let k = self.push_heap_value(ConstHeapValue::UpvalCell(Box::new(ConstRuntimeValue::Nil)))?;
        self.emit(Instr::abx(Opcode::LoadHeapConst, checked_u8("upval cell dst", dst)?, k));
        self.emit_store_cell_value_with_policy(dst, src, "upval cell", move_value)?;
        Ok(dst)
    }

    fn emit_set_global(&mut self, src: u16, slot: u32) -> Result<()> {
        self.emit_set_global_with_policy(src, slot, false)
    }

    pub(super) fn emit_set_global_with_policy(&mut self, src: u16, slot: u32, move_source: bool) -> Result<()> {
        let slot = u16::try_from(slot).map_err(|_| anyhow!("Compiler global slot {slot} exceeds u16"))?;
        let pc = self.function.code.len();
        self.emit(Instr::abx(Opcode::SetGlobal, checked_u8("global src", src)?, slot));
        self.function
            .performance
            .set_global_fact(pc, PerfGlobalFact { slot, move_source });
        Ok(())
    }

    fn collect_closure_captures(&self, params: &[String], body: &Expr) -> Vec<String> {
        let mut bound = HashSet::with_capacity(params.len());
        for param in params {
            bound.insert(param.clone());
        }
        let mut free = Vec::new();
        collect_expr_free_vars(body, &mut bound, &mut free);
        let mut seen = HashSet::new();
        let mut captures = Vec::new();
        for name in free {
            let captures_local = self.locals.contains_key(&name);
            let captures_outer = self.capture_names.contains_key(&name) && !self.global_names.contains_key(&name);
            if (captures_local || captures_outer)
                && !self.function_names.contains_key(&name)
                && !self.native_names.contains_key(&name)
                && seen.insert(name.clone())
            {
                captures.push(name);
            }
        }
        captures
    }

    fn lower_bin(&mut self, lhs: &Expr, op: &BinOp, rhs: &Expr) -> Result<u16> {
        let static_flavor = numeric_flavor(lhs, op, rhs);
        if static_flavor == NumericFlavor::Int
            && let Some(immediate) = support::commuted_int_immediate_operand(op, lhs)
        {
            let rhs = self.lower_readonly_operand(rhs)?;
            if self.function.performance.value_kind(rhs) == PerfValueKind::Int {
                let dst = self.alloc_reg();
                return self.emit_int_immediate_to_register(dst, op, rhs, immediate);
            }
        }
        let lhs = self.lower_readonly_operand(lhs)?;
        if let Some(immediate) = int_immediate_operand(op, rhs) {
            let dst = self.alloc_reg();
            let flavor = if self.function.performance.value_kind(lhs) == PerfValueKind::Int {
                Some(NumericFlavor::Int)
            } else {
                None
            };
            if flavor == Some(static_flavor) {
                return self.emit_int_immediate_to_register(dst, op, lhs, immediate);
            }
        }
        let rhs = self.lower_readonly_operand(rhs)?;
        let dst = self.alloc_reg();
        let flavor =
            numeric_flavor_from_register_facts(&self.function.performance, op, lhs, rhs).unwrap_or(static_flavor);
        self.emit_bin_op_to_register_with_flavor(dst, op, lhs, rhs, flavor)
    }

    fn emit_bin_op_to_register(&mut self, dst: u16, op: &BinOp, lhs: u16, rhs: u16) -> Result<u16> {
        let flavor =
            numeric_flavor_from_register_facts(&self.function.performance, op, lhs, rhs).unwrap_or(NumericFlavor::Int);
        self.emit_bin_op_to_register_with_flavor(dst, op, lhs, rhs, flavor)
    }

    fn emit_bin_op_to_register_with_flavor(
        &mut self,
        dst: u16,
        op: &BinOp,
        lhs: u16,
        rhs: u16,
        flavor: NumericFlavor,
    ) -> Result<u16> {
        let opcode = match op {
            BinOp::Add => match flavor {
                NumericFlavor::Int => Opcode::AddInt,
                NumericFlavor::Float => Opcode::AddFloat,
            },
            BinOp::Sub => match flavor {
                NumericFlavor::Int => Opcode::SubInt,
                NumericFlavor::Float => Opcode::SubFloat,
            },
            BinOp::Mul => match flavor {
                NumericFlavor::Int => Opcode::MulInt,
                NumericFlavor::Float => Opcode::MulFloat,
            },
            BinOp::Div => match flavor {
                NumericFlavor::Int => Opcode::DivInt,
                NumericFlavor::Float => Opcode::DivFloat,
            },
            BinOp::Mod => match flavor {
                NumericFlavor::Int => Opcode::ModInt,
                NumericFlavor::Float => Opcode::ModFloat,
            },
            BinOp::Eq => Opcode::CmpInt,
            BinOp::Ne => Opcode::CmpNeInt,
            BinOp::Lt => Opcode::CmpLtInt,
            BinOp::Le => Opcode::CmpLeInt,
            BinOp::Gt => Opcode::CmpGtInt,
            BinOp::Ge => Opcode::CmpGeInt,
            BinOp::In => Opcode::Contains,
        };
        self.emit(Instr::abc(
            opcode,
            checked_u8("dst", dst)?,
            checked_u8("lhs", lhs)?,
            checked_u8("rhs", rhs)?,
        ));
        self.set_register_kind(dst, bin_op_result_kind(op, flavor));
        Ok(dst)
    }

    fn emit_int_immediate_to_register(&mut self, dst: u16, op: &BinOp, lhs: u16, immediate: i8) -> Result<u16> {
        let opcode = match op {
            BinOp::Add | BinOp::Sub => Opcode::AddIntI,
            BinOp::Mul => Opcode::MulIntI,
            BinOp::Mod => Opcode::ModIntI,
            _ => unreachable!("int immediate operand only supports arithmetic ops"),
        };
        self.emit(Instr::abc(
            opcode,
            checked_u8("dst", dst)?,
            checked_u8("lhs", lhs)?,
            immediate as u8,
        ));
        self.set_register_kind(dst, PerfValueKind::Int);
        Ok(dst)
    }
}

fn impl_method_type(
    target_type: &Type,
    params: &[String],
    param_types: &[Option<Type>],
    named_params: &[crate::stmt::NamedParamDecl],
    return_type: &Option<Type>,
) -> Type {
    let params = params
        .iter()
        .enumerate()
        .map(|(index, name)| {
            param_types
                .get(index)
                .and_then(Clone::clone)
                .unwrap_or_else(|| if name == "self" { target_type.clone() } else { Type::Any })
        })
        .collect();
    let named_params = named_params
        .iter()
        .map(|param| FunctionNamedParamType {
            name: param.name.clone(),
            ty: param.type_annotation.clone().unwrap_or(Type::Any),
            has_default: param.default.is_some(),
        })
        .collect();
    Type::Function {
        params,
        named_params,
        return_type: Box::new(return_type.clone().unwrap_or(Type::Any)),
    }
}

fn expr_is_nil_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Paren(inner) => expr_is_nil_literal(inner),
        Expr::Literal(LiteralVal::Nil) => true,
        _ => false,
    }
}

fn get_field_key(
    index_fact: Option<crate::vm::analysis::PerfIndexFact>,
    key_fact: Option<crate::vm::analysis::PerfKeyFact>,
) -> Option<u16> {
    let index_fact = index_fact?;
    if !matches!(
        index_fact.target_kind,
        crate::vm::analysis::PerfIndexTargetKind::Map | crate::vm::analysis::PerfIndexTargetKind::Object
    ) {
        return None;
    }
    let key = key_fact?.const_key?;
    (key <= u8::MAX as u16).then_some(key)
}

fn string_int_template_key(expr: &Expr) -> Option<(&str, &Expr)> {
    let Expr::TemplateString(parts) = strip_expr_parens(expr) else {
        return None;
    };
    let parts = parts
        .iter()
        .filter(|part| !matches!(part, TemplateStringPart::Literal(value) if value.is_empty()))
        .collect::<Vec<_>>();
    match parts.as_slice() {
        [TemplateStringPart::Expr(expr)] => Some(("", strip_expr_parens(expr))),
        [TemplateStringPart::Literal(prefix), TemplateStringPart::Expr(expr)] => {
            Some((prefix.as_str(), strip_expr_parens(expr)))
        }
        _ => None,
    }
}

fn string_int_key_suffix_is_int_like(
    expr: &Expr,
    locals: &crate::compat::collections::HashMap<String, u16>,
    facts: &crate::vm::analysis::PerformanceFacts,
) -> bool {
    match strip_expr_parens(expr) {
        Expr::Literal(LiteralVal::Int(_)) => true,
        Expr::Var(name) => locals
            .get(name)
            .is_some_and(|reg| facts.value_kind(*reg) == PerfValueKind::Int),
        Expr::Bin(lhs, op, rhs) if op.is_arith() => {
            string_int_key_suffix_is_int_like(lhs, locals, facts)
                && string_int_key_suffix_is_int_like(rhs, locals, facts)
        }
        _ => false,
    }
}

fn strip_expr_parens(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(inner) => strip_expr_parens(inner),
        other => other,
    }
}

fn list_int_key(
    index_fact: Option<crate::vm::analysis::PerfIndexFact>,
    facts: &crate::vm::analysis::PerformanceFacts,
    key: u16,
) -> bool {
    ENABLE_GET_LIST_LOWERING
        && index_fact.is_some_and(|fact| fact.target_kind == crate::vm::analysis::PerfIndexTargetKind::List)
        && facts.value_kind(key) == crate::vm::analysis::PerfValueKind::Int
}

fn default_assign_candidate(stmt: &Stmt) -> Option<(&str, &Expr, bool)> {
    match stmt {
        Stmt::Let {
            pattern: Pattern::Variable(name),
            value,
            is_const,
            ..
        } if !*is_const => Some((name.as_str(), value, true)),
        Stmt::Assign { name, value, .. } => Some((name.as_str(), value, false)),
        _ => None,
    }
}

fn pure_default_expr(expr: &Expr) -> bool {
    matches!(strip_parens(expr), Expr::Literal(_) | Expr::Var(_))
}

fn expr_mentions_name(expr: &Expr, name: &str) -> bool {
    let mut free = Vec::new();
    collect_expr_free_vars(expr, &mut HashSet::new(), &mut free);
    free.iter().any(|candidate| candidate == name)
}

fn if_chain_assigns_only_target(stmt: &Stmt, name: &str) -> bool {
    let Stmt::If {
        then_stmt, else_stmt, ..
    } = stmt
    else {
        return false;
    };
    let Some((assigned, value)) = single_assign_stmt(then_stmt) else {
        return false;
    };
    if assigned != name || expr_mentions_name(value, name) {
        return false;
    }
    match else_stmt.as_deref() {
        None => true,
        Some(nested @ Stmt::If { .. }) => if_chain_assigns_only_target(nested, name),
        Some(_) => false,
    }
}

fn if_chain_condition_mentions_name(stmt: &Stmt, name: &str) -> bool {
    let Stmt::If {
        condition, else_stmt, ..
    } = stmt
    else {
        return false;
    };
    expr_mentions_name(condition, name)
        || else_stmt
            .as_deref()
            .is_some_and(|nested| if_chain_condition_mentions_name(nested, name))
}

fn single_assign_stmt(stmt: &Stmt) -> Option<(&str, &Expr)> {
    match stmt {
        Stmt::Assign { name, value, .. } => Some((name.as_str(), value)),
        Stmt::Block { statements } if statements.len() == 1 => single_assign_stmt(&statements[0]),
        _ => None,
    }
}

fn min_max_update_opcode(condition: &Expr, assigned_name: &str, value: &Expr) -> Option<Opcode> {
    let Expr::Bin(lhs, op, rhs) = strip_parens(condition) else {
        return None;
    };
    let value_name = local_expr_name(value)?;
    match op {
        BinOp::Lt if local_expr_name(lhs)? == value_name && local_expr_name(rhs)? == assigned_name => {
            Some(Opcode::MinInt)
        }
        BinOp::Gt if local_expr_name(lhs)? == value_name && local_expr_name(rhs)? == assigned_name => {
            Some(Opcode::MaxInt)
        }
        BinOp::Gt if local_expr_name(rhs)? == value_name && local_expr_name(lhs)? == assigned_name => {
            Some(Opcode::MinInt)
        }
        BinOp::Lt if local_expr_name(rhs)? == value_name && local_expr_name(lhs)? == assigned_name => {
            Some(Opcode::MaxInt)
        }
        _ => None,
    }
}

fn local_expr_name(expr: &Expr) -> Option<&str> {
    match strip_parens(expr) {
        Expr::Var(name) => Some(name.as_str()),
        _ => None,
    }
}

fn strip_parens(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(inner) => strip_parens(inner),
        _ => expr,
    }
}

const ENABLE_GET_LIST_LOWERING: bool = true;
const ENABLE_COMPARE_TEST_LOWERING: bool = true;
const ENABLE_COMPARE_TEST_IMMEDIATE_LOWERING: bool = true;
const ENABLE_COMPARE_TEST_PAIR_IMMEDIATE_LOWERING: bool = true;

fn compare_test_opcode(op: &BinOp) -> Option<Opcode> {
    match op {
        BinOp::Eq => Some(Opcode::TestEqInt),
        BinOp::Ne => Some(Opcode::TestNeInt),
        BinOp::Lt => Some(Opcode::TestLtInt),
        BinOp::Le => Some(Opcode::TestLeInt),
        BinOp::Gt => Some(Opcode::TestGtInt),
        BinOp::Ge => Some(Opcode::TestGeInt),
        _ => None,
    }
}

fn compare_test_immediate_opcode(op: &BinOp) -> Option<Opcode> {
    match op {
        BinOp::Eq => Some(Opcode::TestEqIntI),
        BinOp::Ne => Some(Opcode::TestNeIntI),
        BinOp::Lt => Some(Opcode::TestLtIntI),
        BinOp::Le => Some(Opcode::TestLeIntI),
        BinOp::Gt => Some(Opcode::TestGtIntI),
        BinOp::Ge => Some(Opcode::TestGeIntI),
        _ => None,
    }
}

fn reverse_compare_test_immediate_opcode(op: &BinOp) -> Option<Opcode> {
    match op {
        BinOp::Eq => Some(Opcode::TestEqIntI),
        BinOp::Ne => Some(Opcode::TestNeIntI),
        BinOp::Lt => Some(Opcode::TestGtIntI),
        BinOp::Le => Some(Opcode::TestGeIntI),
        BinOp::Gt => Some(Opcode::TestLtIntI),
        BinOp::Ge => Some(Opcode::TestLeIntI),
        _ => None,
    }
}

fn compare_test_immediate_operand(expr: &Expr) -> Option<i8> {
    match expr {
        Expr::Paren(inner) => compare_test_immediate_operand(inner),
        Expr::Literal(LiteralVal::Int(value)) => i8::try_from(*value).ok(),
        _ => None,
    }
}

fn zero_int_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Paren(inner) => zero_int_literal(inner),
        Expr::Literal(LiteralVal::Int(0)) => true,
        _ => false,
    }
}

fn equality_u4_local_immediate(expr: &Expr) -> Option<(&str, u8)> {
    let Expr::Bin(lhs, BinOp::Eq, rhs) = expr else {
        return None;
    };
    if let Some(name) = simple_local_expr_name(lhs)
        && let Some(value) = u4_literal(rhs)
    {
        return Some((name, value));
    }
    if let Some(name) = simple_local_expr_name(rhs)
        && let Some(value) = u4_literal(lhs)
    {
        return Some((name, value));
    }
    None
}

fn mod_i4_zero_operands<'a>(lhs: &'a Expr, rhs: &'a Expr) -> Option<(&'a Expr, u8)> {
    if zero_int_literal(rhs)
        && let Some(candidate) = mod_i4_operand(lhs)
    {
        return Some(candidate);
    }
    if zero_int_literal(lhs)
        && let Some(candidate) = mod_i4_operand(rhs)
    {
        return Some(candidate);
    }
    None
}

fn mod_i4_operand(expr: &Expr) -> Option<(&Expr, u8)> {
    let Expr::Bin(lhs, BinOp::Mod, rhs) = strip_parens(expr) else {
        return None;
    };
    let divisor = u4_literal(rhs).filter(|value| *value != 0)?;
    Some((lhs.as_ref(), divisor))
}

fn u4_literal(expr: &Expr) -> Option<u8> {
    match expr {
        Expr::Paren(inner) => u4_literal(inner),
        Expr::Literal(LiteralVal::Int(value)) => u8::try_from(*value).ok().filter(|value| *value < 16),
        _ => None,
    }
}

fn compare_test_operands_are_int(facts: &crate::vm::analysis::PerformanceFacts, lhs: u16, rhs: u16) -> bool {
    facts.value_kind(lhs) == PerfValueKind::Int && facts.value_kind(rhs) == PerfValueKind::Int
}

#[derive(Debug)]
struct CompiledFunction {
    function: Function,
    pending_functions: Vec<Function>,
}

pub fn compile_expr(expr: &Expr) -> Result<Function> {
    Compiler::compile_expr(expr)
}

pub fn compile_program(program: &Program) -> Result<Function> {
    Compiler::compile_program(program)
}

pub fn compile_module(program: &Program) -> Result<Module> {
    Compiler::compile_module(program)
}

pub fn compile_module_with_natives(program: &Program, natives: Vec<NativeEntry>) -> Result<Module> {
    Compiler::compile_module_with_natives(program, natives)
}

pub fn compile_source(source: &str) -> Result<Function> {
    Compiler::compile_source(source)
}

/// One active `for`-pattern variable: the binding slot is recorded when the
/// pattern binds (`None` while the loop head — range/iterable expressions —
/// still lowers against the enclosing scope).
#[derive(Debug)]
struct LoopSnapshotVar {
    name: String,
    slot: Option<u16>,
}

/// One name bound by a `for` pattern: the shadowed previous slot (if any) and
/// whether that previous binding carried a capture-cell mark to re-instate.
struct ForPatternBinding {
    name: String,
    slot: Option<u16>,
    was_cell: bool,
}

fn collect_for_pattern_names(pattern: &ForPattern, out: &mut Vec<LoopSnapshotVar>) {
    match pattern {
        ForPattern::Variable(name) => out.push(LoopSnapshotVar {
            name: name.clone(),
            slot: None,
        }),
        ForPattern::Ignore => {}
        ForPattern::Tuple(patterns) => {
            for pattern in patterns {
                collect_for_pattern_names(pattern, out);
            }
        }
        ForPattern::Array { patterns, rest } => {
            for pattern in patterns {
                collect_for_pattern_names(pattern, out);
            }
            if let Some(rest) = rest {
                out.push(LoopSnapshotVar {
                    name: rest.clone(),
                    slot: None,
                });
            }
        }
        ForPattern::Object(entries) => {
            for (_, pattern) in entries {
                collect_for_pattern_names(pattern, out);
            }
        }
    }
}

pub fn compile_source_module(source: &str) -> Result<Module> {
    Compiler::compile_source_module(source)
}

pub fn compile_source_module_with_natives(source: &str, natives: Vec<NativeEntry>) -> Result<Module> {
    Compiler::compile_source_module_with_natives(source, natives)
}
