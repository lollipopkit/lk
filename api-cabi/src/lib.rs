//! `lk-api` packaged as a C-ABI static library.
//!
//! The AOT driver links this archive into a Tier 0 VM bundle and a Tier 1
//! hybrid binary — both need the embedded interpreter and the `lk_hybrid_*`
//! bridge. It is a separate crate because `crate-type` cannot be conditional:
//! `lk-api` also declaring `staticlib` meant that building `lk-cli`, or running
//! `cargo build --workspace`, emitted a 172MB archive that only the linker path
//! ever opens.
//!
//! Nothing depends on this crate, so it is built only when named
//! (`cargo build -p lk-api-cabi --release`), which is what
//! `ensure_lk_api_staticlib` does.

// The re-export is what pulls `lk-api`'s `#[no_mangle]` symbols into the
// archive. Without a reference the linker has no reason to keep them.
pub use lk_api::*;
