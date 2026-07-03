use crate::compat::collections::{HashMap, HashSet};
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;

use anyhow::Result;

use crate::{
    expr::{Expr, MatchArm, SelectCase},
    stmt::Stmt,
    util::fast_map::FastHashMap,
    val::{LiteralVal, RuntimeMapKey, ShortStr},
    vm::ConstRuntimeValue,
};

use super::{
    Compiler,
    call::map_get_method_call_args,
    checked_u8,
    inline::{inline_body_is_supported, stmt_contains_call_to},
    support::{FunctionInlineBody, const_runtime_map_key_from_literal},
};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum ScalarLoopConstKey {
    Nil,
    Bool(bool),
    Int(i64),
    Float(u64),
    ShortString(String),
}

impl Compiler {
    pub(super) fn begin_loop_scalar_const_scope(&mut self, condition: &Expr, body: &Stmt) -> Result<u16> {
        self.begin_loop_scalar_const_scope_for_exprs(&[condition], body)
    }

    pub(super) fn begin_loop_scalar_const_scope_for_exprs(&mut self, exprs: &[&Expr], body: &Stmt) -> Result<u16> {
        let mut keys = Vec::new();
        for expr in exprs {
            collect_expr_scalar_consts(expr, &mut keys);
            collect_expr_const_map_get_scalar_consts(expr, &self.const_map_locals, &mut keys)?;
        }
        collect_stmt_scalar_consts(body, &mut keys);
        collect_stmt_const_map_get_scalar_consts(body, &self.const_map_locals, &mut keys)?;
        collect_stmt_folded_int_consts(body, &mut HashMap::new(), &mut keys);
        self.collect_inline_call_scalar_consts_from_exprs(exprs, &mut keys);
        self.collect_inline_call_scalar_consts_from_stmt(body, &mut keys);

        let mut scope = HashMap::new();
        for key in keys {
            if scope.contains_key(&key) {
                continue;
            }
            let dst = self.alloc_reg();
            self.emit_scalar_loop_const(dst, &key)?;
            scope.insert(key, dst);
        }
        self.loop_const_scopes.push(scope);
        Ok(self.next_reg)
    }

    fn collect_inline_call_scalar_consts_from_exprs(&self, exprs: &[&Expr], keys: &mut Vec<ScalarLoopConstKey>) {
        let mut visiting = HashSet::new();
        for expr in exprs {
            collect_expr_inline_call_scalar_consts(expr, &self.function_bodies, &mut visiting, keys);
        }
    }

    fn collect_inline_call_scalar_consts_from_stmt(&self, stmt: &Stmt, keys: &mut Vec<ScalarLoopConstKey>) {
        let mut visiting = HashSet::new();
        collect_stmt_inline_call_scalar_consts(stmt, &self.function_bodies, &mut visiting, keys);
    }

    pub(super) fn end_loop_scalar_const_scope(&mut self) {
        self.loop_const_scopes
            .pop()
            .expect("loop const scope should be balanced");
    }

    pub(super) fn cached_loop_literal(&self, value: &LiteralVal) -> Option<u16> {
        let key = scalar_loop_const_key(value)?;
        self.loop_const_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(&key).copied())
    }

    pub(super) fn cached_loop_literal_expr(&self, expr: &Expr) -> Option<u16> {
        match expr {
            Expr::Paren(inner) => self.cached_loop_literal_expr(inner),
            Expr::Literal(value) => self.cached_loop_literal(value),
            _ => None,
        }
    }

    pub(super) fn cached_loop_int_expr_value(&self, expr: &Expr) -> Option<i64> {
        cached_loop_int_expr_value_from_locals(expr, &|name| {
            let reg = self.locals.get(name).copied()?;
            self.loop_const_scopes
                .iter()
                .rev()
                .find_map(|scope| scope.iter().find(|(_, cached)| **cached == reg))
                .and_then(|(key, _)| match key {
                    ScalarLoopConstKey::Int(value) => Some(*value),
                    _ => None,
                })
        })
    }

    pub(super) fn is_loop_cached_literal_register(&self, reg: u16) -> bool {
        self.loop_const_scopes
            .iter()
            .rev()
            .any(|scope| scope.values().any(|cached| *cached == reg))
    }

    pub(super) fn loop_cached_literal_register_floor(&self) -> u16 {
        self.loop_const_scopes
            .iter()
            .flat_map(|scope| scope.values().copied())
            .max()
            .map_or(0, |reg| reg + 1)
    }

    fn emit_scalar_loop_const(&mut self, dst: u16, key: &ScalarLoopConstKey) -> Result<()> {
        match key {
            ScalarLoopConstKey::Nil => {
                self.emit(super::Instr::abc(
                    super::Opcode::LoadNil,
                    checked_u8("loop const dst", dst)?,
                    0,
                    0,
                ));
                self.set_register_kind(dst, super::PerfValueKind::Nil);
            }
            ScalarLoopConstKey::Bool(value) => {
                self.emit(super::Instr::abc(
                    super::Opcode::LoadBool,
                    checked_u8("loop const dst", dst)?,
                    u8::from(*value),
                    0,
                ));
                self.set_register_kind(dst, super::PerfValueKind::Bool);
            }
            ScalarLoopConstKey::Int(value) => {
                let k = self.push_int(*value)?;
                self.emit(super::Instr::abx(
                    super::Opcode::LoadInt,
                    checked_u8("loop const dst", dst)?,
                    k,
                ));
                self.set_register_kind(dst, super::PerfValueKind::Int);
            }
            ScalarLoopConstKey::Float(bits) => {
                let k = self.push_float(f64::from_bits(*bits))?;
                self.emit(super::Instr::abx(
                    super::Opcode::LoadFloat,
                    checked_u8("loop const dst", dst)?,
                    k,
                ));
                self.set_register_kind(dst, super::PerfValueKind::Float);
            }
            ScalarLoopConstKey::ShortString(value) => {
                let k = self.push_string(value)?;
                self.emit(super::Instr::abx(
                    super::Opcode::LoadString,
                    checked_u8("loop const dst", dst)?,
                    k,
                ));
                self.set_register_kind(dst, super::PerfValueKind::String);
            }
        }
        Ok(())
    }
}

