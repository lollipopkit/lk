//! Native backend entry point.
//!
//! Lowers `ModuleArtifact`s through the typed MIR pipeline
//! (`lk-aot-lower` → `lk_aot_mir::validate` → `lk-aot-codegen`'s Cranelift
//! backend). A shape the lowering rejects is surfaced as a precise `Unsupported`
//! reason (inner `Err(String)`), which the caller (`lk compile`) turns into a
//! Tier 0 VM-bundle fallback. Tier 1 hybrid is *not* a whole-module VM shell:
//! only the marked non-lowering helpers are VM-executed, bridged from native
//! code — `ClifArtifact::vm_function_count` tells the caller to embed the module
//! artifact and link lk-api (see `compile_native_executable_from_object_hybrid`).

mod backend;

pub use backend::{ClifArtifact, compile_artifact_to_clif_object};
