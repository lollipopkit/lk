use std::sync::Arc;

use crate::vm::RegionPlan;
use crate::vm::compiler::SsaFunction;

/// Classification of how a value escapes during execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EscapeClass {
    /// Value is compile-time constant or otherwise trivially confined.
    #[default]
    Trivial,
    /// Value remains within the current stack frame but may outlive temporaries.
    Local,
    /// Value may escape the current frame (stored into heap, captured, or returned).
    Escapes,
}

impl EscapeClass {
    pub fn is_escaping(self) -> bool {
        matches!(self, EscapeClass::Escapes)
    }

    pub fn join(self, other: EscapeClass) -> EscapeClass {
        use EscapeClass::*;
        match (self, other) {
            (Escapes, _) | (_, Escapes) => Escapes,
            (Local, _) | (_, Local) => Local,
            _ => Trivial,
        }
    }
}

/// Summary of escape behaviour for the current SSA function.
#[derive(Debug, Clone, Default)]
pub struct EscapeSummary {
    pub return_class: EscapeClass,
    /// SSA values that were classified as escaping.
    pub escaping_values: Vec<usize>,
}

impl EscapeSummary {
    pub fn mark_escaping(&mut self, value: usize) {
        if !self.escaping_values.contains(&value) {
            self.escaping_values.push(value);
        }
    }
}

/// Aggregated analysis artifacts produced by the SSA pipeline.
#[derive(Debug, Clone, Default)]
pub struct FunctionAnalysis {
    pub ssa: Option<SsaFunction>,
    pub escape: EscapeSummary,
    pub region_plan: Arc<RegionPlan>,
}