fn collect_stmt_scalar_consts(stmt: &Stmt, keys: &mut Vec<ScalarLoopConstKey>) {
    match stmt {
        Stmt::Attributed { item, .. } => collect_stmt_scalar_consts(item, keys),
        Stmt::Empty | Stmt::Break | Stmt::Continue | Stmt::Import(_) | Stmt::Struct { .. } | Stmt::TypeAlias { .. } => {
        }
        Stmt::Expr(expr) | Stmt::Return { value: Some(expr) } => collect_expr_scalar_consts(expr, keys),
        Stmt::Return { value: None } => {}
        Stmt::Let { value, .. } | Stmt::Define { value, .. } => collect_expr_scalar_consts(value, keys),
        Stmt::Assign { value, .. } | Stmt::CompoundAssign { value, .. } => collect_expr_scalar_consts(value, keys),
        Stmt::If {
            condition,
            then_stmt,
            else_stmt,
        } => {
            collect_expr_scalar_consts(condition, keys);
            collect_stmt_scalar_consts(then_stmt, keys);
            if let Some(else_stmt) = else_stmt {
                collect_stmt_scalar_consts(else_stmt, keys);
            }
        }
        Stmt::IfLet {
            value,
            then_stmt,
            else_stmt,
            ..
        } => {
            collect_expr_scalar_consts(value, keys);
            collect_stmt_scalar_consts(then_stmt, keys);
            if let Some(else_stmt) = else_stmt {
                collect_stmt_scalar_consts(else_stmt, keys);
            }
        }
        Stmt::While { condition, body } => {
            collect_expr_scalar_consts(condition, keys);
            collect_stmt_scalar_consts(body, keys);
        }
        Stmt::WhileLet { value, body, .. } => {
            collect_expr_scalar_consts(value, keys);
            collect_stmt_scalar_consts(body, keys);
        }
        Stmt::For { iterable, body, .. } => {
            collect_expr_scalar_consts(iterable, keys);
            collect_stmt_scalar_consts(body, keys);
        }
        Stmt::Block { statements } => {
            for stmt in statements {
                collect_stmt_scalar_consts(stmt, keys);
            }
        }
        Stmt::Function { named_params, .. } => {
            for param in named_params {
                if let Some(default) = &param.default {
                    collect_expr_scalar_consts(default, keys);
                }
            }
        }
        Stmt::Trait { .. } => {}
        Stmt::Impl { methods, .. } => {
            for method in methods {
                collect_stmt_scalar_consts(method, keys);
            }
        }
    }
}

