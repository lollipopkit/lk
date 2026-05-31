//! Minimal compiler for the new `Function32` IR.
//!
//! This is the first migration point from AST to the new VM path. It is
//! deliberately small and independent from the previous `FunctionBuilder`.

mod assign;
mod builder;
mod call;
mod container_lower;
mod entry;
mod facts;
#[cfg(test)]
mod facts_tests;
mod free_vars;
mod inline;
mod lower_into;
mod match_expr;
mod pattern_bind;
mod pattern_control;
mod support;
#[cfg(test)]
mod tests;

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::{Result, anyhow, bail};

use crate::{
    expr::{Expr, TemplateStringPart},
    operator::{BinOp, UnaryOp},
    stmt::{ForPattern, Program, Stmt},
    val::{FunctionNamedParamType, LiteralVal, ShortStr, Type},
};

use super::{
    ConstHeapValue32, ConstRuntimeValue32, Function32, GlobalSlot32, Instr32, Module32, NativeEntry32, Opcode32,
};
use crate::vm::analysis::{
    PerfCallTargetKind, PerfContainerBuildFact, PerfGlobalFact, PerfRegisterFact, PerfValueKind,
};
use facts::*;
use free_vars::collect_expr_free_vars;
use support::*;

#[derive(Debug, Default)]
pub struct Compiler32 {
    function: Function32,
    next_reg: u16,
    peak_reg: u16, // highest next_reg ever reached — used for register_count
    locals: HashMap<String, u16>,
    function_names: HashMap<String, u32>,
    function_signatures: HashMap<String, FunctionSignature32>,
    function_bodies: HashMap<String, FunctionInlineBody32>,
    native_names: HashMap<String, u32>,
    global_names: HashMap<String, u32>,
    capture_names: HashMap<String, u16>,
    capture_cells: HashSet<String>,
    cell_locals: HashSet<String>,
    dynamic_function_base: u32,
    pending_functions: Vec<Function32>,
    inline_stack: Vec<String>,
    loops: Vec<LoopPatch32>,
    top_level: bool,
    emitted_return: bool,
}

