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

use anyhow::{Result, bail};

use crate::vm::ModuleArtifact;

/// A Cranelift-compiled native object plus the Tier 1 hybrid bookkeeping the
/// linker needs.
pub struct ClifArtifact {
    /// The relocatable object bytes.
    pub object: Vec<u8>,
    /// Number of VM-executed (bridged) functions. Non-zero means the object
    /// references `lk_hybrid_call_*`, so the link must embed the module artifact
    /// and add the lk-api staticlib (see
    /// [`crate::compile_native_executable_from_object_hybrid`]).
    pub vm_function_count: usize,
}

/// The Cranelift backend's artifact entry point (the sole native codegen): lower
/// to MIR (`hybrid` per `LK_AOT_HYBRID`, so a non-lowering helper bridges to the
/// VM instead of failing the module), validate, and emit a native object. The
/// outer `Result` is a genuine internal failure (validation/codegen bug), never a
/// user error; the inner result is:
/// - `Ok(artifact)` — the whole module lowered; link it (hybrid or plain per
///   `vm_function_count`).
/// - `Err(reason)` — a shape outside the Cranelift slice (MIR lowering or codegen
///   `Unsupported`); the caller may fall back to the Tier 0 VM bundle. `reason`
///   is for diagnostics.
pub fn compile_artifact_to_clif_object(
    artifact: &ModuleArtifact,
    bundles: &[lk_aot_lower::BundledImport],
) -> Result<std::result::Result<ClifArtifact, String>> {
    // `LK_AOT_HYBRID` on unless `=0`: a reachable helper that does not lower
    // natively is bridged to the VM (`docs/aot/tier1-hybrid.md`).
    let hybrid = std::env::var_os("LK_AOT_HYBRID").is_none_or(|value| value != "0");
    let mut mir = match lk_aot_lower::lower_bundled(artifact, bundles, hybrid) {
        Ok(mir) => mir,
        // A shape the MIR lowering itself rejects — fall back, don't fail.
        Err(unsupported) => return Ok(Err(format!("MIR lowering: {unsupported}"))),
    };
    if let Err(error) = lk_aot_mir::validate(&mir) {
        bail!("internal AOT error: MIR validation failed after lowering: {error:?}");
    }
    // Backend-independent cleanup (`Pure`-call CSE + dead-code elimination).
    // Cranelift cannot do this itself for calls to opaque `lkrt` symbols, so
    // the effect metadata in `aot/abi` is only actionable here.
    // `LK_AOT_NO_OPT=1` skips it — a bisect handle when a differential case
    // disagrees, not a supported user knob.
    if std::env::var_os("LK_AOT_NO_OPT").is_none() {
        let stats = lk_aot_mir::opt::optimize(&mut mir);
        if std::env::var_os("LK_AOT_OPT_STATS").is_some() {
            eprintln!(
                "lk-aot opt: {} pure call(s) collapsed, {} dead inst(s) removed",
                stats.cse_calls, stats.dce_insts
            );
            eprintln!(
                "lk-aot opt: {} licm candidate(s)",
                lk_aot_mir::opt::count_licm_candidates(&mir)
            );
        }
        // Optimization must preserve MIR validity; a violation here is our bug,
        // not a user-program limitation, so it fails loudly instead of falling back.
        if let Err(error) = lk_aot_mir::validate(&mir) {
            bail!("internal AOT error: MIR validation failed after optimization: {error:?}");
        }
    }
    let vm_function_count = mir.vm_functions.len();
    match lk_aot_codegen::clif::compile_host_object(&mir) {
        Ok(object) => Ok(Ok(ClifArtifact {
            object,
            vm_function_count,
        })),
        Err(lk_aot_codegen::clif::ClifError::Unsupported(reason)) => Ok(Err(format!("clif: {reason}"))),
        Err(error) => bail!("Cranelift codegen failed: {error:?}"),
    }
}