fn collect_expr_scalar_consts(expr: &Expr, keys: &mut Vec<ScalarLoopConstKey>) {
    match expr {
        Expr::Literal(value) => {
            if let Some(key) = scalar_loop_const_key(value) {
                keys.push(key);
            }
        }
        Expr::Paren(inner) | Expr::Unary(_, inner) => collect_expr_scalar_consts(inner, keys),
        Expr::OptionalAccess(inner, field) | Expr::Access(inner, field) => {
            collect_expr_scalar_consts(inner, keys);
            collect_expr_scalar_consts(field, keys);
        }
        Expr::CallExpr(callee, args) => {
            collect_expr_scalar_consts(callee, keys);
            collect_boxed_exprs_scalar_consts(args, keys);
        }
        Expr::Bin(lhs, _, rhs) | Expr::And(lhs, rhs) | Expr::Or(lhs, rhs) | Expr::NullishCoalescing(lhs, rhs) => {
            collect_expr_scalar_consts(lhs, keys);
            collect_expr_scalar_consts(rhs, keys);
        }
        Expr::List(elements) => {
            collect_boxed_exprs_scalar_consts(elements, keys);
        }
        Expr::Map(entries) => {
            for (key, value) in entries {
                collect_expr_scalar_consts(key, keys);
                collect_expr_scalar_consts(value, keys);
            }
        }
        Expr::StructLiteral { fields, .. } => {
            for (_, value) in fields {
                collect_expr_scalar_consts(value, keys);
            }
        }
        Expr::Call(_, args) => collect_boxed_exprs_scalar_consts(args, keys),
        Expr::CallNamed(callee, positional, named) => {
            collect_expr_scalar_consts(callee, keys);
            collect_boxed_exprs_scalar_consts(positional, keys);
            for (_, value) in named {
                collect_expr_scalar_consts(value, keys);
            }
        }
        Expr::TemplateString(parts) => {
            for part in parts {
                match part {
                    crate::expr::TemplateStringPart::Literal(value) => {
                        if let Some(key) =
                            ShortStr::new(value).map(|_| ScalarLoopConstKey::ShortString(value.to_owned()))
                        {
                            keys.push(key);
                        }
                    }
                    crate::expr::TemplateStringPart::Expr(expr) => collect_expr_scalar_consts(expr, keys),
                }
            }
        }
        Expr::Block(statements) => {
            for stmt in statements {
                collect_stmt_scalar_consts(stmt, keys);
            }
        }
        Expr::Range { start, end, step, .. } => {
            if let Some(start) = start {
                collect_expr_scalar_consts(start, keys);
            }
            if let Some(end) = end {
                collect_expr_scalar_consts(end, keys);
            }
            if let Some(step) = step {
                collect_expr_scalar_consts(step, keys);
            }
        }
        Expr::Select { cases, default_case } => {
            for SelectCase { body, .. } in cases {
                collect_expr_scalar_consts(body, keys);
            }
            if let Some(default_case) = default_case {
                collect_expr_scalar_consts(default_case, keys);
            }
        }
        Expr::Match { value, arms } => {
            collect_expr_scalar_consts(value, keys);
            for MatchArm { body, .. } in arms {
                collect_expr_scalar_consts(body, keys);
            }
        }
        Expr::Conditional(condition, then_expr, else_expr) => {
            collect_expr_scalar_consts(condition, keys);
            collect_expr_scalar_consts(then_expr, keys);
            collect_expr_scalar_consts(else_expr, keys);
        }
        Expr::Var(_) | Expr::Closure { .. } => {}
    }
}

fn collect_stmt_folded_int_consts(stmt: &Stmt, locals: &mut HashMap<String, i64>, keys: &mut Vec<ScalarLoopConstKey>) {
    match stmt {
        Stmt::Attributed { item, .. } => collect_stmt_folded_int_consts(item, locals, keys),
        Stmt::Let { pattern, value, .. } => {
            collect_expr_folded_int_consts(value, locals, keys);
            if let crate::expr::Pattern::Variable(name) = pattern {
                if let Some(value) = folded_int_expr_value(value, locals) {
                    locals.insert(name.clone(), value);
                } else {
                    locals.remove(name);
                }
            }
        }
        Stmt::Define { name, value } => {
            collect_expr_folded_int_consts(value, locals, keys);
            if let Some(value) = folded_int_expr_value(value, locals) {
                locals.insert(name.clone(), value);
            } else {
                locals.remove(name);
            }
        }
        Stmt::Assign { name, value, .. } | Stmt::CompoundAssign { name, value, .. } => {
            collect_expr_folded_int_consts(value, locals, keys);
            locals.remove(name);
        }
        Stmt::Expr(expr) | Stmt::Return { value: Some(expr) } => collect_expr_folded_int_consts(expr, locals, keys),
        Stmt::Return { value: None }
        | Stmt::Empty
        | Stmt::Break
        | Stmt::Continue
        | Stmt::Import(_)
        | Stmt::Struct { .. }
        | Stmt::TypeAlias { .. }
        | Stmt::Trait { .. } => {}
        Stmt::If {
            condition,
            then_stmt,
            else_stmt,
        } => {
            collect_expr_folded_int_consts(condition, locals, keys);
            collect_stmt_folded_int_consts(then_stmt, &mut locals.clone(), keys);
            if let Some(else_stmt) = else_stmt {
                collect_stmt_folded_int_consts(else_stmt, &mut locals.clone(), keys);
            }
        }
        Stmt::IfLet {
            value,
            then_stmt,
            else_stmt,
            ..
        } => {
            collect_expr_folded_int_consts(value, locals, keys);
            collect_stmt_folded_int_consts(then_stmt, &mut locals.clone(), keys);
            if let Some(else_stmt) = else_stmt {
                collect_stmt_folded_int_consts(else_stmt, &mut locals.clone(), keys);
            }
        }
        Stmt::While { condition, body } => {
            collect_expr_folded_int_consts(condition, locals, keys);
            collect_stmt_folded_int_consts(body, &mut locals.clone(), keys);
        }
        Stmt::WhileLet { value, body, .. } => {
            collect_expr_folded_int_consts(value, locals, keys);
            collect_stmt_folded_int_consts(body, &mut locals.clone(), keys);
        }
        Stmt::For { iterable, body, .. } => {
            collect_expr_folded_int_consts(iterable, locals, keys);
            collect_stmt_folded_int_consts(body, &mut locals.clone(), keys);
        }
        Stmt::Block { statements } => {
            let mut scoped = locals.clone();
            for stmt in statements {
                collect_stmt_folded_int_consts(stmt, &mut scoped, keys);
            }
        }
        Stmt::Function { named_params, .. } => {
            for param in named_params {
                if let Some(default) = &param.default {
                    collect_expr_folded_int_consts(default, locals, keys);
                }
            }
        }
        Stmt::Impl { methods, .. } => {
            for method in methods {
                collect_stmt_folded_int_consts(method, &mut HashMap::new(), keys);
            }
        }
    }
}

