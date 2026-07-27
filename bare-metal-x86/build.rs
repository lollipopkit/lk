//! Compiles `program.lk` to a native x86-64 object and links it in.
//!
//! This is what `lk compile object:<triple>` is for: LK hands over a
//! relocatable object and an ordinary embedded build places it. Nothing here
//! knows anything about LK beyond a command line — the same shape works from a
//! Makefile or a CMake target.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=program.lk");
    // The drivers `program.lk` imports are part of the input. Watching only the
    // entry file meant editing a driver left `program.o` stale — a rebuild that
    // silently ran the *previous* driver, which is worse than not rebuilding.
    // The directory rather than each file in it: cargo then notices a driver
    // being added or removed too, which listing today's files cannot.
    println!("cargo:rerun-if-changed=drivers");
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
    let object = out_dir.join("program.o");
    let target = std::env::var("TARGET").expect("TARGET is set by cargo");

    // An explicit path in CI, otherwise whatever is installed.
    let lk = std::env::var("LK_BIN").unwrap_or_else(|_| "lk".to_string());
    let status = Command::new(&lk)
        .arg("compile")
        .arg(format!("object:{target}"))
        .arg("program.lk")
        .arg("--output")
        .arg(&object)
        .status();

    match status {
        Ok(status) if status.success() => {}
        Ok(status) => panic!("`{lk} compile object:{target}` failed with {status}"),
        Err(error) => {
            panic!("could not run `{lk}`: {error}. Set LK_BIN to its path, or `cargo install --path cli`.")
        }
    }

    // `rustc-link-arg` is additive and cannot be clobbered by a `RUSTFLAGS` in
    // the environment — the same reason the interpreter demo passes its linker
    // script this way.
    println!("cargo:rustc-link-arg={}", object.display());

    // The memory map. Without it the linker picks its own base address and the
    // image loads somewhere the boot protocol does not jump to.
    let script = std::fs::canonicalize("link.ld").expect("link.ld exists");
    println!("cargo:rerun-if-changed=link.ld");
    println!("cargo:rustc-link-arg=-T{}", script.display());
}