impl Compiler32 {
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
            other => bail!("Compiler32 does not support expression yet: {:?}", expr_kind(other)),
        }
    }

    fn record_expr_analysis(&mut self, expr: &Expr) {
        if let Some(analysis) = super::ssa::pipeline::analyze_expr(expr) {
            self.function.analyses.push(analysis);
        }
    }

    fn lower_stmt(&mut self, stmt: &Stmt) -> Result<()> {
        match stmt {
            Stmt::Empty => {}
            Stmt::Expr(expr) => {
                let watermark = self.next_reg;
                if !self.try_lower_rewritten_set_index_expr(expr)? {
                    if !self.try_lower_dead_literal_expr(expr)? {
                        self.lower_expr(expr)?;
                    }
                }
                self.next_reg = watermark;
            }
            Stmt::Return { value } => {
                let value = match value {
                    Some(value) => self.lower_expr(value)?,
                    None => {
                        let nil = self.alloc_reg();
                        self.emit(Instr32::abc(Opcode32::LoadNil, checked_u8("dst", nil)?, 0, 0));
                        nil
                    }
                };
                self.emit_return(value)?;
            }
            Stmt::Let { pattern, value, .. } => self.lower_let(pattern, value)?,
            Stmt::Define { name, value } => self.lower_define(name, value)?,
            Stmt::Assign { name, value, .. } => {
                let watermark = self.next_reg;
                self.lower_assign(name, value)?;
                self.next_reg = watermark;
            }
            Stmt::CompoundAssign { name, op, value, .. } => {
                let watermark = self.next_reg;
                self.lower_compound_assign(name, op, value)?;
                self.next_reg = watermark;
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
                for stmt in statements {
                    self.lower_stmt(stmt)?;
                    if self.emitted_return {
                        break;
                    }
                }
                self.locals = locals;
                self.cell_locals = cell_locals;
                if !self.emitted_return {
                    self.next_reg = self.live_register_floor().max(watermark);
                }
            }
        }
        Ok(())
    }

    fn lower_define(&mut self, name: &str, value: &Expr) -> Result<()> {
        let watermark = self.next_reg;
        let slot = if let Some(slot) = self.locals.get(name).copied() {
            slot
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
        self.insert_local(name.to_string(), slot);
        self.next_reg = self.live_register_floor().max(watermark).max(slot + 1);
        Ok(())
    }

    fn lower_val(&mut self, value: &LiteralVal) -> Result<u16> {
        let dst = self.alloc_reg();
        match value {
            LiteralVal::Nil => {
                self.emit(Instr32::abc(Opcode32::LoadNil, checked_u8("dst", dst)?, 0, 0));
                self.set_register_kind(dst, PerfValueKind::Nil);
            }
            LiteralVal::Bool(value) => self.emit(Instr32::abc(
                Opcode32::LoadBool,
                checked_u8("dst", dst)?,
                u8::from(*value),
                0,
            )),
            LiteralVal::Int(value) => {
                let k = self.push_int(*value)?;
                self.emit(Instr32::abx(Opcode32::LoadInt, checked_u8("dst", dst)?, k));
                self.set_register_kind(dst, PerfValueKind::Int);
            }
            LiteralVal::Float(value) => {
                let k = self.push_float(*value)?;
                self.emit(Instr32::abx(Opcode32::LoadFloat, checked_u8("dst", dst)?, k));
                self.set_register_kind(dst, PerfValueKind::Float);
            }
            value if value.as_str().is_some() => {
                let value = value.as_str().expect("checked string");
                if ShortStr::new(value).is_some() {
                    let k = self.push_string(value)?;
                    self.emit(Instr32::abx(Opcode32::LoadString, checked_u8("dst", dst)?, k));
                } else {
                    let k = self.push_heap_value(ConstHeapValue32::LongString(value.into()))?;
                    self.emit(Instr32::abx(Opcode32::LoadHeapConst, checked_u8("dst", dst)?, k));
                }
                self.set_register_kind(dst, PerfValueKind::String);
            }
            other => {
                bail!(
                    "Compiler32 cannot materialize AST literal value yet: {}",
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
        let target = self.lower_readonly_access_target(target)?;
        let index_fact = index_fact_from_target(&self.function.performance, target);
        let (key, key_fact) = self.lower_index_key(key)?;
        let dst = self.alloc_reg();
        let pc = self.function.code.len();
        self.emit(Instr32::abc(
            Opcode32::GetIndex,
            checked_u8("index dst", dst)?,
            checked_u8("index target", target)?,
            checked_u8("index key", key)?,
        ));
        self.function.performance.clear_register(dst);
        if let Some(fact) = key_fact {
            self.function.performance.set_key_fact(pc, fact);
        }
        if let Some(fact) = index_fact {
            self.function.performance.set_index_fact(pc, fact);
        }
        Ok(dst)
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

    fn lower_index_key(&mut self, key: &Expr) -> Result<(u16, Option<crate::vm::analysis::PerfKeyFact>)> {
        if let Some(text) = short_string_literal_key(key) {
            let dst = self.alloc_reg();
            let const_key = self.push_string(text)?;
            self.emit(Instr32::abx(
                Opcode32::LoadString,
                checked_u8("index key", dst)?,
                const_key,
            ));
            self.set_register_kind(dst, PerfValueKind::String);
            return Ok((
                dst,
                Some(crate::vm::analysis::PerfKeyFact {
                    const_key: Some(const_key),
                    string_int: None,
                }),
            ));
        }
        Ok((self.lower_readonly_operand(key)?, None))
    }

    fn lower_optional_access(&mut self, target: &Expr, key: &Expr) -> Result<u16> {
        let target = self.lower_readonly_access_target(target)?;
        let dst = self.alloc_reg();
        self.emit(Instr32::abc(Opcode32::LoadNil, checked_u8("optional dst", dst)?, 0, 0));

        let is_nil = self.alloc_reg();
        self.emit(Instr32::abc(
            Opcode32::IsNil,
            checked_u8("optional test dst", is_nil)?,
            checked_u8("optional target", target)?,
            0,
        ));
        let skip_get = self.emit_test_placeholder(is_nil)?;

        let index_fact = index_fact_from_target(&self.function.performance, target);
        let (key, key_fact) = self.lower_index_key(key)?;
        let pc = self.function.code.len();
        self.emit(Instr32::abc(
            Opcode32::GetIndex,
            checked_u8("optional get dst", dst)?,
            checked_u8("optional get target", target)?,
            checked_u8("optional get key", key)?,
        ));
        self.function.performance.clear_register(dst);
        if let Some(fact) = key_fact {
            self.function.performance.set_key_fact(pc, fact);
        }
        if let Some(fact) = index_fact {
            self.function.performance.set_index_fact(pc, fact);
        }
        let end = self.function.code.len();
        self.patch_test_true_jump(skip_get, end)?;
        Ok(dst)
    }

    fn lower_unary(&mut self, op: &UnaryOp, inner: &Expr) -> Result<u16> {
        let src = self.lower_readonly_operand(inner)?;
        let dst = self.alloc_reg();
        let opcode = match op {
            UnaryOp::Not => Opcode32::Not,
        };
        self.emit(Instr32::abc(
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
                self.emit(Instr32::abc(
                    Opcode32::IsNil,
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

    fn lower_template_string(&mut self, parts: &[TemplateStringPart]) -> Result<u16> {
        let mut acc = self.lower_val(&LiteralVal::from_str(""))?;
        for part in parts {
            let part_reg = match part {
                TemplateStringPart::Literal(value) => self.lower_val(&LiteralVal::from_str(value))?,
                TemplateStringPart::Expr(expr) => {
                    let value = self.lower_expr(expr)?;
                    let dst = self.alloc_reg();
                    self.emit(Instr32::abc(
                        Opcode32::ToString,
                        checked_u8("template string dst", dst)?,
                        checked_u8("template string src", value)?,
                        0,
                    ));
                    self.set_register_kind(dst, PerfValueKind::String);
                    dst
                }
            };
            let next = self.alloc_reg();
            self.emit(Instr32::abc(
                Opcode32::ConcatString,
                checked_u8("template concat dst", next)?,
                checked_u8("template concat lhs", acc)?,
                checked_u8("template concat rhs", part_reg)?,
            ));
            self.set_register_kind(next, PerfValueKind::String);
            acc = next;
        }
        Ok(acc)
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
                    self.emit(Instr32::abc(
                        Opcode32::LoadNil,
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
                        self.emit(Instr32::abc(
                            Opcode32::LoadNil,
                            checked_u8("block returned", nil)?,
                            0,
                            0,
                        ));
                        return Ok(nil);
                    }
                }
            }
        }
        if let Some(last) = last {
            Ok(last)
        } else {
            let nil = self.alloc_reg();
            self.emit(Instr32::abc(Opcode32::LoadNil, checked_u8("empty block", nil)?, 0, 0));
            Ok(nil)
        }
    }

    fn lower_closure(&mut self, params: &[String], body: &Expr) -> Result<u16> {
        let captures = self.collect_closure_captures(params, body);
        let function_index = self
            .dynamic_function_base
            .checked_add(self.pending_functions.len() as u32)
            .ok_or_else(|| anyhow!("Compiler32 dynamic function index overflow"))?;
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
        self.emit(Instr32::abc(
            Opcode32::MakeClosure,
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
    ) -> Result<CompiledFunction32> {
        if params.len() > u16::MAX as usize {
            bail!("Compiler32 closure has too many params: {}", params.len());
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
                    compiler.emit(Instr32::abc(Opcode32::LoadNil, checked_u8("dst", nil)?, 0, 0));
                    compiler.emit_return(nil)?;
                }
            }
            body => {
                let value = compiler.lower_expr(body)?;
                compiler.emit_return(value)?;
            }
        }
        Ok(CompiledFunction32 {
            function: compiler.finish()?,
            pending_functions: compiler.pending_functions,
        })
    }

    fn lower_conditional(&mut self, condition: &Expr, then_expr: &Expr, else_expr: &Expr) -> Result<u16> {
        let condition = self.lower_readonly_operand(condition)?;
        let dst = self.alloc_reg();
        let test_pc = self.emit_test_placeholder(condition)?;

        self.lower_expr_to_register(dst, then_expr, "conditional then")?;
        let jmp_end = self.emit_jmp_placeholder();

        let else_start = self.function.code.len();
        self.patch_test_false_jump(test_pc, else_start)?;
        self.lower_expr_to_register(dst, else_expr, "conditional else")?;

        let end = self.function.code.len();
        self.patch_jmp(jmp_end, end)?;
        Ok(dst)
    }

    fn materialize_list(&mut self, values: Vec<u16>) -> Result<u16> {
        let len = values.len();
        if len > u8::MAX as usize {
            bail!("Compiler32 list literal has {} elements, max {}", len, u8::MAX);
        }

        let base = self.alloc_regs(len)?;
        for (offset, value) in values.into_iter().enumerate() {
            let move_source = !self.is_current_local_slot(value);
            self.emit_move_with_policy(base + offset as u16, value, "list element", move_source)?;
        }

        let dst = self.alloc_reg();
        let pc = self.function.code.len();
        self.emit(Instr32::abc(
            Opcode32::NewList,
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
        Err(anyhow!("Compiler32 undefined local/global `{name}`"))
    }

    fn lower_if(&mut self, condition: &Expr, then_stmt: &Stmt, else_stmt: Option<&Stmt>) -> Result<()> {
        let watermark = self.next_reg;
        let condition = self.lower_readonly_operand(condition)?;
        let test_pc = self.emit_test_placeholder(condition)?;

        self.emitted_return = false;
        self.lower_stmt(then_stmt)?;
        let then_returns = self.emitted_return;
        self.next_reg = watermark; // recycle registers from then-branch

        if let Some(else_stmt) = else_stmt {
            let jmp_end = (!then_returns).then(|| self.emit_jmp_placeholder());
            let else_start = self.function.code.len();
            self.patch_test_false_jump(test_pc, else_start)?;

            self.emitted_return = false;
            self.lower_stmt(else_stmt)?;
            let else_returns = self.emitted_return;
            self.next_reg = watermark; // recycle registers from else-branch

            if let Some(jmp_end) = jmp_end {
                let end = self.function.code.len();
                self.patch_jmp(jmp_end, end)?;
            }
            self.emitted_return = then_returns && else_returns;
        } else {
            let end = self.function.code.len();
            self.patch_test_false_jump(test_pc, end)?;
            self.emitted_return = false;
        }

        Ok(())
    }

    fn lower_while(&mut self, condition: &Expr, body: &Stmt) -> Result<()> {
        let watermark = self.next_reg;
        let loop_start = self.function.code.len();
        let condition = self.lower_readonly_operand(condition)?;
        let test_pc = self.emit_test_placeholder(condition)?;

        self.loops.push(LoopPatch32::default());
        self.emitted_return = false;
        self.lower_stmt(body)?;
        let loop_patch = self.loops.pop().expect("loop patch just pushed");
        if !self.emitted_return {
            let jmp_back = self.emit_jmp_placeholder();
            self.patch_jmp(jmp_back, loop_start)?;
        }

        let end = self.function.code.len();
        self.patch_test_false_jump(test_pc, end)?;
        for pc in loop_patch.breaks {
            self.patch_jmp(pc, end)?;
        }
        for pc in loop_patch.continues {
            self.patch_jmp(pc, loop_start)?;
        }
        self.emitted_return = false;
        self.next_reg = watermark; // recycle all loop registers
        Ok(())
    }

    fn lower_for(&mut self, pattern: &ForPattern, iterable: &Expr, body: &Stmt) -> Result<()> {
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
        let direct_iterable = matches!(
            self.function.performance.value_kind(iterable_value),
            PerfValueKind::List | PerfValueKind::String
        );
        let iterable = if direct_iterable {
            iterable_value
        } else {
            let iterable = self.alloc_reg();
            self.emit(Instr32::abc(
                Opcode32::ToIter,
                checked_u8("for indexed iter dst", iterable)?,
                checked_u8("for indexed iter src", iterable_value)?,
                0,
            ));
            self.set_register_kind(iterable, PerfValueKind::List);
            iterable
        };
        let len = self.alloc_reg();
        self.emit(Instr32::abc(
            Opcode32::Len,
            checked_u8("for indexed len dst", len)?,
            checked_u8("for indexed iterable", iterable)?,
            0,
        ));
        self.set_register_kind(len, PerfValueKind::Int);
        let index = self.lower_val(&LiteralVal::Int(0))?;
        let step = self.lower_val(&LiteralVal::Int(1))?;
        let value = self.alloc_reg();

        let loop_start = self.function.code.len();
        let condition = self.alloc_reg();
        self.emit(Instr32::abc(
            Opcode32::CmpLtInt,
            checked_u8("for indexed condition dst", condition)?,
            checked_u8("for indexed index", index)?,
            checked_u8("for indexed len", len)?,
        ));
        let exit_test = self.emit_test_placeholder(condition)?;
        self.emit(Instr32::abc(
            Opcode32::GetIndex,
            checked_u8("for indexed value", value)?,
            checked_u8("for indexed iterable", iterable)?,
            checked_u8("for indexed index", index)?,
        ));
        if let Some(fact) = index_fact_from_target(&self.function.performance, iterable) {
            let pc = self.function.code.len() - 1;
            self.function.performance.set_index_fact(pc, fact);
        }
        let previous_binding = self.bind_for_pattern(pattern, value)?;

        self.loops.push(LoopPatch32::default());
        self.emitted_return = false;
        self.lower_stmt(body)?;
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
        let step_sign = range_step_sign(step);
        let index = self.alloc_reg();
        match start {
            Some(start) => self.lower_expr_to_register(index, start, "for range initial index")?,
            None => self.emit_literal_to_register(index, &LiteralVal::Int(0))?,
        }
        let end = end.ok_or_else(|| anyhow!("Compiler32 open-ended range for loop is not supported"))?;
        let end = self.lower_expr(end)?;
        let step = match step {
            Some(step) => self.lower_expr(step)?,
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
        self.next_reg = watermark; // recycle all loop registers
        Ok(())
    }

    fn lower_for_range_static_loop(
        &mut self,
        index: u16,
        end: u16,
        step: u16,
        inclusive: bool,
        positive_step: bool,
        body: &Stmt,
    ) -> Result<()> {
        let loop_start = self.function.code.len();
        let condition = self.alloc_reg();
        self.emit(Instr32::abc(
            match (positive_step, inclusive) {
                (true, true) => Opcode32::CmpLeInt,
                (true, false) => Opcode32::CmpLtInt,
                (false, true) => Opcode32::CmpGeInt,
                (false, false) => Opcode32::CmpGtInt,
            },
            checked_u8("for range static condition dst", condition)?,
            checked_u8("for range static index", index)?,
            checked_u8("for range static end", end)?,
        ));

        let exit_test = self.emit_test_placeholder(condition)?;
        self.loops.push(LoopPatch32::default());
        self.emitted_return = false;
        self.lower_stmt(body)?;
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
        self.emitted_return = false;
        Ok(())
    }

    fn lower_for_range_dynamic_loop(
        &mut self,
        index: u16,
        end: u16,
        step: u16,
        inclusive: bool,
        body: &Stmt,
    ) -> Result<()> {
        let loop_start = self.function.code.len();
        let zero = self.lower_val(&LiteralVal::Int(0))?;
        let is_positive = self.alloc_reg();
        self.emit(Instr32::abc(
            Opcode32::CmpGtInt,
            checked_u8("for step sign dst", is_positive)?,
            checked_u8("for step", step)?,
            checked_u8("for zero", zero)?,
        ));

        let positive_cond = self.alloc_reg();
        self.emit(Instr32::abc(
            if inclusive {
                Opcode32::CmpLeInt
            } else {
                Opcode32::CmpLtInt
            },
            checked_u8("for positive cond dst", positive_cond)?,
            checked_u8("for index", index)?,
            checked_u8("for end", end)?,
        ));
        let negative_cond = self.alloc_reg();
        self.emit(Instr32::abc(
            if inclusive {
                Opcode32::CmpGeInt
            } else {
                Opcode32::CmpGtInt
            },
            checked_u8("for negative cond dst", negative_cond)?,
            checked_u8("for index", index)?,
            checked_u8("for end", end)?,
        ));

        let condition = self.alloc_reg();
        self.emit_move(condition, positive_cond, "for range positive condition")?;
        let keep_positive = self.emit_test_placeholder(is_positive)?;
        self.emit_move(condition, negative_cond, "for range negative condition")?;
        let condition_ready = self.function.code.len();
        self.patch_test_true_jump(keep_positive, condition_ready)?;

        let exit_test = self.emit_test_placeholder(condition)?;
        self.loops.push(LoopPatch32::default());
        self.emitted_return = false;
        self.lower_stmt(body)?;
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
        self.emitted_return = false;
        Ok(())
    }

    fn bind_for_pattern(&mut self, pattern: &ForPattern, value: u16) -> Result<Vec<(String, Option<u16>)>> {
        let mut previous = Vec::new();
        self.bind_for_pattern_inner(pattern, value, &mut previous)?;
        Ok(previous)
    }

    fn bind_for_pattern_inner(
        &mut self,
        pattern: &ForPattern,
        value: u16,
        previous: &mut Vec<(String, Option<u16>)>,
    ) -> Result<()> {
        match pattern {
            ForPattern::Variable(name) => {
                previous.push((name.clone(), self.insert_local(name.clone(), value)));
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
                self.emit(Instr32::abc(
                    Opcode32::SliceFrom,
                    checked_u8("for rest slice", slice)?,
                    checked_u8("for rest value", value)?,
                    checked_u8("for rest start", start)?,
                ));
                previous.push((rest.clone(), self.insert_local(rest.clone(), slice)));
                Ok(())
            }
            ForPattern::Object(entries) => {
                let condition =
                    self.lower_map_pattern_key_condition(value, entries.iter().map(|(key, _)| key.as_str()))?;
                self.emit_pattern_assert(condition)?;
                for (key, pattern) in entries {
                    let key = self.lower_val(&LiteralVal::from_str(key))?;
                    let field = self.alloc_reg();
                    self.emit(Instr32::abc(
                        Opcode32::GetIndex,
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
        previous: &mut Vec<(String, Option<u16>)>,
    ) -> Result<()> {
        for (index, pattern) in patterns.iter().enumerate() {
            let index = i64::try_from(index).map_err(|_| anyhow!("Compiler32 for pattern index overflow"))?;
            let key = self.lower_val(&LiteralVal::Int(index))?;
            let field = self.alloc_reg();
            self.emit(Instr32::abc(
                Opcode32::GetIndex,
                checked_u8("for sequence field", field)?,
                checked_u8("for sequence value", value)?,
                checked_u8("for sequence index", key)?,
            ));
            self.bind_for_pattern_inner(pattern, field, previous)?;
        }
        Ok(())
    }

    fn restore_for_pattern(&mut self, previous: Vec<(String, Option<u16>)>) {
        for (name, old) in previous.into_iter().rev() {
            if let Some(old) = old {
                self.insert_local(name, old);
            } else {
                self.locals.remove(&name);
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
                bail!("Compiler32 impl block only supports function methods");
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
            .ok_or_else(|| anyhow!("Compiler32 dynamic impl method index overflow"))?;
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
        self.emit(Instr32::abx(
            Opcode32::LoadFunction,
            checked_u8("impl method function dst", dst)?,
            u16::try_from(function_index)
                .map_err(|_| anyhow!("Compiler32 impl method index {function_index} exceeds u16"))?,
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
            .ok_or_else(|| anyhow!("Compiler32 undefined callable `{name}`"))
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
            .ok_or_else(|| anyhow!("Compiler32 undefined function `{name}`"))?;
        let dst = self.alloc_reg();
        let function_index = u16::try_from(function_index)
            .map_err(|_| anyhow!("Compiler32 function index {function_index} exceeds u16"))?;
        self.emit(Instr32::abx(
            Opcode32::LoadFunction,
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
            .ok_or_else(|| anyhow!("Compiler32 undefined native `{name}`"))?;
        let dst = self.alloc_reg();
        let native_index =
            u16::try_from(native_index).map_err(|_| anyhow!("Compiler32 native index {native_index} exceeds u16"))?;
        self.emit(Instr32::abx(
            Opcode32::LoadNative,
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
        let slot = u16::try_from(slot).map_err(|_| anyhow!("Compiler32 global slot {slot} exceeds u16"))?;
        let pc = self.function.code.len();
        self.emit(Instr32::abx(Opcode32::GetGlobal, checked_u8("global dst", dst)?, slot));
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
        self.emit(Instr32::abx(
            Opcode32::LoadCapture,
            checked_u8("capture dst", dst)?,
            capture,
        ));
        self.function.performance.clear_register(dst);
        Ok(dst)
    }

    fn emit_load_cell_value(&mut self, cell: u16) -> Result<u16> {
        let dst = self.alloc_reg();
        self.emit(Instr32::abc(
            Opcode32::LoadCellVal,
            checked_u8("cell value dst", dst)?,
            checked_u8("cell value src", cell)?,
            0,
        ));
        self.function.performance.clear_register(dst);
        Ok(dst)
    }

    fn lower_capture_value(&mut self, name: &str) -> Result<(u16, bool)> {
        if let Some(local) = self.locals.get(name).copied() {
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
        let dst = self.alloc_reg();
        let k = self.push_heap_value(ConstHeapValue32::UpvalCell(Box::new(ConstRuntimeValue32::Nil)))?;
        self.emit(Instr32::abx(
            Opcode32::LoadHeapConst,
            checked_u8("upval cell dst", dst)?,
            k,
        ));
        self.emit_store_cell_value(dst, src, "upval cell")?;
        Ok(dst)
    }

    fn emit_set_global(&mut self, src: u16, slot: u32) -> Result<()> {
        self.emit_set_global_with_policy(src, slot, false)
    }

    pub(super) fn emit_set_global_with_policy(&mut self, src: u16, slot: u32, move_source: bool) -> Result<()> {
        let slot = u16::try_from(slot).map_err(|_| anyhow!("Compiler32 global slot {slot} exceeds u16"))?;
        let pc = self.function.code.len();
        self.emit(Instr32::abx(Opcode32::SetGlobal, checked_u8("global src", src)?, slot));
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
        let lhs = self.lower_readonly_operand(lhs)?;
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
                NumericFlavor::Int => Opcode32::AddInt,
                NumericFlavor::Float => Opcode32::AddFloat,
            },
            BinOp::Sub => match flavor {
                NumericFlavor::Int => Opcode32::SubInt,
                NumericFlavor::Float => Opcode32::SubFloat,
            },
            BinOp::Mul => match flavor {
                NumericFlavor::Int => Opcode32::MulInt,
                NumericFlavor::Float => Opcode32::MulFloat,
            },
            BinOp::Div => match flavor {
                NumericFlavor::Int => Opcode32::DivInt,
                NumericFlavor::Float => Opcode32::DivFloat,
            },
            BinOp::Mod => match flavor {
                NumericFlavor::Int => Opcode32::ModInt,
                NumericFlavor::Float => Opcode32::ModFloat,
            },
            BinOp::Eq => Opcode32::CmpInt,
            BinOp::Ne => Opcode32::CmpNeInt,
            BinOp::Lt => Opcode32::CmpLtInt,
            BinOp::Le => Opcode32::CmpLeInt,
            BinOp::Gt => Opcode32::CmpGtInt,
            BinOp::Ge => Opcode32::CmpGeInt,
            BinOp::In => Opcode32::Contains,
        };
        self.emit(Instr32::abc(
            opcode,
            checked_u8("dst", dst)?,
            checked_u8("lhs", lhs)?,
            checked_u8("rhs", rhs)?,
        ));
        self.set_register_kind(dst, bin_op_result_kind(op, flavor));
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

#[derive(Debug)]
struct CompiledFunction32 {
    function: Function32,
    pending_functions: Vec<Function32>,
}

pub fn compile_expr32(expr: &Expr) -> Result<Function32> {
    Compiler32::compile_expr(expr)
}

pub fn compile_program32(program: &Program) -> Result<Function32> {
    Compiler32::compile_program(program)
}

pub fn compile_module32(program: &Program) -> Result<Module32> {
    Compiler32::compile_module(program)
}

pub fn compile_module_with_natives32(program: &Program, natives: Vec<NativeEntry32>) -> Result<Module32> {
    Compiler32::compile_module_with_natives(program, natives)
}

pub fn compile_source32(source: &str) -> Result<Function32> {
    Compiler32::compile_source(source)
}

pub fn compile_source_module32(source: &str) -> Result<Module32> {
    Compiler32::compile_source_module(source)
}

pub fn compile_source_module_with_natives32(source: &str, natives: Vec<NativeEntry32>) -> Result<Module32> {
    Compiler32::compile_source_module_with_natives(source, natives)
}
