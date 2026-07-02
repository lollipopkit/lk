use anyhow::{Result, bail};
use std::collections::HashSet;

use crate::{
    expr::{Expr, Pattern, SelectPattern},
    stmt::Stmt,
};

use super::{Compiler, support::mutated_names_in_stmt};

#[derive(Default)]
struct InlineReturnPatches {
    exit_jumps: Vec<usize>,
}

impl Compiler {
    pub(super) fn try_inline_direct_function_call(
        &mut self,
        function_name: &str,
        args: &[Box<Expr>],
    ) -> Result<Option<u16>> {
        if self.inline_stack.iter().any(|name| name == function_name) {
            return Ok(None);
        }
        let Some(body) = self.function_bodies.get(function_name).cloned() else {
            return Ok(None);
        };
        if body.named_param_count != 0 || body.params.len() != args.len() {
            return Ok(None);
        }
        if !inline_body_is_supported(&body.body) || stmt_contains_call_to(&body.body, function_name) {
            return Ok(None);
        }
        let local_names = local_names_in_inline_body(&body.body, &body.params);
        if !assigned_names_in_stmt(&body.body)
            .iter()
            .all(|name| local_names.contains(name))
        {
            return Ok(None);
        }

        self.inline_stack.push(function_name.to_owned());
        let result = self.inline_direct_function_body(&body.params, args, &body.body);
        self.inline_stack.pop();
        result.map(Some)
    }

    fn inline_direct_function_body(&mut self, params: &[String], args: &[Box<Expr>], body: &Stmt) -> Result<u16> {
        let saved_locals = self.locals.clone();
        let saved_cell_locals = self.cell_locals.clone();
        let mutated_names = mutated_names_in_stmt(body);

        let result = (|| {
            for (param, arg) in params.iter().zip(args.iter()) {
                if mutated_names.contains(param) {
                    let slot = self.alloc_reg();
                    if !self.try_lower_expr_to_register(slot, arg)? {
                        let arg = self.lower_expr(arg)?;
                        let move_source = !self.is_current_local_slot(arg);
                        self.emit_move_with_policy(slot, arg, "inline param", move_source)?;
                    }
                    self.insert_local(param.clone(), slot);
                } else {
                    let arg = self.lower_inline_readonly_arg(arg)?;
                    self.insert_local(param.clone(), arg);
                }
            }
            self.lower_inline_body(body)
        })();

        self.locals = saved_locals;
        self.cell_locals = saved_cell_locals;
        result
    }

    fn lower_inline_readonly_arg(&mut self, arg: &Expr) -> Result<u16> {
        self.lower_readonly_operand(arg)
    }

    fn lower_inline_body(&mut self, body: &Stmt) -> Result<u16> {
        let result = self.alloc_reg();
        let mut returns = InlineReturnPatches::default();
        match body {
            Stmt::Attributed { item, .. } => return self.lower_inline_body(item),
            Stmt::Block { statements } => self.lower_inline_block(statements, result, &mut returns)?,
            Stmt::Return { value: Some(value) } => self.lower_inline_return(value, result, &mut returns, true)?,
            _ => bail!("Compiler unsupported inline function body"),
        }
        let end = self.function.code.len();
        for pc in returns.exit_jumps {
            self.patch_jmp(pc, end)?;
        }
        Ok(result)
    }

    fn lower_inline_block(
        &mut self,
        statements: &[Box<Stmt>],
        result: u16,
        returns: &mut InlineReturnPatches,
    ) -> Result<()> {
        let Some((last, prefix)) = statements.split_last() else {
            bail!("Compiler cannot inline empty function body");
        };
        for stmt in prefix {
            self.lower_inline_stmt(stmt, result, returns, false)?;
        }
        match last.as_ref() {
            Stmt::Attributed { item, .. } => self.lower_inline_stmt(item, result, returns, true),
            Stmt::Return { value: Some(value) } => self.lower_inline_return(value, result, returns, true),
            _ => bail!("Compiler inline function body must end with return value"),
        }
    }

