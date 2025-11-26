use std::sync::Arc;

use crate::vm::analysis::FunctionAnalysis;
use crate::vm::{AllocationRegion, RegionPlan};

use super::{SsaFunction, escape, lower_expr_to_ssa};
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
    FunctionAnalysis {
        ssa: Some(ssa),
        escape,
        region_plan,
    }
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
