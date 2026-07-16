//! `lk-aot-codegen` — the `MirModule` → native code backend.
//!
//! Consumes a validated [`lk_aot_mir::MirModule`] and emits a native relocatable
//! object through Cranelift ([`clif`]). Because the MIR is already typed and
//! SSA-formed, lowering is a straightforward walk; the typed builder plus
//! Cranelift's verifier reject any malformed instruction at compile time (a
//! shape outside the slice returns [`clif::ClifError::Unsupported`]).

pub mod clif;