    fn lower_inline_stmt(
        &mut self,
        stmt: &Stmt,
        result: u16,
        returns: &mut InlineReturnPatches,
        tail_position: bool,
    ) -> Result<()> {
        match stmt {
            Stmt::Attributed { item, .. } => self.lower_inline_stmt(item, result, returns, tail_position),
            Stmt::Block { statements } => {
                self.local_rebind_suppression += 1;
                self.lower_inline_stmt_sequence(statements, result, returns)?;
                self.local_rebind_suppression -= 1;
                Ok(())
            }
            Stmt::Let {
                pattern: Pattern::Variable(name),
                value,
                ..
            }
            | Stmt::Define { name, value } => {
                let slot = self.alloc_reg();
                if !self.try_lower_expr_to_register(slot, value)? {
                    let value = self.lower_expr(value)?;
                    let move_source = !self.is_current_local_slot(value);
                    self.emit_move_with_policy(slot, value, "inline local", move_source)?;
                }
                self.insert_local(name.clone(), slot);
                Ok(())
            }
            Stmt::Assign { name, value, .. } => self.lower_assign(name, value),
            Stmt::CompoundAssign { name, op, value, .. } => self.lower_compound_assign(name, op, value),
            Stmt::If {
                condition,
                then_stmt,
                else_stmt,
            } => self.lower_inline_if(condition, then_stmt, else_stmt.as_deref(), result, returns),
            Stmt::While { condition, body } => self.lower_inline_while(condition, body, result, returns),
            Stmt::Return { value: Some(value) } => self.lower_inline_return(value, result, returns, tail_position),
            Stmt::Expr(expr) if inline_dead_expr_is_supported(expr) => {
                self.lower_expr(expr)?;
                Ok(())
            }
            _ => bail!("Compiler unsupported inline prefix statement"),
        }
    }

    fn lower_inline_stmt_sequence(
        &mut self,
        statements: &[Box<Stmt>],
        result: u16,
        returns: &mut InlineReturnPatches,
    ) -> Result<()> {
        let mut index = 0;
        while index < statements.len() {
            if index + 1 < statements.len()
                && self.try_lower_move2_assign_pair(statements[index].as_ref(), statements[index + 1].as_ref())?
            {
                index += 2;
            } else {
                self.lower_inline_stmt(statements[index].as_ref(), result, returns, false)?;
                index += 1;
            }
        }
        Ok(())
    }

    fn lower_inline_return(
        &mut self,
        value: &Expr,
        result: u16,
        returns: &mut InlineReturnPatches,
        tail_position: bool,
    ) -> Result<()> {
        self.lower_expr_to_register(result, value, "inline return")?;
        if !tail_position {
            let exit = self.emit_jmp_placeholder();
            returns.exit_jumps.push(exit);
        }
        Ok(())
    }

    fn lower_inline_if(
        &mut self,
        condition: &Expr,
        then_stmt: &Stmt,
        else_stmt: Option<&Stmt>,
        result: u16,
        returns: &mut InlineReturnPatches,
    ) -> Result<()> {
        let watermark = self.next_reg;
        let false_jumps = self.emit_condition_false_jumps(condition)?;

        self.local_rebind_suppression += 1;
        self.lower_inline_stmt(then_stmt, result, returns, false)?;
        self.local_rebind_suppression -= 1;
        self.next_reg = watermark;

        if let Some(else_stmt) = else_stmt {
            let jmp_end = self.emit_jmp_placeholder();
            let else_start = self.function.code.len();
            self.patch_condition_false_jumps(false_jumps, else_start)?;

            self.local_rebind_suppression += 1;
            self.lower_inline_stmt(else_stmt, result, returns, false)?;
            self.local_rebind_suppression -= 1;
            self.next_reg = watermark;

            let end = self.function.code.len();
            self.patch_jmp(jmp_end, end)?;
        } else {
            let end = self.function.code.len();
            self.patch_condition_false_jumps(false_jumps, end)?;
        }
        self.emitted_return = false;
        Ok(())
    }

