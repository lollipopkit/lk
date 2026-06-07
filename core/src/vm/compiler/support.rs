use crate::util::fast_map::fast_hash_map_new;
use std::collections::{HashMap, HashSet};

use anyhow::{Result, anyhow, bail};

use crate::{
    expr::{Expr, Pattern, SelectPattern},
    operator::BinOp,
    stmt::{NamedParamDecl, Program, Stmt},
    val::{LiteralVal, RuntimeMapKey, ShortStr},
    vm::ConstRuntimeValue,
};

use std::sync::Arc;

use super::{ConstHeapValue, GlobalSlot, NativeEntry, free_vars::collect_function_free_vars};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ShortCircuitKind {
    And,
    Or,
    Nullish,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NumericFlavor {
    Int,
    Float,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RangeStepSign {
    Positive,
    Negative,
    Dynamic,
}

#[derive(Debug, Default)]
pub(super) struct LoopPatch {
    pub(super) breaks: Vec<usize>,
    pub(super) continues: Vec<usize>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct FunctionSignature {
    pub(super) positional_params: Vec<String>,
    pub(super) positional_count: usize,
    pub(super) named_params: Vec<NamedParamDecl>,
}

#[derive(Clone, Debug)]
pub(super) struct FunctionInlineBody {
    pub(super) params: Vec<String>,
    pub(super) named_param_count: usize,
    pub(super) body: Stmt,
}

pub(super) fn collect_function_names(program: &Program) -> Result<HashMap<String, u32>> {
    let mut names = HashMap::new();
    let mut next = 1_u32;
    for stmt in &program.statements {
        if let Stmt::Function { name, .. } = stmt.as_ref() {
            if names.insert(name.clone(), next).is_some() {
                bail!("Compiler duplicate function `{name}`");
            }
            next = next
                .checked_add(1)
                .ok_or_else(|| anyhow!("Compiler function index overflow"))?;
        }
    }
    Ok(names)
}

pub(super) fn collect_function_inline_bodies(program: &Program) -> Result<HashMap<String, FunctionInlineBody>> {
    let mut bodies = HashMap::new();
    for stmt in &program.statements {
        if let Stmt::Function {
            name,
            params,
            named_params,
            body,
            ..
        } = stmt.as_ref()
        {
            let body = FunctionInlineBody {
                params: params.clone(),
                named_param_count: named_params.len(),
                body: body.as_ref().clone(),
            };
            if bodies.insert(name.clone(), body).is_some() {
                bail!("Compiler duplicate function `{name}`");
            }
        }
    }
    Ok(bodies)
}

pub(super) fn collect_function_signatures(program: &Program) -> Result<HashMap<String, FunctionSignature>> {
    let mut signatures = HashMap::new();
    for stmt in &program.statements {
        if let Stmt::Function {
            name,
            params,
            named_params,
            ..
        } = stmt.as_ref()
        {
            let signature = FunctionSignature {
                positional_params: params.clone(),
                positional_count: params.len(),
                named_params: named_params.clone(),
            };
            if signatures.insert(name.clone(), signature).is_some() {
                bail!("Compiler duplicate function `{name}`");
            }
        }
    }
    Ok(signatures)
}

pub(super) fn function_frame_params(params: &[String], named_params: &[NamedParamDecl]) -> Vec<String> {
    let mut frame_params = Vec::with_capacity(params.len() + named_params.len());
    frame_params.extend(params.iter().cloned());
    frame_params.extend(named_params.iter().map(|param| param.name.clone()));
    frame_params
}

pub(super) fn range_step_sign(step: Option<&Expr>) -> RangeStepSign {
    let Some(step) = step else {
        return RangeStepSign::Positive;
    };
    match const_int_expr_value(step) {
        Some(value) if value > 0 => RangeStepSign::Positive,
        Some(value) if value < 0 => RangeStepSign::Negative,
        _ => RangeStepSign::Dynamic,
    }
}

pub(super) fn simple_local_expr_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Paren(inner) => simple_local_expr_name(inner),
        Expr::Var(name) => Some(name.as_str()),
        _ => None,
    }
}

pub(super) fn mutated_names_in_stmt(stmt: &Stmt) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_mutated_names(stmt, &mut names);
    names
}

