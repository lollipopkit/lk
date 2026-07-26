//! `lkrt` packaged as a C-ABI static library.
//!
//! The AOT driver links hosted executables against this archive. It exists as a
//! separate crate because `crate-type` cannot be made conditional: a
//! `staticlib` must be self-contained, and a bare-metal binary that depends on
//! `lkrt` for its rlib would still have had cargo build the staticlib — failing
//! on a missing allocator and panic handler that the binary itself provides.
//!
//! Splitting them lets each consumer take the form it needs: hosted AOT links
//! this archive, bare-metal links the rlib.

// The re-export is what pulls `lkrt`'s `#[no_mangle]` symbols into the archive.
// Without a reference the linker has no reason to keep them.
pub use lkrt::*;