    fn lower_inline_while(
        &mut self,
        condition: &Expr,
        body: &Stmt,
        result: u16,
        returns: &mut InlineReturnPatches,
    ) -> Result<()> {
        let watermark = self.next_reg;
        self.begin_loop_scalar_const_scope(condition, body)?;
        let condition_start = self.function.code.len();
        let false_jumps = self.emit_condition_false_jumps(condition)?;
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

        self.local_rebind_suppression += 1;
        self.lower_inline_stmt(body, result, returns, false)?;
        self.local_rebind_suppression -= 1;
        self.next_reg = watermark;
        let jmp_back = self.emit_jmp_placeholder();
        self.patch_jmp(jmp_back, loop_start)?;

        let end = self.function.code.len();
        self.patch_condition_false_jumps(false_jumps, end)?;
        self.emitted_return = false;
        self.end_loop_scalar_const_scope();
        self.next_reg = watermark;
        Ok(())
    }
}

fn inline_stmt_is_supported(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Attributed { item, .. } => inline_stmt_is_supported(item),
        Stmt::Block { statements } => statements.iter().all(|stmt| inline_stmt_is_supported(stmt)),
        Stmt::Let {
            pattern: Pattern::Variable(_),
            value,
            ..
        }
        | Stmt::Define { value, .. } => inline_expr_is_supported(value),
        Stmt::Assign { value, .. } | Stmt::CompoundAssign { value, .. } => inline_expr_is_supported(value),
        Stmt::If {
            condition,
            then_stmt,
            else_stmt,
        } => {
            inline_expr_is_supported(condition)
                && inline_stmt_is_supported(then_stmt)
                && else_stmt.as_ref().is_none_or(|stmt| inline_stmt_is_supported(stmt))
        }
        Stmt::While { condition, body } => inline_expr_is_supported(condition) && inline_stmt_is_supported(body),
        Stmt::Return { value: Some(value) } => inline_expr_is_supported(value),
        Stmt::Expr(expr) => inline_dead_expr_is_supported(expr),
        _ => false,
    }
}

fn inline_prefix_stmt_is_supported(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Attributed { item, .. } => inline_prefix_stmt_is_supported(item),
        Stmt::Return { .. } => false,
        _ => inline_stmt_is_supported(stmt),
    }
}

fn inline_tail_return_is_supported(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Attributed { item, .. } => inline_tail_return_is_supported(item),
        Stmt::Return { value: Some(value) } => inline_expr_is_supported(value),
        _ => false,
    }
}

fn inline_block_is_supported(statements: &[Box<Stmt>]) -> bool {
    let Some((last, prefix)) = statements.split_last() else {
        return false;
    };
    !prefix.is_empty()
        && prefix.iter().all(|stmt| inline_prefix_stmt_is_supported(stmt))
        && inline_tail_return_is_supported(last)
}

pub(super) fn inline_body_is_supported(body: &Stmt) -> bool {
    match body {
        Stmt::Attributed { item, .. } => inline_body_is_supported(item),
        Stmt::Block { statements } => inline_block_is_supported(statements),
        _ => false,
    }
}

fn inline_dead_expr_is_supported(expr: &Expr) -> bool {
    matches!(expr, Expr::Literal(_))
}

fn inline_expr_is_supported(expr: &Expr) -> bool {
    match expr {
        Expr::Paren(inner) | Expr::Unary(_, inner) | Expr::OptionalAccess(inner, _) => inline_expr_is_supported(inner),
        Expr::Literal(_) | Expr::Var(_) => true,
        Expr::Bin(lhs, _, rhs)
        | Expr::And(lhs, rhs)
        | Expr::Or(lhs, rhs)
        | Expr::NullishCoalescing(lhs, rhs)
        | Expr::Access(lhs, rhs) => inline_expr_is_supported(lhs) && inline_expr_is_supported(rhs),
        Expr::Conditional(condition, then_expr, else_expr) => {
            inline_expr_is_supported(condition)
                && inline_expr_is_supported(then_expr)
                && inline_expr_is_supported(else_expr)
        }
        Expr::Call(name, args) => {
            !args.is_empty() && !name.is_empty() && args.iter().all(|arg| inline_expr_is_supported(arg))
        }
        Expr::CallExpr(callee, args) => {
            !inline_call_expr_uses_runtime_method_helper(callee)
                && inline_expr_is_supported(callee)
                && args.iter().all(|arg| inline_expr_is_supported(arg))
        }
        Expr::List(values) => values.iter().all(|value| inline_expr_is_supported(value)),
        Expr::Map(entries) => entries
            .iter()
            .all(|(key, value)| inline_expr_is_supported(key) && inline_expr_is_supported(value)),
        Expr::TemplateString(parts) => parts.iter().all(|part| match part {
            crate::expr::TemplateStringPart::Literal(_) => true,
            crate::expr::TemplateStringPart::Expr(expr) => inline_expr_is_supported(expr),
        }),
        _ => false,
    }
}