fn collect_expr_folded_int_consts(expr: &Expr, locals: &HashMap<String, i64>, keys: &mut Vec<ScalarLoopConstKey>) {
    if let Some(value) = folded_int_expr_value(expr, locals) {
        keys.push(ScalarLoopConstKey::Int(value));
    }
    match expr {
        Expr::Paren(inner) | Expr::Unary(_, inner) => collect_expr_folded_int_consts(inner, locals, keys),
        Expr::Bin(lhs, _, rhs)
        | Expr::And(lhs, rhs)
        | Expr::Or(lhs, rhs)
        | Expr::NullishCoalescing(lhs, rhs)
        | Expr::Access(lhs, rhs)
        | Expr::OptionalAccess(lhs, rhs) => {
            collect_expr_folded_int_consts(lhs, locals, keys);
            collect_expr_folded_int_consts(rhs, locals, keys);
        }
        Expr::Conditional(condition, then_expr, else_expr) => {
            collect_expr_folded_int_consts(condition, locals, keys);
            collect_expr_folded_int_consts(then_expr, locals, keys);
            collect_expr_folded_int_consts(else_expr, locals, keys);
        }
        Expr::Call(_, args) => collect_boxed_exprs_folded_int_consts(args, locals, keys),
        Expr::CallExpr(callee, args) => {
            collect_expr_folded_int_consts(callee, locals, keys);
            collect_boxed_exprs_folded_int_consts(args, locals, keys);
        }
        Expr::CallNamed(callee, positional, named) => {
            collect_expr_folded_int_consts(callee, locals, keys);
            collect_boxed_exprs_folded_int_consts(positional, locals, keys);
            for (_, value) in named {
                collect_expr_folded_int_consts(value, locals, keys);
            }
        }
        Expr::List(values) => collect_boxed_exprs_folded_int_consts(values, locals, keys),
        Expr::Map(entries) => {
            for (key, value) in entries {
                collect_expr_folded_int_consts(key, locals, keys);
                collect_expr_folded_int_consts(value, locals, keys);
            }
        }
        Expr::StructLiteral { fields, .. } => {
            for (_, value) in fields {
                collect_expr_folded_int_consts(value, locals, keys);
            }
        }
        Expr::TemplateString(parts) => {
            for part in parts {
                if let crate::expr::TemplateStringPart::Expr(expr) = part {
                    collect_expr_folded_int_consts(expr, locals, keys);
                }
            }
        }
        Expr::Block(statements) => {
            let mut scoped = locals.clone();
            for stmt in statements {
                collect_stmt_folded_int_consts(stmt, &mut scoped, keys);
            }
        }
        Expr::Range { start, end, step, .. } => {
            for expr in [start, end, step].into_iter().flatten() {
                collect_expr_folded_int_consts(expr, locals, keys);
            }
        }
        Expr::Select { cases, default_case } => {
            for case in cases {
                if let Some(guard) = &case.guard {
                    collect_expr_folded_int_consts(guard, locals, keys);
                }
                collect_expr_folded_int_consts(&case.body, locals, keys);
            }
            if let Some(default_case) = default_case {
                collect_expr_folded_int_consts(default_case, locals, keys);
            }
        }
        Expr::Match { value, arms } => {
            collect_expr_folded_int_consts(value, locals, keys);
            for arm in arms {
                collect_expr_folded_int_consts(&arm.body, locals, keys);
            }
        }
        Expr::Closure { body, .. } => collect_expr_folded_int_consts(body, locals, keys),
        Expr::Literal(_) | Expr::Var(_) => {}
    }
}

fn collect_boxed_exprs_folded_int_consts(
    exprs: &[Box<Expr>],
    locals: &HashMap<String, i64>,
    keys: &mut Vec<ScalarLoopConstKey>,
) {
    for expr in exprs {
        collect_expr_folded_int_consts(expr, locals, keys);
    }
}

fn folded_int_expr_value(expr: &Expr, locals: &HashMap<String, i64>) -> Option<i64> {
    cached_loop_int_expr_value_from_locals(expr, &|name| locals.get(name).copied())
}

fn cached_loop_int_expr_value_from_locals(expr: &Expr, local_value: &impl Fn(&str) -> Option<i64>) -> Option<i64> {
    match expr {
        Expr::Paren(inner) => cached_loop_int_expr_value_from_locals(inner, local_value),
        Expr::Literal(LiteralVal::Int(value)) => Some(*value),
        Expr::Var(name) => local_value(name),
        Expr::Bin(lhs, op, rhs) => {
            let lhs = cached_loop_int_expr_value_from_locals(lhs, local_value)?;
            let rhs = cached_loop_int_expr_value_from_locals(rhs, local_value)?;
            match op {
                crate::operator::BinOp::Add => Some(lhs.wrapping_add(rhs)),
                crate::operator::BinOp::Sub => Some(lhs.wrapping_sub(rhs)),
                crate::operator::BinOp::Mul => Some(lhs.wrapping_mul(rhs)),
                crate::operator::BinOp::Div if rhs != 0 => Some(lhs / rhs),
                crate::operator::BinOp::Mod if rhs != 0 => Some(lhs % rhs),
                _ => None,
            }
        }
        _ => None,
    }
}

