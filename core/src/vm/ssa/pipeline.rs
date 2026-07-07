#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use alloc::sync::Arc;

use crate::operator::BinOp;
use crate::vm::alloc::{AllocationRegion, RegionPlan};
use crate::vm::analysis::{FunctionAnalysis, PerfContainerFact, PerfValueFact, PerfValueKind, PerformanceFacts};

use crate::expr::Expr;
use crate::vm::ssa::{SsaFunction, SsaRvalue, escape, lower_expr_to_ssa};

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
            let value_facts = infer_rvalue_facts(&stmt.value, &facts);
            let escape_class = if escaping.get(value_id).copied().unwrap_or(false) {
                crate::vm::analysis::EscapeClass::Escapes
            } else {
                crate::vm::analysis::EscapeClass::Trivial
            };
            facts.values[value_id] = PerfValueFact {
                kind: value_facts.kind,
                escape: escape_class,
                move_preferred: !escape_class.is_escaping(),
                must_clone: escape_class.is_escaping(),
            };
            if let Some(list) = value_facts.list {
                facts.set_value_list_fact(value_id, list);
            }
            if let Some(map) = value_facts.map {
                facts.set_value_map_fact(value_id, map);
            }
        }
    }

    facts
}

#[derive(Debug, Clone, Copy, Default)]
struct InferredValueFacts {
    kind: PerfValueKind,
    list: Option<PerfContainerFact>,
    map: Option<PerfContainerFact>,
}

fn infer_rvalue_facts(value: &SsaRvalue, facts: &PerformanceFacts) -> InferredValueFacts {
    match value {
        SsaRvalue::Const(value) => InferredValueFacts {
            kind: PerfValueKind::from_literal(value),
            ..InferredValueFacts::default()
        },
        SsaRvalue::Param(_) | SsaRvalue::Unary { .. } => InferredValueFacts::default(),
        SsaRvalue::Binary { op, lhs, rhs } => {
            let lhs = facts.value(lhs.index()).map(|fact| fact.kind).unwrap_or_default();
            let rhs = facts.value(rhs.index()).map(|fact| fact.kind).unwrap_or_default();
            let kind = match op {
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
            };
            InferredValueFacts {
                kind,
                ..InferredValueFacts::default()
            }
        }
        SsaRvalue::List(values) => {
            let list = PerfContainerFact {
                value_kind: join_value_kinds(
                    values
                        .iter()
                        .filter_map(|value| facts.value(value.index()).map(|fact| fact.kind)),
                ),
                known_len: Some(values.len()),
                adoptable: values.is_empty(),
            };
            InferredValueFacts {
                kind: PerfValueKind::List,
                list: Some(list),
                ..InferredValueFacts::default()
            }
        }
        SsaRvalue::Map(entries) => {
            let map = PerfContainerFact {
                value_kind: join_value_kinds(
                    entries
                        .iter()
                        .filter_map(|(_, value)| facts.value(value.index()).map(|fact| fact.kind)),
                ),
                known_len: Some(entries.len()),
                adoptable: entries.is_empty(),
            };
            InferredValueFacts {
                kind: PerfValueKind::Map,
                map: Some(map),
                ..InferredValueFacts::default()
            }
        }
        SsaRvalue::StructLiteral { .. } | SsaRvalue::Call { .. } => InferredValueFacts::default(),
        SsaRvalue::Phi { sources } => InferredValueFacts {
            kind: join_value_kinds(
                sources
                    .iter()
                    .filter_map(|source| facts.value(source.value.index()).map(|fact| fact.kind)),
            ),
            ..InferredValueFacts::default()
        },
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

#[cfg(test)]
mod tests {
    use crate::{expr::Expr, val::LiteralVal, vm::analysis::PerfValueKind};

    use super::analyze_expr;

    #[test]
    fn performance_facts_record_list_container_shape() {
        let analysis = analyze_expr(&Expr::List(vec![
            Box::new(Expr::Literal(LiteralVal::Int(1))),
            Box::new(Expr::Literal(LiteralVal::Int(2))),
        ]))
        .expect("analysis");
        let ssa = analysis.ssa.as_ref().expect("ssa");
        let value_id = ssa.blocks[ssa.entry.index()]
            .statements
            .last()
            .expect("list statement")
            .result
            .index();

        assert_eq!(
            analysis.perf.value(value_id).map(|fact| fact.kind),
            Some(PerfValueKind::List)
        );
        let list = analysis.perf.value_list(value_id).expect("list fact");
        assert_eq!(list.value_kind, PerfValueKind::Int);
        assert_eq!(list.known_len, Some(2));
        assert!(!list.adoptable);
    }

    #[test]
    fn performance_facts_record_map_container_shape() {
        let analysis = analyze_expr(&Expr::Map(vec![(
            Box::new(Expr::Literal(LiteralVal::from_str("answer"))),
            Box::new(Expr::Literal(LiteralVal::Int(42))),
        )]))
        .expect("analysis");
        let ssa = analysis.ssa.as_ref().expect("ssa");
        let value_id = ssa.blocks[ssa.entry.index()]
            .statements
            .last()
            .expect("map statement")
            .result
            .index();

        assert_eq!(
            analysis.perf.value(value_id).map(|fact| fact.kind),
            Some(PerfValueKind::Map)
        );
        let map = analysis.perf.value_map(value_id).expect("map fact");
        assert_eq!(map.value_kind, PerfValueKind::Int);
        assert_eq!(map.known_len, Some(1));
        assert!(!map.adoptable);
    }
}