fn inline_call_expr_uses_runtime_method_helper(callee: &Expr) -> bool {
    let Expr::Access(_, field) = callee else {
        return false;
    };
    matches!(
        field.as_ref(),
        Expr::Literal(crate::val::LiteralVal::String(method)) if method.as_ref() == "starts_with"
    )
}

pub(super) fn stmt_contains_call_to(stmt: &Stmt, target: &str) -> bool {
    match stmt {
        Stmt::Attributed { item, .. } => stmt_contains_call_to(item, target),
        Stmt::If {
            condition,
            then_stmt,
            else_stmt,
        } => {
            expr_contains_call_to(condition, target)
                || stmt_contains_call_to(then_stmt, target)
                || else_stmt
                    .as_ref()
                    .is_some_and(|stmt| stmt_contains_call_to(stmt, target))
        }
        Stmt::IfLet {
            value,
            then_stmt,
            else_stmt,
            ..
        } => {
            expr_contains_call_to(value, target)
                || stmt_contains_call_to(then_stmt, target)
                || else_stmt
                    .as_ref()
                    .is_some_and(|stmt| stmt_contains_call_to(stmt, target))
        }
        Stmt::While { condition, body } => {
            expr_contains_call_to(condition, target) || stmt_contains_call_to(body, target)
        }
        Stmt::WhileLet { value, body, .. } => {
            expr_contains_call_to(value, target) || stmt_contains_call_to(body, target)
        }
        Stmt::For { iterable, body, .. } => {
            expr_contains_call_to(iterable, target) || stmt_contains_call_to(body, target)
        }
        Stmt::Let { value, .. }
        | Stmt::Assign { value, .. }
        | Stmt::CompoundAssign { value, .. }
        | Stmt::Define { value, .. } => expr_contains_call_to(value, target),
        Stmt::Return { value } => value.as_ref().is_some_and(|value| expr_contains_call_to(value, target)),
        Stmt::Function { body, .. } => stmt_contains_call_to(body, target),
        Stmt::Block { statements } => statements.iter().any(|stmt| stmt_contains_call_to(stmt, target)),
        Stmt::Expr(expr) => expr_contains_call_to(expr, target),
        Stmt::Empty
        | Stmt::Import(_)
        | Stmt::Struct { .. }
        | Stmt::TypeAlias { .. }
        | Stmt::Trait { .. }
        | Stmt::Impl { .. }
        | Stmt::Break
        | Stmt::Continue => false,
    }
}

