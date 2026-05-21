use std::sync::Arc;

use crate::op::BinOp;
use crate::vm::analysis::{FunctionAnalysis, PerfContainerFact, PerfValueFact, PerfValueKind, PerformanceFacts};
use crate::vm::{AllocationRegion, RegionPlan};

use super::{SsaFunction, SsaRvalue, escape, lower_expr_to_ssa};
use crate::expr::Expr;

/// Run the SSA pipeline (lowering + escape analysis) for an expression.
pub fn analyze_expr(expr: &Expr) -> Option<FunctionAnalysis> {
    match lower_expr_to_ssa(expr) {
        Ok(ssa) => Some(run_analyses(ssa)),
        Err(_) => None,
    }
}

fn run_analyses(ssa: SsaFunction) -> FunctionAnalysis {
    let escape = escape::analyze(&ssa);
    let region_plan = Arc::new(build_region_plan(&escape));
    let perf = build_performance_facts(&ssa, &escape);
    FunctionAnalysis {
        ssa: Some(ssa),
        escape,
        region_plan,
        perf,
    }
}

fn build_performance_facts(ssa: &SsaFunction, escape: &crate::vm::analysis::EscapeSummary) -> PerformanceFacts {
    let mut facts = PerformanceFacts::default();
    let mut escaping = vec![false; value_capacity(ssa)];
    for &value in &escape.escaping_values {
        if escaping.len() <= value {
            escaping.resize(value + 1, false);
        }
        escaping[value] = true;
    }

    for block in &ssa.blocks {
        for stmt in &block.statements {
            let value_id = stmt.result.index();
            facts.ensure_value(value_id);
            let kind = infer_rvalue_kind(&stmt.value, &facts);
            let escape_class = if escaping.get(value_id).copied().unwrap_or(false) {
                crate::vm::analysis::EscapeClass::Escapes
            } else {
                crate::vm::analysis::EscapeClass::Trivial
            };
            facts.values[value_id] = PerfValueFact {
                kind,
                escape: escape_class,
                move_preferred: !escape_class.is_escaping(),
                must_clone: escape_class.is_escaping(),
            };
        }
    }

    facts
}

fn infer_rvalue_kind(value: &SsaRvalue, facts: &PerformanceFacts) -> PerfValueKind {
    match value {
        SsaRvalue::Const(value) => PerfValueKind::from_val(value),
        SsaRvalue::Param(_) => PerfValueKind::Unknown,
        SsaRvalue::Unary { .. } => PerfValueKind::Unknown,
        SsaRvalue::Binary { op, lhs, rhs } => {
            let lhs = facts.value(lhs.index()).map(|fact| fact.kind).unwrap_or_default();
            let rhs = facts.value(rhs.index()).map(|fact| fact.kind).unwrap_or_default();
            match op {
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Mod
                    if lhs == PerfValueKind::Int && rhs == PerfValueKind::Int =>
                {
                    PerfValueKind::Int
                }
                op if op.is_arith() && (lhs == PerfValueKind::Float || rhs == PerfValueKind::Float) => {
                    PerfValueKind::Float
                }
                op if op.is_cmp() => PerfValueKind::Bool,
                _ => PerfValueKind::Unknown,
            }
        }
        SsaRvalue::List(values) => {
            let _container = PerfContainerFact {
                value_kind: join_value_kinds(
                    values
                        .iter()
                        .filter_map(|value| facts.value(value.index()).map(|fact| fact.kind)),
                ),
                known_len: Some(values.len()),
                adoptable: values.is_empty(),
            };
            PerfValueKind::List
        }
        SsaRvalue::Map(entries) => {
            let _container = PerfContainerFact {
                value_kind: join_value_kinds(
                    entries
                        .iter()
                        .filter_map(|(_, value)| facts.value(value.index()).map(|fact| fact.kind)),
                ),
                known_len: Some(entries.len()),
                adoptable: entries.is_empty(),
            };
            PerfValueKind::Map
        }
        SsaRvalue::StructLiteral { .. } | SsaRvalue::Call { .. } => PerfValueKind::Unknown,
        SsaRvalue::Phi { sources } => join_value_kinds(
            sources
                .iter()
                .filter_map(|source| facts.value(source.value.index()).map(|fact| fact.kind)),
        ),
    }
}

fn join_value_kinds(kinds: impl IntoIterator<Item = PerfValueKind>) -> PerfValueKind {
    let mut iter = kinds.into_iter();
    let Some(first) = iter.next() else {
        return PerfValueKind::Unknown;
    };
    iter.fold(first, PerfValueKind::join)
}

fn value_capacity(ssa: &SsaFunction) -> usize {
    ssa.blocks
        .iter()
        .flat_map(|block| block.statements.iter().map(|stmt| stmt.result.index()))
        .max()
        .map(|idx| idx + 1)
        .unwrap_or(0)
}

fn build_region_plan(summary: &crate::vm::analysis::EscapeSummary) -> RegionPlan {
    let mut plan = RegionPlan::default();
    for &value in &summary.escaping_values {
        ensure_len(&mut plan.values, value + 1);
        plan.values[value] = AllocationRegion::Heap;
    }
    plan.return_region = if summary.return_class.is_escaping() {
        AllocationRegion::Heap
    } else {
        AllocationRegion::ThreadLocal
    };
    plan
}

fn ensure_len<T: Default>(vec: &mut Vec<T>, len: usize) {
    if vec.len() < len {
        vec.resize_with(len, Default::default);
    }
}