pub(super) fn pattern_binds_scrutinee_directly(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Variable(_) => true,
        Pattern::Guard { pattern, .. } => pattern_binds_scrutinee_directly(pattern),
        Pattern::Or(patterns) => patterns.iter().any(pattern_binds_scrutinee_directly),
        Pattern::Wildcard
        | Pattern::List { .. }
        | Pattern::Map { .. }
        | Pattern::Literal(_)
        | Pattern::Range { .. } => false,
    }
}

fn collect_mutated_names(stmt: &Stmt, names: &mut HashSet<String>) {
    match stmt {
        Stmt::If {
            condition,
            then_stmt,
            else_stmt,
        } => {
            collect_mutated_names_in_expr(condition, names);
            collect_mutated_names(then_stmt, names);
            if let Some(else_stmt) = else_stmt {
                collect_mutated_names(else_stmt, names);
            }
        }
        Stmt::IfLet {
            value,
            then_stmt,
            else_stmt,
            ..
        } => {
            collect_mutated_names_in_expr(value, names);
            collect_mutated_names(then_stmt, names);
            if let Some(else_stmt) = else_stmt {
                collect_mutated_names(else_stmt, names);
            }
        }
        Stmt::While { condition, body } => {
            collect_mutated_names_in_expr(condition, names);
            collect_mutated_names(body, names);
        }
        Stmt::WhileLet { value, body, .. } => {
            collect_mutated_names_in_expr(value, names);
            collect_mutated_names(body, names);
        }
        Stmt::For { iterable, body, .. } => {
            collect_mutated_names_in_expr(iterable, names);
            collect_mutated_names(body, names);
        }
        Stmt::Let { value, .. } | Stmt::Define { value, .. } => collect_mutated_names_in_expr(value, names),
        Stmt::Assign { name, value, .. } | Stmt::CompoundAssign { name, value, .. } => {
            names.insert(name.clone());
            collect_mutated_names_in_expr(value, names);
        }
        Stmt::Function { named_params, body, .. } => {
            for param in named_params {
                if let Some(default) = &param.default {
                    collect_mutated_names_in_expr(default, names);
                }
            }
            collect_mutated_names(body, names);
        }
        Stmt::Impl { methods, .. } => {
            for method in methods {
                collect_mutated_names(method, names);
            }
        }
        Stmt::Expr(expr) => collect_mutated_names_in_expr(expr, names),
        Stmt::Return { value } => {
            if let Some(value) = value {
                collect_mutated_names_in_expr(value, names);
            }
        }
        Stmt::Block { statements } => {
            for stmt in statements {
                collect_mutated_names(stmt, names);
            }
        }
        Stmt::Import(_)
        | Stmt::Break
        | Stmt::Continue
        | Stmt::Struct { .. }
        | Stmt::TypeAlias { .. }
        | Stmt::Trait { .. }
        | Stmt::Empty => {}
    }
}