fn collect_boxed_exprs_scalar_consts(exprs: &[Box<Expr>], keys: &mut Vec<ScalarLoopConstKey>) {
    for expr in exprs {
        collect_expr_scalar_consts(expr, keys);
    }
}

fn collect_stmt_inline_call_scalar_consts(
    stmt: &Stmt,
    bodies: &HashMap<String, FunctionInlineBody>,
    visiting: &mut HashSet<String>,
    keys: &mut Vec<ScalarLoopConstKey>,
) {
    match stmt {
        Stmt::Attributed { item, .. } => collect_stmt_inline_call_scalar_consts(item, bodies, visiting, keys),
        Stmt::Empty | Stmt::Break | Stmt::Continue | Stmt::Import(_) | Stmt::Struct { .. } | Stmt::TypeAlias { .. } => {
        }
        Stmt::Expr(expr) | Stmt::Return { value: Some(expr) } => {
            collect_expr_inline_call_scalar_consts(expr, bodies, visiting, keys);
        }
        Stmt::Return { value: None } => {}
        Stmt::Let { value, .. } | Stmt::Define { value, .. } => {
            collect_expr_inline_call_scalar_consts(value, bodies, visiting, keys);
        }
        Stmt::Assign { value, .. } | Stmt::CompoundAssign { value, .. } => {
            collect_expr_inline_call_scalar_consts(value, bodies, visiting, keys);
        }
        Stmt::If {
            condition,
            then_stmt,
            else_stmt,
        } => {
            collect_expr_inline_call_scalar_consts(condition, bodies, visiting, keys);
            collect_stmt_inline_call_scalar_consts(then_stmt, bodies, visiting, keys);
            if let Some(else_stmt) = else_stmt {
                collect_stmt_inline_call_scalar_consts(else_stmt, bodies, visiting, keys);
            }
        }
        Stmt::IfLet {
            value,
            then_stmt,
            else_stmt,
            ..
        } => {
            collect_expr_inline_call_scalar_consts(value, bodies, visiting, keys);
            collect_stmt_inline_call_scalar_consts(then_stmt, bodies, visiting, keys);
            if let Some(else_stmt) = else_stmt {
                collect_stmt_inline_call_scalar_consts(else_stmt, bodies, visiting, keys);
            }
        }
        Stmt::While { condition, body } => {
            collect_expr_inline_call_scalar_consts(condition, bodies, visiting, keys);
            collect_stmt_inline_call_scalar_consts(body, bodies, visiting, keys);
        }
        Stmt::WhileLet { value, body, .. } => {
            collect_expr_inline_call_scalar_consts(value, bodies, visiting, keys);
            collect_stmt_inline_call_scalar_consts(body, bodies, visiting, keys);
        }
        Stmt::For { iterable, body, .. } => {
            collect_expr_inline_call_scalar_consts(iterable, bodies, visiting, keys);
            collect_stmt_inline_call_scalar_consts(body, bodies, visiting, keys);
        }
        Stmt::Block { statements } => {
            for stmt in statements {
                collect_stmt_inline_call_scalar_consts(stmt, bodies, visiting, keys);
            }
        }
        Stmt::Function { named_params, .. } => {
            for param in named_params {
                if let Some(default) = &param.default {
                    collect_expr_inline_call_scalar_consts(default, bodies, visiting, keys);
                }
            }
        }
        Stmt::Trait { .. } => {}
        Stmt::Impl { methods, .. } => {
            for method in methods {
                collect_stmt_inline_call_scalar_consts(method, bodies, visiting, keys);
            }
        }
    }
}

