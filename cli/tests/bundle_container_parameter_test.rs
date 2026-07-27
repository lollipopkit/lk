//! What a bundled module may do with a container it was handed.
//!
//! Bundling flattens modules into one, so a container the caller passes arrives
//! by *reference*, where the VM would have given the module its own copy. That
//! difference is real and it is why a module that mutates through a parameter is
//! refused rather than bundled — the two backends would compute different
//! things, and no differential test would catch it because the VM's answer is
//! the only one anybody wrote down.
//!
//! But the difference is observable only through a **write**: this function's
//! own, or someone else's through a handle it kept. A method that reads the
//! receiver and answers a number can do neither, and refusing those cost more
//! than it bought — a bundled module could not have `fn log(message: String)`,
//! because strings are immutable so *every* string method is a read.
//!
//! These tests pin both halves: the reads bundle, the writes still do not, and
//! a user-defined method that merely shares a name with a read is not mistaken
//! for one.

use std::path::Path;

/// A module that takes a `String` and looks at it.
///
/// The case that found this. `uart_text(text: String)` in a bare-metal serial
/// driver walks the string with `byte_at` — no allocation, usable from an
/// interrupt — and the whole module was refused for it.
#[test]
fn a_bundled_module_may_read_a_string_parameter() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("log.lk"),
        "fn checksum(message: String) -> Int {\n\
        \x20   let sum = 0;\n\
        \x20   for i in 0..message.len() { sum = sum + message.byte_at(i); }\n\
        \x20   return sum;\n\
         }\n\
         fn shouts(message: String) -> Bool { return message.starts_with(\"!\"); }\n",
    )
    .expect("write module");

    let source = dir.path().join("main.lk");
    std::fs::write(
        &source,
        "use { checksum, shouts } from \"log\";\n\
         println(checksum(\"net: ok\"));\n\
         println(shouts(\"!boom\"));\n\
         println(shouts(\"quiet\"));\n",
    )
    .expect("write main");

    assert_native_agrees_with_vm(&source, dir.path().join("reads_string"));
}

/// The same for a list, which is the case the rule exists for — read it, do not
/// write it.
#[test]
fn a_bundled_module_may_read_a_list_parameter() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("stats.lk"),
        "fn total(xs: List<Int>) -> Int {\n\
        \x20   let sum = 0;\n\
        \x20   for i in 0..xs.len() { sum = sum + (xs[i] as Int); }\n\
        \x20   return sum;\n\
         }\n",
    )
    .expect("write module");

    let source = dir.path().join("main.lk");
    std::fs::write(
        &source,
        "use { total } from \"stats\";\n\
         let xs = [1, 2, 3];\n\
         println(total(xs));\n\
         xs.push(4);\n\
         println(total(xs));\n",
    )
    .expect("write main");

    assert_native_agrees_with_vm(&source, dir.path().join("reads_list"));
}

/// A module that writes through a parameter is still refused.
///
/// This is the guarantee the narrowing must not have weakened. Under the VM the
/// caller's list is untouched — the module got a copy — and under a flattened
/// build it would grow. Refusing is what keeps the two the same program.
#[test]
fn a_bundled_module_that_writes_through_a_parameter_is_still_refused() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("grow.lk"),
        "fn extend(xs: List<Int>) -> Int {\n\
        \x20   xs.push(99);\n\
        \x20   return xs.len();\n\
         }\n",
    )
    .expect("write module");

    let source = dir.path().join("main.lk");
    std::fs::write(
        &source,
        "use { extend } from \"grow\";\nlet xs = [1];\nprintln(extend(xs));\nprintln(xs.len());\n",
    )
    .expect("write main");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
        .args(["compile", source.to_str().expect("utf-8 path")])
        .arg("--output")
        .arg(dir.path().join("grow_exe").to_str().expect("utf-8 path"))
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .output()
        .expect("run lk compile");
    assert!(
        !output.status.success(),
        "a module that pushes through a parameter must not be bundled"
    );
    // And says *why*. Without bundling, the call into the module is a
    // `GetGlobal` that resolves to nothing, so the bare failure names a symptom
    // — "global `extend` does not resolve" sends the reader looking for a
    // missing import. The decline reason travels with it.
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(
        message.contains("container parameter"),
        "the diagnostic should say what it refused, got: {message}"
    );
}

/// A user-defined method that happens to be called `contains` is not the
/// builtin, and is not assumed to read.
///
/// The allow list is names, because the bytecode carries no types. What makes
/// that safe is that a name any `impl` in the module defines is excluded from
/// it — nothing stops a type from having a `contains` that rearranges the
/// receiver first, and this is that type.
#[test]
fn a_user_method_sharing_a_read_only_name_is_not_treated_as_one() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("sneaky.lk"),
        "struct Bag { items: List<Int> }\n\
         impl Bag {\n\
        \x20   fn contains(self, needle: Int) -> Bool {\n\
        \x20       self.items.push(needle);\n\
        \x20       return true;\n\
        \x20   }\n\
         }\n\
         fn probe(bag: Bag) -> Bool { return bag.contains(7); }\n",
    )
    .expect("write module");

    let source = dir.path().join("main.lk");
    std::fs::write(
        &source,
        "use { probe, Bag } from \"sneaky\";\n\
         let bag = Bag { items: [1] };\n\
         println(probe(bag));\n\
         println(bag.items.len());\n",
    )
    .expect("write main");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
        .args(["compile", source.to_str().expect("utf-8 path")])
        .arg("--output")
        .arg(dir.path().join("sneaky_exe").to_str().expect("utf-8 path"))
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .output()
        .expect("run lk compile");
    assert!(
        !output.status.success(),
        "a user `contains` that mutates must not be mistaken for the builtin read"
    );
}

/// Compiles `source` natively, runs it, and checks it says what the VM says.
///
/// Against the VM rather than against numbers, because the failure this whole
/// area is about is a bundled build that computes something the VM does not.
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
        // Pinned: a fall back to the VM bundle would pass without the module
        // ever having been bundled, which is the thing under test.
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