fn collect_mutated_names_in_expr(expr: &Expr, names: &mut HashSet<String>) {
    match expr {
        Expr::Paren(inner) | Expr::Unary(_, inner) => collect_mutated_names_in_expr(inner, names),
        Expr::Bin(lhs, _, rhs)
        | Expr::And(lhs, rhs)
        | Expr::Or(lhs, rhs)
        | Expr::NullishCoalescing(lhs, rhs)
        | Expr::Access(lhs, rhs)
        | Expr::OptionalAccess(lhs, rhs) => {
            collect_mutated_names_in_expr(lhs, names);
            collect_mutated_names_in_expr(rhs, names);
        }
        Expr::Conditional(condition, then_expr, else_expr) => {
            collect_mutated_names_in_expr(condition, names);
            collect_mutated_names_in_expr(then_expr, names);
            collect_mutated_names_in_expr(else_expr, names);
        }
        Expr::Call(_, args) => {
            for arg in args {
                collect_mutated_names_in_expr(arg, names);
            }
        }
        Expr::CallExpr(callee, args) => {
            collect_mutated_names_in_expr(callee, names);
            for arg in args {
                collect_mutated_names_in_expr(arg, names);
            }
        }
        Expr::CallNamed(callee, positional, named) => {
            collect_mutated_names_in_expr(callee, names);
            for arg in positional {
                collect_mutated_names_in_expr(arg, names);
            }
            for (_, arg) in named {
                collect_mutated_names_in_expr(arg, names);
            }
        }
        Expr::List(values) => {
            for value in values {
                collect_mutated_names_in_expr(value, names);
            }
        }
        Expr::Map(entries) => {
            for (key, value) in entries {
                collect_mutated_names_in_expr(key, names);
                collect_mutated_names_in_expr(value, names);
            }
        }
        Expr::StructLiteral { fields, .. } => {
            for (_, value) in fields {
                collect_mutated_names_in_expr(value, names);
            }
        }
        Expr::TemplateString(parts) => {
            for part in parts {
                if let crate::expr::TemplateStringPart::Expr(expr) = part {
                    collect_mutated_names_in_expr(expr, names);
                }
            }
        }
        Expr::Block(statements) => {
            for stmt in statements {
                collect_mutated_names(stmt, names);
            }
        }
        Expr::Range { start, end, step, .. } => {
            for expr in [start, end, step].into_iter().flatten() {
                collect_mutated_names_in_expr(expr, names);
            }
        }
        Expr::Select { cases, default_case } => {
            for case in cases {
                collect_mutated_names_in_select_pattern(&case.pattern, names);
                if let Some(guard) = &case.guard {
                    collect_mutated_names_in_expr(guard, names);
                }
                collect_mutated_names_in_expr(&case.body, names);
            }
            if let Some(default_case) = default_case {
                collect_mutated_names_in_expr(default_case, names);
            }
        }
        Expr::Match { value, arms } => {
            collect_mutated_names_in_expr(value, names);
            for arm in arms {
                collect_mutated_names_in_pattern(&arm.pattern, names);
                collect_mutated_names_in_expr(&arm.body, names);
            }
        }
        Expr::Closure { body, .. } => collect_mutated_names_in_expr(body, names),
        Expr::Literal(_) | Expr::Var(_) => {}
    }
}

fn collect_mutated_names_in_pattern(pattern: &Pattern, names: &mut HashSet<String>) {
    match pattern {
        Pattern::List { patterns, .. } | Pattern::Or(patterns) => {
            for pattern in patterns {
                collect_mutated_names_in_pattern(pattern, names);
            }
        }
        Pattern::Map { patterns, .. } => {
            for (_, pattern) in patterns {
                collect_mutated_names_in_pattern(pattern, names);
            }
        }
        Pattern::Guard { pattern, guard } => {
            collect_mutated_names_in_pattern(pattern, names);
            collect_mutated_names_in_expr(guard, names);
        }
        Pattern::Range { start, end, .. } => {
            collect_mutated_names_in_expr(start, names);
            collect_mutated_names_in_expr(end, names);
        }
        Pattern::Literal(_) | Pattern::Variable(_) | Pattern::Wildcard => {}
    }
}

fn collect_mutated_names_in_select_pattern(pattern: &SelectPattern, names: &mut HashSet<String>) {
    match pattern {
        SelectPattern::Recv { channel, .. } => collect_mutated_names_in_expr(channel, names),
        SelectPattern::Send { channel, value } => {
            collect_mutated_names_in_expr(channel, names);
            collect_mutated_names_in_expr(value, names);
        }
    }
}

