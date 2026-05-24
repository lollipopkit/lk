use std::collections::{BTreeMap, HashMap};

use anyhow::{Result, anyhow, bail};

use crate::{
    expr::{Expr, Pattern},
    operator::BinOp,
    stmt::{NamedParamDecl, Program, Stmt},
    val::{LiteralVal, RuntimeMapKey, ShortStr},
    vm::ConstRuntimeValue32,
};

use std::sync::Arc;

use super::{ConstHeapValue32, GlobalSlot32, NativeEntry32};

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

#[derive(Debug, Default)]
pub(super) struct LoopPatch32 {
    pub(super) breaks: Vec<usize>,
    pub(super) continues: Vec<usize>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct FunctionSignature32 {
    pub(super) positional_params: Vec<String>,
    pub(super) positional_count: usize,
    pub(super) named_params: Vec<NamedParamDecl>,
}

pub(super) fn collect_function_names(program: &Program) -> Result<HashMap<String, u32>> {
    let mut names = HashMap::new();
    let mut next = 1_u32;
    for stmt in &program.statements {
        if let Stmt::Function { name, .. } = stmt.as_ref() {
            if names.insert(name.clone(), next).is_some() {
                bail!("Compiler32 duplicate function `{name}`");
            }
            next = next
                .checked_add(1)
                .ok_or_else(|| anyhow!("Compiler32 function index overflow"))?;
        }
    }
    Ok(names)
}

pub(super) fn collect_function_signatures(program: &Program) -> Result<HashMap<String, FunctionSignature32>> {
    let mut signatures = HashMap::new();
    for stmt in &program.statements {
        if let Stmt::Function {
            name,
            params,
            named_params,
            ..
        } = stmt.as_ref()
        {
            let signature = FunctionSignature32 {
                positional_params: params.clone(),
                positional_count: params.len(),
                named_params: named_params.clone(),
            };
            if signatures.insert(name.clone(), signature).is_some() {
                bail!("Compiler32 duplicate function `{name}`");
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
    for stmt in &program.statements {
        match stmt.as_ref() {
            Stmt::Define { name, .. } | Stmt::Function { name, .. } => {
                insert_global_name(&mut names, name.clone())?;
            }
            Stmt::Let { pattern, .. } => {
                if let Pattern::Variable(name) = pattern {
                    insert_global_name(&mut names, name.clone())?;
                }
            }
            _ => {}
        }
    }
    Ok(names)
}

fn insert_global_name(names: &mut HashMap<String, u32>, name: String) -> Result<()> {
    if names.contains_key(&name) {
        return Ok(());
    }
    let slot = u32::try_from(names.len()).map_err(|_| anyhow!("Compiler32 global slot overflow"))?;
    names.insert(name, slot);
    Ok(())
}

pub(super) fn global_slots_from_names(names: &HashMap<String, u32>) -> Vec<GlobalSlot32> {
    let mut slots = vec![None; names.len()];
    for (name, slot) in names {
        slots[*slot as usize] = Some(GlobalSlot32 {
            name: Arc::<str>::from(name.as_str()),
        });
    }
    let mut out = Vec::with_capacity(slots.len());
    for slot in slots {
        out.push(slot.expect("dense global slot"));
    }
    out
}

pub(super) fn collect_native_names(natives: &[NativeEntry32]) -> Result<HashMap<String, u32>> {
    let mut names = HashMap::new();
    for (index, native) in natives.iter().enumerate() {
        let index = u32::try_from(index).map_err(|_| anyhow!("Compiler32 native index overflow"))?;
        if names.insert(native.name.clone(), index).is_some() {
            bail!("Compiler32 duplicate native `{}`", native.name);
        }
    }
    Ok(names)
}

pub(super) fn checked_u8(name: &str, value: u16) -> Result<u8> {
    u8::try_from(value).map_err(|_| anyhow!("Compiler32 {name} register {} exceeds u8 encoding", value))
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
    i32::try_from(offset).map_err(|_| anyhow!("Compiler32 jump offset {offset} exceeds i32"))
}

fn const_heap_value_from_literal(value: &LiteralVal) -> Result<ConstHeapValue32> {
    if let Some(text) = value.as_str() {
        if ShortStr::new(text).is_some() {
            bail!("Compiler32 short string does not require heap const");
        }
        return Ok(ConstHeapValue32::LongString(text.into()));
    }

    bail!("Compiler32 cannot convert {} to heap const", ast_literal_kind(value))
}

pub(super) fn const_heap_value_from_expr_literal(expr: &Expr) -> Result<Option<ConstHeapValue32>> {
    match expr {
        Expr::Literal(value) => const_heap_value_from_literal(value).map(Some),
        Expr::List(values) => const_heap_list_from_expr_literals(values),
        Expr::Map(entries) => const_heap_map_from_expr_literals(entries),
        _ => Ok(None),
    }
}

pub(super) fn const_heap_list_from_expr_literals(values: &[Box<Expr>]) -> Result<Option<ConstHeapValue32>> {
    let mut const_values = Vec::with_capacity(values.len());
    for value in values {
        let Some(value) = const_runtime_value_from_expr_literal(value)? else {
            return Ok(None);
        };
        const_values.push(value);
    }
    Ok(Some(ConstHeapValue32::List(const_values)))
}

pub(super) fn const_heap_map_from_expr_literals(
    entries: &[(Box<Expr>, Box<Expr>)],
) -> Result<Option<ConstHeapValue32>> {
    let mut const_entries = BTreeMap::new();
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
    Ok(Some(ConstHeapValue32::Map(const_entries)))
}

fn const_runtime_value_from_literal(value: &LiteralVal) -> Result<ConstRuntimeValue32> {
    Ok(match value {
        LiteralVal::Nil => ConstRuntimeValue32::Nil,
        LiteralVal::Bool(value) => ConstRuntimeValue32::Bool(*value),
        LiteralVal::Int(value) => ConstRuntimeValue32::Int(*value),
        LiteralVal::Float(value) => ConstRuntimeValue32::Float(*value),
        value if value.as_str().is_some() => {
            let value = value.as_str().expect("checked string");
            if let Some(short) = ShortStr::new(value) {
                ConstRuntimeValue32::ShortStr(short)
            } else {
                ConstRuntimeValue32::Heap(Box::new(ConstHeapValue32::LongString(value.into())))
            }
        }
        other => bail!(
            "Compiler32 cannot convert AST literal value to ConstRuntimeValue32: {}",
            ast_literal_kind(other)
        ),
    })
}

fn const_runtime_value_from_expr_literal(expr: &Expr) -> Result<Option<ConstRuntimeValue32>> {
    Ok(Some(match expr {
        Expr::Literal(value) => const_runtime_value_from_literal(value)?,
        Expr::List(..) | Expr::Map(..) => {
            let Some(value) = const_heap_value_from_expr_literal(expr)? else {
                return Ok(None);
            };
            ConstRuntimeValue32::Heap(Box::new(value))
        }
        _ => return Ok(None),
    }))
}

fn const_runtime_map_key_from_literal(value: &LiteralVal) -> Result<Option<RuntimeMapKey>> {
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
        other => bail!("Compiler32 cannot convert {} to const map key", ast_literal_kind(other)),
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
