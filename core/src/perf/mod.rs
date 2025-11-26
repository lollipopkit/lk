//! Shared performance benchmarking scaffolding used by benches and reporting
//! utilities.
//!
//! Centralizing the workloads here keeps Criterion benches, ad-hoc profilers,
//! and CI-facing reporters in sync so we do not accidentally compare different
//! scenarios across tools.

pub mod scenarios;