fn const_int_expr_value(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Paren(inner) => const_int_expr_value(inner),
        Expr::Literal(LiteralVal::Int(value)) => Some(*value),
        Expr::Bin(lhs, BinOp::Add, rhs) => const_int_expr_value(lhs)?.checked_add(const_int_expr_value(rhs)?),
        Expr::Bin(lhs, BinOp::Sub, rhs) => const_int_expr_value(lhs)?.checked_sub(const_int_expr_value(rhs)?),
        Expr::Bin(lhs, BinOp::Mul, rhs) => const_int_expr_value(lhs)?.checked_mul(const_int_expr_value(rhs)?),
        _ => None,
    }
}

pub(super) fn collect_global_names_with_external<I, S>(
    program: &Program,
    external_globals: I,
) -> Result<HashMap<String, u32>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut names = HashMap::new();
    for name in external_globals {
        insert_global_name(&mut names, name.as_ref().to_owned())?;
    }

    let top_level_lets = collect_top_level_let_names(program);
    let function_visible_lets = collect_callable_visible_top_level_lets(program, &top_level_lets);

    for stmt in &program.statements {
        match stmt.as_ref() {
            Stmt::Define { name, .. } | Stmt::Function { name, .. } => {
                insert_global_name(&mut names, name.clone())?;
            }
            Stmt::Let { pattern, .. } => {
                if let Pattern::Variable(name) = pattern
                    && function_visible_lets.contains(name)
                {
                    insert_global_name(&mut names, name.clone())?;
                }
            }
            _ => {}
        }
    }
    Ok(names)
}

fn collect_top_level_let_names(program: &Program) -> HashSet<String> {
    let mut names = HashSet::new();
    for stmt in &program.statements {
        if let Stmt::Let {
            pattern: Pattern::Variable(name),
            ..
        } = stmt.as_ref()
        {
            names.insert(name.clone());
        }
    }
    names
}

fn collect_callable_visible_top_level_lets(program: &Program, top_level_lets: &HashSet<String>) -> HashSet<String> {
    let mut visible = HashSet::new();
    for stmt in &program.statements {
        match stmt.as_ref() {
            Stmt::Function {
                params,
                named_params,
                body,
                ..
            } => collect_visible_function_lets(&mut visible, top_level_lets, params, named_params, body),
            Stmt::Impl { methods, .. } => {
                for method in methods {
                    if let Stmt::Function {
                        params,
                        named_params,
                        body,
                        ..
                    } = method
                    {
                        collect_visible_function_lets(&mut visible, top_level_lets, params, named_params, body);
                    }
                }
            }
            _ => {}
        }
    }
    visible
}

fn collect_visible_function_lets(
    visible: &mut HashSet<String>,
    top_level_lets: &HashSet<String>,
    params: &[String],
    named_params: &[NamedParamDecl],
    body: &Stmt,
) {
    for name in collect_function_free_vars(params, named_params, body) {
        if top_level_lets.contains(&name) {
            visible.insert(name);
        }
    }
}

fn insert_global_name(names: &mut HashMap<String, u32>, name: String) -> Result<()> {
    if names.contains_key(&name) {
        return Ok(());
    }
    let slot = u32::try_from(names.len()).map_err(|_| anyhow!("Compiler global slot overflow"))?;
    names.insert(name, slot);
    Ok(())
}

pub(super) fn global_slots_from_names(names: &HashMap<String, u32>) -> Vec<GlobalSlot> {
    let mut slots = vec![None; names.len()];
    for (name, slot) in names {
        slots[*slot as usize] = Some(GlobalSlot {
            name: Arc::<str>::from(name.as_str()),
        });
    }
    let mut out = Vec::with_capacity(slots.len());
    for slot in slots {
        out.push(slot.expect("dense global slot"));
    }
    out
}

