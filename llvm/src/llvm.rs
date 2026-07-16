//! Native backend entry point.
//!
//! Lowers `ModuleArtifact`s through the typed MIR pipeline
//! (`lk-aot-lower` → `lk_aot_mir::validate` → `lk-aot-codegen`'s Cranelift
//! backend). Shapes the lowering rejects fail the compile with a precise
//! `Unsupported` reason; there is no VM shell embedding.

mod backend;

pub use backend::{ClifArtifact, compile_artifact_to_clif_object};
