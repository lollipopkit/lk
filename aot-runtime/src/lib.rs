//! Static runtime boundary for LK AOT executables.
//!
//! This crate intentionally owns the staticlib linked by `lk compile exe`.
//! Keeping AOT symbols behind a dedicated crate lets the CLI link one runtime
//! archive and gives future work a narrow place to remove remaining VM pieces.

pub use lk_core::llvm::runtime::*;

#[used]
static KEEP_STDLIB_LINKED: fn(&mut lk_core::module::ModuleRegistry) = lk_stdlib::register_stdlib_core_globals;
