//! What a bundled module may say at its top level, and why the answer is more
//! than "a literal".
//!
//! Bundling flattens an imported module into the importing one, so the module's
//! top level has to be effect-free: there is nowhere for it to *run*. The scan
//! that enforces that used to accept only a load followed by a bind, which made
//! `const FRAME = HEADER + BODY;` a "top-level effect" while `const FRAME = 42;`
//! was fine — and deriving one constant from two others is the ordinary shape of
//! a protocol or register header. The alternative is the same number written
//! twice, in a file whose whole purpose is that it is written once.
//!
//! So the scan folds. These tests pin what it folds, that it folds to the value
//! the VM computes, and that a module whose top level really does have an effect
//! is still refused.

use std::path::Path;

/// A constant derived from other constants, through every folded shape.
///
/// Checked against the VM rather than against numbers written here: the fold is
/// a second implementation of arithmetic the executor already does, and the
/// failure it would produce is a program that compiles and computes something
/// else.
#[test]
fn a_bundled_module_may_derive_a_constant_from_constants() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("proto.lk"),
        // Addition, subtraction, multiplication, a chain three deep, and the
        // immediate forms the compiler picks when one side is a literal.
        "const HEADER = 14;\n\
         const BODY = 28;\n\
         const FRAME = HEADER + BODY;\n\
         const PAYLOAD = FRAME - HEADER;\n\
         const BURST = FRAME * 4;\n\
         const RING = BURST + 1;\n\
         const SLOT = HEADER * BODY - FRAME;\n\
         fn describe() -> Int { return FRAME + PAYLOAD + BURST + RING + SLOT; }\n",
    )
    .expect("write module");

    let source = dir.path().join("main.lk");
    std::fs::write(
        &source,
        "use { describe, FRAME, PAYLOAD, BURST, RING, SLOT } from \"proto\";\n\
         println(FRAME);\n\
         println(PAYLOAD);\n\
         println(BURST);\n\
         println(RING);\n\
         println(SLOT);\n\
         println(describe());\n",
    )
    .expect("write main");

    assert_native_agrees_with_vm(&source, dir.path().join("derived"));
}

/// A negative value, and the wrapping the executor does.
///
/// The fold has to match the VM at the edges as well as in the middle: a fold
/// that saturated or panicked where the executor wraps would reject a program
/// the VM runs, which is worse than computing the wrong answer because it looks
/// like a missing feature.
#[test]
fn a_derived_constant_wraps_where_the_executor_wraps() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("edges.lk"),
        "const LOW = 5;\n\
         const HIGH = 9;\n\
         const BELOW = LOW - HIGH;\n\
         const HUGE = 4611686018427387904;\n\
         const OVER = HUGE + HUGE;\n\
         fn edges() -> Int { return BELOW + OVER; }\n",
    )
    .expect("write module");

    let source = dir.path().join("main.lk");
    std::fs::write(
        &source,
        "use { edges, BELOW, OVER } from \"edges\";\n\
         println(BELOW);\n\
         println(OVER);\n\
         println(edges());\n",
    )
    .expect("write main");

    assert_native_agrees_with_vm(&source, dir.path().join("edges"));
}

/// A module whose top level does something is still refused, and says so.
///
/// The fold widens what counts as a *description*; it does not make a module's
/// top level a place where things happen. A bundled module has nowhere to run,
/// so a call there would be silently skipped — which is exactly the failure the
/// scan exists to prevent.
#[test]
fn a_bundled_module_with_a_real_top_level_effect_is_still_refused() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("noisy.lk"),
        "fn shout() -> Int { println(\"side effect\"); return 1; }\n\
         const RESULT = shout();\n",
    )
    .expect("write module");

    let source = dir.path().join("main.lk");
    std::fs::write(&source, "use { RESULT } from \"noisy\";\nprintln(RESULT);\n").expect("write main");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
        .args(["compile", source.to_str().expect("utf-8 path")])
        .arg("--output")
        .arg(dir.path().join("noisy_exe").to_str().expect("utf-8 path"))
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .output()
        .expect("run lk compile");
    assert!(!output.status.success(), "a top-level call must not be bundled");
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(
        message.contains("top-level"),
        "the diagnostic should say what it refused, got: {message}"
    );
}

/// Compiles `source` natively, runs it, and checks it says what the VM says.
fn assert_native_agrees_with_vm(source: &Path, exe: std::path::PathBuf) {
    let vm = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
        .arg(source.to_str().expect("utf-8 path"))
        .env("LK_FORCE_VM", "1")
        .output()
        .expect("run under the VM");
    assert!(
        vm.status.success(),
        "the VM must run it: {}",
        String::from_utf8_lossy(&vm.stderr)
    );

    let status = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
        .args(["compile", source.to_str().expect("utf-8 path")])
        .arg("--output")
        .arg(exe.to_str().expect("utf-8 path"))
        // Pinned to the native path: a fall back to the VM bundle would run the
        // module's top level for real and pass without the fold existing.
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .status()
        .expect("run lk compile");
    assert!(status.success(), "must lower natively");

    let native = std::process::Command::new(&exe)
        .output()
        .expect("run the compiled program");
    assert_eq!(
        String::from_utf8_lossy(&vm.stdout),
        String::from_utf8_lossy(&native.stdout),
        "the VM and the native build disagree"
    );
}