fn expr_contains_call_to(expr: &Expr, target: &str) -> bool {
    match expr {
        Expr::Paren(inner)
        | Expr::Unary(_, inner)
        | Expr::OptionalAccess(inner, _)
        | Expr::Match { value: inner, .. } => expr_contains_call_to(inner, target),
        Expr::Bin(lhs, _, rhs)
        | Expr::And(lhs, rhs)
        | Expr::Or(lhs, rhs)
        | Expr::NullishCoalescing(lhs, rhs)
        | Expr::Access(lhs, rhs) => expr_contains_call_to(lhs, target) || expr_contains_call_to(rhs, target),
        Expr::Conditional(condition, then_expr, else_expr) => {
            expr_contains_call_to(condition, target)
                || expr_contains_call_to(then_expr, target)
                || expr_contains_call_to(else_expr, target)
        }
        Expr::Call(name, args) => name == target || args.iter().any(|arg| expr_contains_call_to(arg, target)),
        Expr::CallExpr(callee, args) => {
            matches!(callee.as_ref(), Expr::Var(name) if name == target)
                || expr_contains_call_to(callee, target)
                || args.iter().any(|arg| expr_contains_call_to(arg, target))
        }
        Expr::CallNamed(callee, positional, named) => {
            matches!(callee.as_ref(), Expr::Var(name) if name == target)
                || expr_contains_call_to(callee, target)
                || positional.iter().any(|arg| expr_contains_call_to(arg, target))
                || named.iter().any(|(_, arg)| expr_contains_call_to(arg, target))
        }
        Expr::List(values) => values.iter().any(|value| expr_contains_call_to(value, target)),
        Expr::Map(entries) => entries
            .iter()
            .any(|(key, value)| expr_contains_call_to(key, target) || expr_contains_call_to(value, target)),
        Expr::StructLiteral { fields, .. } => fields.iter().any(|(_, value)| expr_contains_call_to(value, target)),
        Expr::TemplateString(parts) => parts.iter().any(|part| match part {
            crate::expr::TemplateStringPart::Literal(_) => false,
            crate::expr::TemplateStringPart::Expr(expr) => expr_contains_call_to(expr, target),
        }),
        Expr::Block(statements) => statements.iter().any(|stmt| stmt_contains_call_to(stmt, target)),
        Expr::Range { start, end, step, .. } => [start, end, step]
            .into_iter()
            .flatten()
            .any(|expr| expr_contains_call_to(expr, target)),
        Expr::Select { cases, default_case } => {
            cases.iter().any(|case| {
                select_pattern_contains_call_to(&case.pattern, target)
                    || case
                        .guard
                        .as_ref()
                        .is_some_and(|guard| expr_contains_call_to(guard, target))
                    || expr_contains_call_to(&case.body, target)
            }) || default_case
                .as_ref()
                .is_some_and(|expr| expr_contains_call_to(expr, target))
        }
        Expr::Literal(_) | Expr::Var(_) => false,
        Expr::Closure { body, .. } => expr_contains_call_to(body, target),
    }
}

fn select_pattern_contains_call_to(pattern: &SelectPattern, target: &str) -> bool {
    match pattern {
        SelectPattern::Recv { channel, .. } => expr_contains_call_to(channel, target),
        SelectPattern::Send { channel, value } => {
            expr_contains_call_to(channel, target) || expr_contains_call_to(value, target)
        }
    }
}

fn assigned_names_in_stmt(stmt: &Stmt) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_assigned_names(stmt, &mut names);
    names
}

fn local_names_in_inline_body(stmt: &Stmt, params: &[String]) -> HashSet<String> {
    let mut names = params.iter().cloned().collect::<HashSet<_>>();
    collect_local_names(stmt, &mut names);
    names
}

fn collect_local_names(stmt: &Stmt, names: &mut HashSet<String>) {
    match stmt {
        Stmt::Attributed { item, .. } => collect_local_names(item, names),
        Stmt::Let {
            pattern: Pattern::Variable(name),
            ..
        }
        | Stmt::Define { name, .. } => {
            names.insert(name.clone());
        }
        Stmt::If {
            then_stmt, else_stmt, ..
        } => {
            collect_local_names(then_stmt, names);
            if let Some(else_stmt) = else_stmt {
                collect_local_names(else_stmt, names);
            }
        }
        Stmt::While { body, .. } => collect_local_names(body, names),
        Stmt::Block { statements } => {
            for stmt in statements {
                collect_local_names(stmt, names);
            }
        }
        _ => {}
    }
}

fn collect_assigned_names(stmt: &Stmt, names: &mut HashSet<String>) {
    match stmt {
        Stmt::Attributed { item, .. } => collect_assigned_names(item, names),
        Stmt::Assign { name, value, .. } | Stmt::CompoundAssign { name, value, .. } => {
            names.insert(name.clone());
            collect_assigned_names_in_expr(value, names);
        }
        Stmt::Let {
            pattern: Pattern::Variable(name),
            value,
            ..
        }
        | Stmt::Define { name, value } => {
            names.insert(name.clone());
            collect_assigned_names_in_expr(value, names);
        }
        Stmt::Let { value, .. } => collect_assigned_names_in_expr(value, names),
        Stmt::If {
            condition,
            then_stmt,
            else_stmt,
        } => {
            collect_assigned_names_in_expr(condition, names);
            collect_assigned_names(then_stmt, names);
            if let Some(else_stmt) = else_stmt {
                collect_assigned_names(else_stmt, names);
            }
        }
        Stmt::While { condition, body } => {
            collect_assigned_names_in_expr(condition, names);
            collect_assigned_names(body, names);
        }
        Stmt::Block { statements } => {
            for stmt in statements {
                collect_assigned_names(stmt, names);
            }
        }
        Stmt::Expr(expr) => collect_assigned_names_in_expr(expr, names),
        Stmt::Return { value: Some(value) } => collect_assigned_names_in_expr(value, names),
        _ => {}
    }
}