pub(super) fn collect_native_names(natives: &[NativeEntry]) -> Result<HashMap<String, u32>> {
    let mut names = HashMap::new();
    for (index, native) in natives.iter().enumerate() {
        let index = u32::try_from(index).map_err(|_| anyhow!("Compiler native index overflow"))?;
        if names.insert(native.name.clone(), index).is_some() {
            bail!("Compiler duplicate native `{}`", native.name);
        }
    }
    Ok(names)
}

pub(super) fn checked_u8(name: &str, value: u16) -> Result<u8> {
    u8::try_from(value).map_err(|_| anyhow!("Compiler {name} register {} exceeds u8 encoding", value))
}

pub(super) fn numeric_flavor(lhs: &Expr, op: &BinOp, rhs: &Expr) -> NumericFlavor {
    if op.is_arith() && (expr_is_statically_float(lhs) || expr_is_statically_float(rhs)) {
        NumericFlavor::Float
    } else {
        NumericFlavor::Int
    }
}

pub(super) fn expr_is_statically_float(expr: &Expr) -> bool {
    match expr {
        Expr::Paren(inner) => expr_is_statically_float(inner),
        Expr::Literal(LiteralVal::Float(_)) => true,
        Expr::Bin(lhs, op, rhs) if op.is_arith() => expr_is_statically_float(lhs) || expr_is_statically_float(rhs),
        Expr::Conditional(_, then_expr, else_expr) => {
            expr_is_statically_float(then_expr) && expr_is_statically_float(else_expr)
        }
        _ => false,
    }
}

pub(super) fn jump_offset(pc: usize, target: usize) -> Result<i32> {
    let offset = target as i64 - pc as i64 - 1;
    i32::try_from(offset).map_err(|_| anyhow!("Compiler jump offset {offset} exceeds i32"))
}

fn const_heap_value_from_literal(value: &LiteralVal) -> Result<ConstHeapValue> {
    if let Some(text) = value.as_str() {
        if ShortStr::new(text).is_some() {
            bail!("Compiler short string does not require heap const");
        }
        return Ok(ConstHeapValue::LongString(text.into()));
    }

    bail!("Compiler cannot convert {} to heap const", ast_literal_kind(value))
}

pub(super) fn const_heap_value_from_expr_literal(expr: &Expr) -> Result<Option<ConstHeapValue>> {
    match expr {
        Expr::Literal(value) => const_heap_value_from_literal(value).map(Some),
        Expr::List(values) => const_heap_list_from_expr_literals(values),
        Expr::Map(entries) => const_heap_map_from_expr_literals(entries),
        _ => Ok(None),
    }
}

pub(super) fn const_heap_list_from_expr_literals(values: &[Box<Expr>]) -> Result<Option<ConstHeapValue>> {
    let mut const_values = Vec::with_capacity(values.len());
    for value in values {
        let Some(value) = const_runtime_value_from_expr_literal(value)? else {
            return Ok(None);
        };
        const_values.push(value);
    }
    Ok(Some(ConstHeapValue::List(const_values)))
}

pub(super) fn const_heap_map_from_expr_literals(entries: &[(Box<Expr>, Box<Expr>)]) -> Result<Option<ConstHeapValue>> {
    let mut const_entries = fast_hash_map_new();
    for (key, value) in entries {
        let Expr::Literal(key) = &**key else {
            return Ok(None);
        };
        let Some(key) = const_runtime_map_key_from_literal(key)? else {
            return Ok(None);
        };
        let Some(value) = const_runtime_value_from_expr_literal(value)? else {
            return Ok(None);
        };
        const_entries.insert(key, value);
    }
    Ok(Some(ConstHeapValue::Map(const_entries)))
}