fn collect_expr_inline_call_scalar_consts(
    expr: &Expr,
    bodies: &HashMap<String, FunctionInlineBody>,
    visiting: &mut HashSet<String>,
    keys: &mut Vec<ScalarLoopConstKey>,
) {
    match expr {
        Expr::Call(name, args) => {
            collect_inline_body_scalar_consts(name, args.len(), bodies, visiting, keys);
            collect_boxed_exprs_inline_call_scalar_consts(args, bodies, visiting, keys);
        }
        Expr::CallExpr(callee, args) => {
            if let Expr::Var(name) = callee.as_ref() {
                collect_inline_body_scalar_consts(name, args.len(), bodies, visiting, keys);
            }
            collect_expr_inline_call_scalar_consts(callee, bodies, visiting, keys);
            collect_boxed_exprs_inline_call_scalar_consts(args, bodies, visiting, keys);
        }
        Expr::Paren(inner) | Expr::Unary(_, inner) => {
            collect_expr_inline_call_scalar_consts(inner, bodies, visiting, keys)
        }
        Expr::OptionalAccess(inner, field) | Expr::Access(inner, field) => {
            collect_expr_inline_call_scalar_consts(inner, bodies, visiting, keys);
            collect_expr_inline_call_scalar_consts(field, bodies, visiting, keys);
        }
        Expr::Bin(lhs, _, rhs) | Expr::And(lhs, rhs) | Expr::Or(lhs, rhs) | Expr::NullishCoalescing(lhs, rhs) => {
            collect_expr_inline_call_scalar_consts(lhs, bodies, visiting, keys);
            collect_expr_inline_call_scalar_consts(rhs, bodies, visiting, keys);
        }
        Expr::List(elements) => collect_boxed_exprs_inline_call_scalar_consts(elements, bodies, visiting, keys),
        Expr::Map(entries) => {
            for (key, value) in entries {
                collect_expr_inline_call_scalar_consts(key, bodies, visiting, keys);
                collect_expr_inline_call_scalar_consts(value, bodies, visiting, keys);
            }
        }
        Expr::StructLiteral { fields, .. } => {
            for (_, value) in fields {
                collect_expr_inline_call_scalar_consts(value, bodies, visiting, keys);
            }
        }
        Expr::CallNamed(callee, positional, named) => {
            collect_expr_inline_call_scalar_consts(callee, bodies, visiting, keys);
            collect_boxed_exprs_inline_call_scalar_consts(positional, bodies, visiting, keys);
            for (_, value) in named {
                collect_expr_inline_call_scalar_consts(value, bodies, visiting, keys);
            }
        }
        Expr::TemplateString(parts) => {
            for part in parts {
                match part {
                    crate::expr::TemplateStringPart::Literal(value) => {
                        if let Some(key) =
                            ShortStr::new(value).map(|_| ScalarLoopConstKey::ShortString(value.to_owned()))
                        {
                            keys.push(key);
                        }
                    }
                    crate::expr::TemplateStringPart::Expr(expr) => {
                        collect_expr_inline_call_scalar_consts(expr, bodies, visiting, keys);
                    }
                }
            }
        }
        Expr::Block(statements) => {
            for stmt in statements {
                collect_stmt_inline_call_scalar_consts(stmt, bodies, visiting, keys);
            }
        }
        Expr::Range { start, end, step, .. } => {
            for expr in [start, end, step].into_iter().flatten() {
                collect_expr_inline_call_scalar_consts(expr, bodies, visiting, keys);
            }
        }
        Expr::Select { cases, default_case } => {
            for SelectCase { guard, body, .. } in cases {
                if let Some(guard) = guard {
                    collect_expr_inline_call_scalar_consts(guard, bodies, visiting, keys);
                }
                collect_expr_inline_call_scalar_consts(body, bodies, visiting, keys);
            }
            if let Some(default_case) = default_case {
                collect_expr_inline_call_scalar_consts(default_case, bodies, visiting, keys);
            }
        }
        Expr::Match { value, arms } => {
            collect_expr_inline_call_scalar_consts(value, bodies, visiting, keys);
            for MatchArm { body, .. } in arms {
                collect_expr_inline_call_scalar_consts(body, bodies, visiting, keys);
            }
        }
        Expr::Conditional(condition, then_expr, else_expr) => {
            collect_expr_inline_call_scalar_consts(condition, bodies, visiting, keys);
            collect_expr_inline_call_scalar_consts(then_expr, bodies, visiting, keys);
            collect_expr_inline_call_scalar_consts(else_expr, bodies, visiting, keys);
        }
        Expr::Closure { body, .. } => collect_expr_inline_call_scalar_consts(body, bodies, visiting, keys),
        Expr::Literal(_) | Expr::Var(_) => {}
    }
}

fn collect_inline_body_scalar_consts(
    name: &str,
    arg_count: usize,
    bodies: &HashMap<String, FunctionInlineBody>,
    visiting: &mut HashSet<String>,
    keys: &mut Vec<ScalarLoopConstKey>,
) {
    let Some(body) = bodies.get(name) else {
        return;
    };
    if body.named_param_count != 0
        || body.params.len() != arg_count
        || !inline_body_is_supported(&body.body)
        || stmt_contains_call_to(&body.body, name)
        || !visiting.insert(name.to_owned())
    {
        return;
    }
    collect_stmt_scalar_consts(&body.body, keys);
    collect_stmt_inline_call_scalar_consts(&body.body, bodies, visiting, keys);
    visiting.remove(name);
}

fn collect_boxed_exprs_inline_call_scalar_consts(
    exprs: &[Box<Expr>],
    bodies: &HashMap<String, FunctionInlineBody>,
    visiting: &mut HashSet<String>,
    keys: &mut Vec<ScalarLoopConstKey>,
) {
    for expr in exprs {
        collect_expr_inline_call_scalar_consts(expr, bodies, visiting, keys);
    }
}

