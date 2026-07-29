//! The LK language server, as a library the binary is a shell over.
//!
//! `main.rs` used to declare `mod analyzer; mod server;` of its own, so the
//! whole crate compiled **twice** — once as this lib, once inside the binary.
//! Besides the build time, it made the lib's dead-code analysis wrong: every
//! `analyzer` method whose only caller lives in `server` looked unused from
//! here, and `-D warnings` (which CI sets) failed on one of them.
pub mod analyzer;
pub mod server;

pub use analyzer::LkAnalyzer;
pub use server::compute_inlay_hints;

#[cfg(test)]
mod bench_test;
#[cfg(test)]
mod editor_grammar_test;
#[cfg(test)]
mod inlay_hint_test;