fn const_runtime_value_from_literal(value: &LiteralVal) -> Result<ConstRuntimeValue> {
    Ok(match value {
        LiteralVal::Nil => ConstRuntimeValue::Nil,
        LiteralVal::Bool(value) => ConstRuntimeValue::Bool(*value),
        LiteralVal::Int(value) => ConstRuntimeValue::Int(*value),
        LiteralVal::Float(value) => ConstRuntimeValue::Float(*value),
        value if value.as_str().is_some() => {
            let value = value.as_str().expect("checked string");
            if let Some(short) = ShortStr::new(value) {
                ConstRuntimeValue::ShortStr(short)
            } else {
                ConstRuntimeValue::Heap(Box::new(ConstHeapValue::LongString(value.into())))
            }
        }
        other => bail!(
            "Compiler cannot convert AST literal value to ConstRuntimeValue: {}",
            ast_literal_kind(other)
        ),
    })
}

fn const_runtime_value_from_expr_literal(expr: &Expr) -> Result<Option<ConstRuntimeValue>> {
    Ok(Some(match expr {
        Expr::Literal(value) => const_runtime_value_from_literal(value)?,
        Expr::List(..) | Expr::Map(..) => {
            let Some(value) = const_heap_value_from_expr_literal(expr)? else {
                return Ok(None);
            };
            ConstRuntimeValue::Heap(Box::new(value))
        }
        _ => return Ok(None),
    }))
}

pub(super) fn const_runtime_map_key_from_literal(value: &LiteralVal) -> Result<Option<RuntimeMapKey>> {
    Ok(Some(match value {
        LiteralVal::Nil => RuntimeMapKey::Nil,
        LiteralVal::Bool(value) => RuntimeMapKey::Bool(*value),
        LiteralVal::Int(value) => RuntimeMapKey::Int(*value),
        value if value.as_str().is_some() => {
            let value = value.as_str().expect("checked string");
            if let Some(short) = ShortStr::new(value) {
                RuntimeMapKey::ShortStr(short)
            } else {
                RuntimeMapKey::String(value.into())
            }
        }
        LiteralVal::Float(_) => return Ok(None),
        other => bail!("Compiler cannot convert {} to const map key", ast_literal_kind(other)),
    }))
}

pub(super) fn ast_literal_kind(value: &LiteralVal) -> &'static str {
    match value {
        LiteralVal::Nil => "Nil",
        LiteralVal::Bool(_) => "Bool",
        LiteralVal::Int(_) => "Int",
        LiteralVal::Float(_) => "Float",
        LiteralVal::ShortStr(_) | LiteralVal::String(_) => "String",
    }
}

pub(super) fn expr_kind(expr: &Expr) -> &'static str {
    match expr {
        Expr::Bin(..) => "Bin",
        Expr::Unary(..) => "Unary",
        Expr::Conditional(..) => "Conditional",
        Expr::And(..) => "And",
        Expr::Or(..) => "Or",
        Expr::NullishCoalescing(..) => "NullishCoalescing",
        Expr::Access(..) => "Access",
        Expr::OptionalAccess(..) => "OptionalAccess",
        Expr::Paren(..) => "Paren",
        Expr::List(..) => "List",
        Expr::Map(..) => "Map",
        Expr::StructLiteral { .. } => "StructLiteral",
        Expr::Var(..) => "Var",
        Expr::Call(..) => "Call",
        Expr::CallExpr(..) => "CallExpr",
        Expr::CallNamed(..) => "CallNamed",
        Expr::Range { .. } => "Range",
        Expr::Select { .. } => "Select",
        Expr::TemplateString(..) => "TemplateString",
        Expr::Closure { .. } => "Closure",
        Expr::Block(..) => "Block",
        Expr::Match { .. } => "Match",
        Expr::Literal(..) => "LiteralVal",
    }
}

pub(super) fn pattern_kind(pattern: &Pattern) -> &'static str {
    match pattern {
        Pattern::Literal(_) => "Literal",
        Pattern::Variable(_) => "Variable",
        Pattern::Wildcard => "Wildcard",
        Pattern::List { .. } => "List",
        Pattern::Map { .. } => "Map",
        Pattern::Or(_) => "Or",
        Pattern::Guard { .. } => "Guard",
        Pattern::Range { .. } => "Range",
    }
}