fn collect_stmt_const_map_get_scalar_consts(
    stmt: &Stmt,
    const_maps: &HashMap<String, FastHashMap<RuntimeMapKey, ConstRuntimeValue>>,
    keys: &mut Vec<ScalarLoopConstKey>,
) -> Result<()> {
    match stmt {
        Stmt::Attributed { item, .. } => collect_stmt_const_map_get_scalar_consts(item, const_maps, keys)?,
        Stmt::Empty | Stmt::Break | Stmt::Continue | Stmt::Import(_) | Stmt::Struct { .. } | Stmt::TypeAlias { .. } => {
        }
        Stmt::Expr(expr) | Stmt::Return { value: Some(expr) } => {
            collect_expr_const_map_get_scalar_consts(expr, const_maps, keys)?;
        }
        Stmt::Return { value: None } => {}
        Stmt::Let { value, .. } | Stmt::Define { value, .. } => {
            collect_expr_const_map_get_scalar_consts(value, const_maps, keys)?;
        }
        Stmt::Assign { value, .. } | Stmt::CompoundAssign { value, .. } => {
            collect_expr_const_map_get_scalar_consts(value, const_maps, keys)?;
        }
        Stmt::If {
            condition,
            then_stmt,
            else_stmt,
        } => {
            collect_expr_const_map_get_scalar_consts(condition, const_maps, keys)?;
            collect_stmt_const_map_get_scalar_consts(then_stmt, const_maps, keys)?;
            if let Some(else_stmt) = else_stmt {
                collect_stmt_const_map_get_scalar_consts(else_stmt, const_maps, keys)?;
            }
        }
        Stmt::IfLet {
            value,
            then_stmt,
            else_stmt,
            ..
        } => {
            collect_expr_const_map_get_scalar_consts(value, const_maps, keys)?;
            collect_stmt_const_map_get_scalar_consts(then_stmt, const_maps, keys)?;
            if let Some(else_stmt) = else_stmt {
                collect_stmt_const_map_get_scalar_consts(else_stmt, const_maps, keys)?;
            }
        }
        Stmt::While { condition, body } => {
            collect_expr_const_map_get_scalar_consts(condition, const_maps, keys)?;
            collect_stmt_const_map_get_scalar_consts(body, const_maps, keys)?;
        }
        Stmt::WhileLet { value, body, .. } => {
            collect_expr_const_map_get_scalar_consts(value, const_maps, keys)?;
            collect_stmt_const_map_get_scalar_consts(body, const_maps, keys)?;
        }
        Stmt::For { iterable, body, .. } => {
            collect_expr_const_map_get_scalar_consts(iterable, const_maps, keys)?;
            collect_stmt_const_map_get_scalar_consts(body, const_maps, keys)?;
        }
        Stmt::Block { statements } => {
            for stmt in statements {
                collect_stmt_const_map_get_scalar_consts(stmt, const_maps, keys)?;
            }
        }
        Stmt::Function { named_params, .. } => {
            for param in named_params {
                if let Some(default) = &param.default {
                    collect_expr_const_map_get_scalar_consts(default, const_maps, keys)?;
                }
            }
        }
        Stmt::Trait { .. } => {}
        Stmt::Impl { methods, .. } => {
            for method in methods {
                collect_stmt_const_map_get_scalar_consts(method, const_maps, keys)?;
            }
        }
    }
    Ok(())
}

fn collect_expr_const_map_get_scalar_consts(
    expr: &Expr,
    const_maps: &HashMap<String, FastHashMap<RuntimeMapKey, ConstRuntimeValue>>,
    keys: &mut Vec<ScalarLoopConstKey>,
) -> Result<()> {
    if let Some(key) = const_map_get_scalar_loop_key(expr, const_maps)? {
        keys.push(key);
    }
    match expr {
        Expr::Literal(_) | Expr::Var(_) => {}
        Expr::Paren(inner) | Expr::Unary(_, inner) => {
            collect_expr_const_map_get_scalar_consts(inner, const_maps, keys)?;
        }
        Expr::OptionalAccess(inner, field) | Expr::Access(inner, field) => {
            collect_expr_const_map_get_scalar_consts(inner, const_maps, keys)?;
            collect_expr_const_map_get_scalar_consts(field, const_maps, keys)?;
        }
        Expr::CallExpr(callee, args) => {
            collect_expr_const_map_get_scalar_consts(callee, const_maps, keys)?;
            collect_boxed_exprs_const_map_get_scalar_consts(args, const_maps, keys)?;
        }
        Expr::Call(_, args) => collect_boxed_exprs_const_map_get_scalar_consts(args, const_maps, keys)?,
        Expr::CallNamed(callee, positional, named) => {
            collect_expr_const_map_get_scalar_consts(callee, const_maps, keys)?;
            collect_boxed_exprs_const_map_get_scalar_consts(positional, const_maps, keys)?;
            for (_, arg) in named {
                collect_expr_const_map_get_scalar_consts(arg, const_maps, keys)?;
            }
        }
        Expr::Bin(lhs, _, rhs) | Expr::And(lhs, rhs) | Expr::Or(lhs, rhs) | Expr::NullishCoalescing(lhs, rhs) => {
            collect_expr_const_map_get_scalar_consts(lhs, const_maps, keys)?;
            collect_expr_const_map_get_scalar_consts(rhs, const_maps, keys)?;
        }
        Expr::List(elements) => collect_boxed_exprs_const_map_get_scalar_consts(elements, const_maps, keys)?,
        Expr::Map(entries) => {
            for (key, value) in entries {
                collect_expr_const_map_get_scalar_consts(key, const_maps, keys)?;
                collect_expr_const_map_get_scalar_consts(value, const_maps, keys)?;
            }
        }
        Expr::StructLiteral { fields, .. } => {
            for (_, value) in fields {
                collect_expr_const_map_get_scalar_consts(value, const_maps, keys)?;
            }
        }
        Expr::Closure { body, .. } => {
            collect_expr_const_map_get_scalar_consts(body, const_maps, keys)?;
        }
        Expr::TemplateString(parts) => {
            for part in parts {
                if let crate::expr::TemplateStringPart::Expr(expr) = part {
                    collect_expr_const_map_get_scalar_consts(expr, const_maps, keys)?;
                }
            }
        }
        Expr::Block(statements) => {
            for stmt in statements {
                collect_stmt_const_map_get_scalar_consts(stmt, const_maps, keys)?;
            }
        }
        Expr::Range { start, end, step, .. } => {
            for expr in [start, end, step].into_iter().flatten() {
                collect_expr_const_map_get_scalar_consts(expr, const_maps, keys)?;
            }
        }
        Expr::Match { value, arms } => {
            collect_expr_const_map_get_scalar_consts(value, const_maps, keys)?;
            for arm in arms {
                collect_expr_const_map_get_scalar_consts(&arm.body, const_maps, keys)?;
            }
        }
        Expr::Conditional(condition, then_expr, else_expr) => {
            collect_expr_const_map_get_scalar_consts(condition, const_maps, keys)?;
            collect_expr_const_map_get_scalar_consts(then_expr, const_maps, keys)?;
            collect_expr_const_map_get_scalar_consts(else_expr, const_maps, keys)?;
        }
        Expr::Select { cases, default_case } => {
            for case in cases {
                if let Some(guard) = &case.guard {
                    collect_expr_const_map_get_scalar_consts(guard, const_maps, keys)?;
                }
                collect_expr_const_map_get_scalar_consts(&case.body, const_maps, keys)?;
            }
            if let Some(default_case) = default_case {
                collect_expr_const_map_get_scalar_consts(default_case, const_maps, keys)?;
            }
        }
    }
    Ok(())
}

