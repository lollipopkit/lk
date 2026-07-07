#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::{
    expr::Expr,
    operator::BinOp,
    vm::analysis::{PerfContainerFact, PerfIndexFact, PerfIndexTargetKind, PerfValueKind, PerformanceFacts},
};

use super::support::{NumericFlavor, numeric_flavor};

pub(super) fn short_string_literal_key(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Paren(inner) => short_string_literal_key(inner),
        Expr::Literal(value) => value.as_str().filter(|text| crate::val::ShortStr::new(text).is_some()),
        _ => None,
    }
}

pub(super) fn literal_dead_write_is_safe(value: &crate::val::LiteralVal) -> bool {
    match value {
        crate::val::LiteralVal::Nil
        | crate::val::LiteralVal::Bool(_)
        | crate::val::LiteralVal::Int(_)
        | crate::val::LiteralVal::Float(_) => true,
        value => value
            .as_str()
            .is_some_and(|text| crate::val::ShortStr::new(text).is_some()),
    }
}

pub(super) fn list_fact_from_exprs(elements: &[Box<Expr>]) -> PerfContainerFact {
    PerfContainerFact {
        value_kind: join_expr_value_kinds(elements.iter().map(|expr| expr_static_value_kind(expr))),
        known_len: Some(elements.len()),
        adoptable: elements.is_empty(),
    }
}

pub(super) fn map_fact_from_exprs(entries: &[(Box<Expr>, Box<Expr>)]) -> PerfContainerFact {
    PerfContainerFact {
        value_kind: join_expr_value_kinds(entries.iter().map(|(_, value)| expr_static_value_kind(value))),
        known_len: Some(entries.len()),
        adoptable: entries.is_empty(),
    }
}

pub(super) fn numeric_flavor_from_register_facts(
    facts: &PerformanceFacts,
    op: &BinOp,
    lhs: u16,
    rhs: u16,
) -> Option<NumericFlavor> {
    if !op.is_arith() {
        return None;
    }
    let lhs = facts.value_kind(lhs);
    let rhs = facts.value_kind(rhs);
    match (lhs, rhs) {
        (PerfValueKind::Float, PerfValueKind::Float)
        | (PerfValueKind::Float, PerfValueKind::Int)
        | (PerfValueKind::Int, PerfValueKind::Float) => Some(NumericFlavor::Float),
        (PerfValueKind::Int, PerfValueKind::Int) => Some(NumericFlavor::Int),
        _ => None,
    }
}

pub(super) fn index_fact_from_target(facts: &PerformanceFacts, target: u16) -> Option<PerfIndexFact> {
    let target_kind = match facts.value_kind(target) {
        PerfValueKind::List => PerfIndexTargetKind::List,
        PerfValueKind::Map => PerfIndexTargetKind::Map,
        PerfValueKind::Object => PerfIndexTargetKind::Object,
        PerfValueKind::String => PerfIndexTargetKind::String,
        _ => return None,
    };
    let value_kind = match target_kind {
        PerfIndexTargetKind::List => facts.list_value_kind(target).unwrap_or_default(),
        PerfIndexTargetKind::Map => facts.map_value_kind(target).unwrap_or_default(),
        PerfIndexTargetKind::Object | PerfIndexTargetKind::String | PerfIndexTargetKind::Unknown => {
            PerfValueKind::Unknown
        }
    };
    Some(PerfIndexFact {
        target_kind,
        value_kind,
    })
}

pub(super) fn bin_op_result_kind(op: &BinOp, flavor: NumericFlavor) -> PerfValueKind {
    if op.is_cmp() || matches!(op, BinOp::In) {
        return PerfValueKind::Bool;
    }
    match flavor {
        NumericFlavor::Int if op.is_arith() => PerfValueKind::Int,
        NumericFlavor::Float if op.is_arith() => PerfValueKind::Float,
        _ => PerfValueKind::Unknown,
    }
}

fn join_expr_value_kinds(kinds: impl IntoIterator<Item = PerfValueKind>) -> PerfValueKind {
    let mut iter = kinds.into_iter();
    let Some(first) = iter.next() else {
        return PerfValueKind::Unknown;
    };
    iter.fold(first, PerfValueKind::join)
}

pub(super) fn expr_static_value_kind(expr: &Expr) -> PerfValueKind {
    match expr {
        Expr::Paren(inner) => expr_static_value_kind(inner),
        Expr::Literal(value) => PerfValueKind::from_literal(value),
        Expr::TemplateString(_) => PerfValueKind::String,
        Expr::List(_) => PerfValueKind::List,
        Expr::Map(_) => PerfValueKind::Map,
        Expr::StructLiteral { .. } => PerfValueKind::Object,
        Expr::Conditional(_, then_expr, else_expr) => {
            expr_static_value_kind(then_expr).join(expr_static_value_kind(else_expr))
        }
        Expr::Bin(lhs, op, rhs) => bin_op_static_value_kind(lhs, op, rhs),
        _ => PerfValueKind::Unknown,
    }
}

fn bin_op_static_value_kind(lhs: &Expr, op: &BinOp, rhs: &Expr) -> PerfValueKind {
    if op.is_cmp() || matches!(op, BinOp::In) {
        return PerfValueKind::Bool;
    }
    match numeric_flavor(lhs, op, rhs) {
        NumericFlavor::Int if op.is_arith() => PerfValueKind::Int,
        NumericFlavor::Float if op.is_arith() => PerfValueKind::Float,
        _ => PerfValueKind::Unknown,
    }
}
