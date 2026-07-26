//! Hands the linker script to the link step.
//!
//! This is deliberately *not* done through `.cargo/config.toml`'s `rustflags`:
//! a `RUSTFLAGS` environment variable replaces that table wholesale rather than
//! adding to it, and CI sets `RUSTFLAGS=-D warnings`. The link then silently
//! proceeds without `-Tlink.x`, producing an ELF with no vector table that
//! builds fine and locks up the moment it boots (PC=0). `rustc-link-arg` from a
//! build script is additive and cannot be clobbered that way.

use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

fn main() {
    // `cortex-m-rt`'s link.x does `INCLUDE memory.x`, which has to be findable
    // on the linker search path.
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
    File::create(out.join("memory.x"))
        .expect("create memory.x in OUT_DIR")
        .write_all(include_bytes!("memory.x"))
        .expect("write memory.x");
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rustc-link-arg=-Tlink.x");
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=build.rs");
}