fn collect_assigned_names_in_expr(expr: &Expr, names: &mut HashSet<String>) {
    match expr {
        Expr::Paren(inner)
        | Expr::Unary(_, inner)
        | Expr::OptionalAccess(inner, _)
        | Expr::Match { value: inner, .. } => collect_assigned_names_in_expr(inner, names),
        Expr::Bin(lhs, _, rhs)
        | Expr::And(lhs, rhs)
        | Expr::Or(lhs, rhs)
        | Expr::NullishCoalescing(lhs, rhs)
        | Expr::Access(lhs, rhs) => {
            collect_assigned_names_in_expr(lhs, names);
            collect_assigned_names_in_expr(rhs, names);
        }
        Expr::Conditional(condition, then_expr, else_expr) => {
            collect_assigned_names_in_expr(condition, names);
            collect_assigned_names_in_expr(then_expr, names);
            collect_assigned_names_in_expr(else_expr, names);
        }
        Expr::Call(_, args) => {
            for arg in args {
                collect_assigned_names_in_expr(arg, names);
            }
        }
        Expr::CallExpr(callee, args) => {
            collect_assigned_names_in_expr(callee, names);
            for arg in args {
                collect_assigned_names_in_expr(arg, names);
            }
        }
        Expr::CallNamed(callee, positional, named) => {
            collect_assigned_names_in_expr(callee, names);
            for arg in positional {
                collect_assigned_names_in_expr(arg, names);
            }
            for (_, arg) in named {
                collect_assigned_names_in_expr(arg, names);
            }
        }
        Expr::List(values) => {
            for value in values {
                collect_assigned_names_in_expr(value, names);
            }
        }
        Expr::Map(entries) => {
            for (key, value) in entries {
                collect_assigned_names_in_expr(key, names);
                collect_assigned_names_in_expr(value, names);
            }
        }
        Expr::StructLiteral { fields, .. } => {
            for (_, value) in fields {
                collect_assigned_names_in_expr(value, names);
            }
        }
        Expr::TemplateString(parts) => {
            for part in parts {
                if let crate::expr::TemplateStringPart::Expr(expr) = part {
                    collect_assigned_names_in_expr(expr, names);
                }
            }
        }
        Expr::Block(statements) => {
            for stmt in statements {
                collect_assigned_names(stmt, names);
            }
        }
        Expr::Range { start, end, step, .. } => {
            for expr in [start, end, step].into_iter().flatten() {
                collect_assigned_names_in_expr(expr, names);
            }
        }
        Expr::Select { cases, default_case } => {
            for case in cases {
                collect_assigned_names_in_select_pattern(&case.pattern, names);
                if let Some(guard) = &case.guard {
                    collect_assigned_names_in_expr(guard, names);
                }
                collect_assigned_names_in_expr(&case.body, names);
            }
            if let Some(default_case) = default_case {
                collect_assigned_names_in_expr(default_case, names);
            }
        }
        Expr::Closure { body, .. } => collect_assigned_names_in_expr(body, names),
        Expr::Literal(_) | Expr::Var(_) => {}
    }
}

fn collect_assigned_names_in_select_pattern(pattern: &SelectPattern, names: &mut HashSet<String>) {
    match pattern {
        SelectPattern::Recv { channel, .. } => collect_assigned_names_in_expr(channel, names),
        SelectPattern::Send { channel, value } => {
            collect_assigned_names_in_expr(channel, names);
            collect_assigned_names_in_expr(value, names);
        }
    }
}
