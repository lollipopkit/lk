#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
/// Allocation region selected by escape analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AllocationRegion {
    #[default]
    ThreadLocal,
    Heap,
}

/// Plan produced for a function describing how SSA values should be allocated.
#[derive(Debug, Clone, Default)]
pub struct RegionPlan {
    /// Allocation class per SSA value index.
    pub values: Vec<AllocationRegion>,
    /// Allocation class for the function return value (by convention index = `values.len()`).
    pub return_region: AllocationRegion,
}

impl RegionPlan {
    pub fn region_for(&self, value_index: usize) -> AllocationRegion {
        self.values
            .get(value_index)
            .copied()
            .unwrap_or(AllocationRegion::ThreadLocal)
    }
}
