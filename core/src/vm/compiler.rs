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
mod control_flow;
mod decls;
mod entry;
mod expr_lower;
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
mod stmt_lower;
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
    /// Top-level `let` names visible to callables: user-data globals, not
    /// module objects — method calls on them dispatch as methods.
    user_let_globals: HashSet<String>,
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
    pub(super) fn lower_expr(&mut self, expr: &Expr) -> Result<u16> {
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
            } // Every `Expr` variant lowers — parse-time-desugared sugar
              // (try/catch → pcall, select → select$block) never reaches here
              // as a dedicated node.
        }
    }

    pub(super) fn record_expr_analysis(&mut self, expr: &Expr) {
        if let Some(analysis) = super::ssa::pipeline::analyze_expr(expr) {
            self.function.analyses.push(analysis);
        }
    }

    pub(super) fn lower_template_string(&mut self, parts: &[TemplateStringPart]) -> Result<u16> {
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

    pub(super) fn lower_template_string_part(
        &mut self,
        part: &TemplateStringPart,
        force_expr_string: bool,
    ) -> Result<u16> {
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

    pub(super) fn lower_template_string_part_to_register(
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

    pub(super) fn lower_block_expr(&mut self, statements: &[Box<Stmt>]) -> Result<u16> {
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

    pub(super) fn lower_closure(&mut self, params: &[String], body: &Expr) -> Result<u16> {
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

    pub(in crate::vm::compiler) fn compile_closure_function(
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
        compiler.user_let_globals = self.user_let_globals.clone();
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

    pub(super) fn lower_conditional(&mut self, condition: &Expr, then_expr: &Expr, else_expr: &Expr) -> Result<u16> {
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

    pub(super) fn materialize_list(&mut self, values: Vec<u16>) -> Result<u16> {
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

    pub(super) fn lower_var(&mut self, name: &str) -> Result<u16> {
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

    pub(super) fn lower_bin(&mut self, lhs: &Expr, op: &BinOp, rhs: &Expr) -> Result<u16> {
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

    pub(super) fn emit_bin_op_to_register(&mut self, dst: u16, op: &BinOp, lhs: u16, rhs: u16) -> Result<u16> {
        let flavor =
            numeric_flavor_from_register_facts(&self.function.performance, op, lhs, rhs).unwrap_or(NumericFlavor::Int);
        self.emit_bin_op_to_register_with_flavor(dst, op, lhs, rhs, flavor)
    }

    pub(in crate::vm::compiler) fn emit_bin_op_to_register_with_flavor(
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

    pub(super) fn emit_int_immediate_to_register(
        &mut self,
        dst: u16,
        op: &BinOp,
        lhs: u16,
        immediate: i8,
    ) -> Result<u16> {
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
pub(in crate::vm::compiler) struct CompiledFunction {
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
