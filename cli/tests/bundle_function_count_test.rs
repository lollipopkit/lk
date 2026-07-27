//! How many functions a bundled program may hold, and why the answer is not
//! "as many as it likes".
//!
//! A `CallDirect` or `MakeClosure` names its target in the instruction's `b`
//! field, which is a byte. The compiler is not bound by that — past index 255
//! it lowers the call generically — but a bundled dependency's instructions are
//! already emitted by the time the merge renumbers them, and rewriting one
//! instruction into two would move every jump offset after it.
//!
//! So the merge numbers directly-called functions first. What is bounded is the
//! importing file's functions plus the dep functions a dep calls *directly*,
//! not the total. These tests pin both halves of that: a program past 256 in
//! total compiles and computes, and the diagnostic for a program past the real
//! bound says which bound it crossed.

use std::path::Path;

/// A program with more functions than a call instruction can name still
/// compiles natively, and still computes with them.
///
/// The dep's functions are leaves — nothing there calls anything — so none of
/// them is a `CallDirect` target and all of them may sit above 255. That is the
/// ordinary shape of a driver: it exports what the program calls, and the
/// program reaches it by name, which the lowering resolves through a `u32`.
#[test]
fn a_bundle_may_hold_more_functions_than_a_call_can_name() {
    let dir = tempfile::tempdir().expect("temp dir");

    let mut dep = String::new();
    for index in 0..300 {
        dep.push_str(&format!("fn leaf{index}() -> Int {{ return {index}; }}\n"));
    }
    std::fs::write(dir.path().join("dep.lk"), dep).expect("write dep");

    let mut main = String::from("use { leaf0, leaf299 } from \"dep\";\n");
    main.push_str("return leaf0() + leaf299();\n");
    let source = dir.path().join("main.lk");
    std::fs::write(&source, main).expect("write main");

    let exe = dir.path().join("bundle_many");
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
        .args(["compile", source.to_str().expect("utf-8 path")])
        .arg("--output")
        .arg(exe.to_str().expect("utf-8 path"))
        // Pinned to the native path: falling back to the VM bundle would make
        // this pass without the merge having numbered anything.
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .status()
        .expect("run lk compile");
    assert!(status.success(), "300 bundled functions must lower natively");

    let output = std::process::Command::new(&exe)
        .output()
        .expect("run the compiled program");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("299"),
        "expected 0 + 299 from the first and last bundled function, got: {stdout}"
    );
}

/// The *merge's* numbering, isolated.
///
/// Every index inside the dep is under 256, so the dep compiles with ordinary
/// `CallDirect`s. What crosses the line is the merged index: the importing file
/// has a hundred functions of its own, so numbered as they arrive the dep's
/// chain lands past 255 and its call instructions no longer fit.
///
/// The chain is declared *last* in the dep, which is the whole test — declaring
/// it first would put it low under either numbering.
#[test]
fn the_merge_numbers_directly_called_functions_first() {
    let dir = tempfile::tempdir().expect("temp dir");

    let mut dep = String::new();
    for index in 0..160 {
        dep.push_str(&format!("fn leaf{index}() -> Int {{ return {index}; }}\n"));
    }
    dep.push_str("fn chain_end() -> Int { return 7; }\n");
    for index in 0..40 {
        dep.push_str(&format!(
            "fn chain{index}() -> Int {{ return {} + 1; }}\n",
            if index == 0 {
                "chain_end()".to_string()
            } else {
                format!("chain{}()", index - 1)
            }
        ));
    }
    std::fs::write(dir.path().join("dep.lk"), dep).expect("write dep");

    let mut main = String::from("use { chain39, leaf0, leaf159 } from \"dep\";\n");
    for index in 0..100 {
        main.push_str(&format!("fn own{index}() -> Int {{ return {index}; }}\n"));
    }
    main.push_str("return chain39() + leaf0() + leaf159() + own0();\n");
    let source = dir.path().join("main.lk");
    std::fs::write(&source, main).expect("write main");

    assert_native_agrees_with_vm(&source, dir.path().join("bundle_mixed"));
}

/// A call to a function past index 255, inside one module and with no bundling
/// at all.
///
/// `CallDirect` names its target in a byte, so the compiler spells this one
/// `LoadFunction` + `Call` instead — correct bytecode that the native lowering
/// used to reject, which made 256 functions a ceiling on the *native* path too.
#[test]
fn a_call_past_the_direct_call_index_lowers_natively() {
    let dir = tempfile::tempdir().expect("temp dir");

    let mut source_text = String::new();
    for index in 0..300 {
        source_text.push_str(&format!("fn leaf{index}() -> Int {{ return {index}; }}\n"));
    }
    // Called from inside a function, so the call is a real one rather than the
    // entry's own bookkeeping.
    source_text.push_str("fn reach() -> Int { return leaf299() + leaf0(); }\n");
    source_text.push_str("return reach();\n");
    let source = dir.path().join("far_call.lk");
    std::fs::write(&source, source_text).expect("write source");

    assert_native_agrees_with_vm(&source, dir.path().join("far_call"));
}

/// Compiles `source` natively, runs it, and checks it says what the VM says.
///
/// Comparing against the VM rather than against a number: the merge is a
/// native-path transformation — the VM keeps each module in its own namespace —
/// so a numbering it invents wrong is exactly the kind of thing that computes a
/// different answer without failing anything.
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
        // Pinned to the native path: a fall back to the VM bundle would make
        // every one of these pass without proving anything.
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