fn collect_boxed_exprs_const_map_get_scalar_consts(
    exprs: &[Box<Expr>],
    const_maps: &HashMap<String, FastHashMap<RuntimeMapKey, ConstRuntimeValue>>,
    keys: &mut Vec<ScalarLoopConstKey>,
) -> Result<()> {
    for expr in exprs {
        collect_expr_const_map_get_scalar_consts(expr, const_maps, keys)?;
    }
    Ok(())
}

fn const_map_get_scalar_loop_key(
    expr: &Expr,
    const_maps: &HashMap<String, FastHashMap<RuntimeMapKey, ConstRuntimeValue>>,
) -> Result<Option<ScalarLoopConstKey>> {
    let Some((target, key)) = const_map_get_target_and_key(expr) else {
        return Ok(None);
    };
    let Some(target_name) = super::support::simple_local_expr_name(target) else {
        return Ok(None);
    };
    let Some(map) = const_maps.get(target_name) else {
        return Ok(None);
    };
    let Some(key) = const_map_key_from_expr(key)? else {
        return Ok(None);
    };
    Ok(map.get(&key).and_then(const_runtime_scalar_loop_key))
}

fn const_map_get_target_and_key(expr: &Expr) -> Option<(&Expr, &Expr)> {
    let Expr::CallExpr(callee, args) = expr else {
        return None;
    };
    if let Some((target, key)) = map_get_method_call_args(callee, args) {
        return Some((target, key));
    }
    if args.len() != 2 {
        return None;
    }
    let Expr::Access(target, method) = callee.as_ref() else {
        return None;
    };
    if !matches!(target.as_ref(), Expr::Var(name) if name == "map") || method_name(method) != Some("get") {
        return None;
    }
    Some((args[0].as_ref(), args[1].as_ref()))
}

fn const_map_key_from_expr(expr: &Expr) -> Result<Option<RuntimeMapKey>> {
    match expr {
        Expr::Paren(inner) => const_map_key_from_expr(inner),
        Expr::Literal(value) => const_runtime_map_key_from_literal(value),
        _ => Ok(None),
    }
}

fn method_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Var(name) => Some(name.as_str()),
        Expr::Literal(value) => value.as_str(),
        _ => None,
    }
}

fn const_runtime_scalar_loop_key(value: &ConstRuntimeValue) -> Option<ScalarLoopConstKey> {
    match value {
        ConstRuntimeValue::Nil => Some(ScalarLoopConstKey::Nil),
        ConstRuntimeValue::Bool(value) => Some(ScalarLoopConstKey::Bool(*value)),
        ConstRuntimeValue::Int(value) => Some(ScalarLoopConstKey::Int(*value)),
        ConstRuntimeValue::Float(value) => Some(ScalarLoopConstKey::Float(value.to_bits())),
        ConstRuntimeValue::ShortStr(value) => Some(ScalarLoopConstKey::ShortString(value.as_str().to_owned())),
        ConstRuntimeValue::Heap(_) => None,
    }
}

fn scalar_loop_const_key(value: &LiteralVal) -> Option<ScalarLoopConstKey> {
    match value {
        LiteralVal::Nil => Some(ScalarLoopConstKey::Nil),
        LiteralVal::Bool(value) => Some(ScalarLoopConstKey::Bool(*value)),
        LiteralVal::Int(value) => Some(ScalarLoopConstKey::Int(*value)),
        LiteralVal::Float(value) => Some(ScalarLoopConstKey::Float(value.to_bits())),
        value => {
            let value = value.as_str()?;
            ShortStr::new(value).map(|_| ScalarLoopConstKey::ShortString(value.to_owned()))
        }
    }
}
