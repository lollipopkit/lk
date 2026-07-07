//! Tier 1 hybrid end-to-end (`docs/llvm/tier1-hybrid.md`, opt-in via
//! `LK_AOT_HYBRID=1`): a program whose helper does not lower natively compiles
//! to a *hybrid* executable (native code + bridged VM-executed function), and
//! its observable behaviour matches the VM exactly — stdout, ordering across
//! the native/VM stdio boundary, and exit codes for uncaught errors.
#![cfg(feature = "llvm")]

use std::path::PathBuf;
use std::process::Command;

fn bin_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lk"))
}

const HYBRID_PROGRAM: &str = "\
fn report(x) { let f = \"acc={}\".trim(); println(f, x); }\n\
let acc = 0;\n\
for i in 0..10 { acc += i; }\n\
report(acc);\n\
println(\"done\");\n\
return 0;\n";

#[test]
fn hybrid_executable_matches_vm_output_and_ordering() {
    let dir = std::env::temp_dir().join(format!("lk_hybrid_cli_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create tmp dir");
    let file = dir.join("hybrid.lk");
    std::fs::write(&file, HYBRID_PROGRAM).expect("write program");

    let vm = Command::new(bin_path())
        .current_dir(&dir)
        .arg("hybrid.lk")
        .env("LK_FORCE_VM", "1")
        .output()
        .expect("vm run");
    assert!(vm.status.success(), "vm: {}", String::from_utf8_lossy(&vm.stderr));

    let compile = Command::new(bin_path())
        .current_dir(&dir)
        .args(["compile", "hybrid.lk"])
        .env("LK_AOT_HYBRID", "1")
        .output()
        .expect("hybrid compile");
    let compile_stderr = String::from_utf8_lossy(&compile.stderr).into_owned();
    assert!(compile.status.success(), "compile: {compile_stderr}");
    // The point of the test is the *hybrid* path: a silent Tier 0 fallback
    // would also produce matching output, so pin the mode.
    assert!(
        compile_stderr.contains("Tier 1 hybrid"),
        "expected the hybrid link path, got: {compile_stderr}"
    );
    assert!(
        !compile_stderr.contains("falling back"),
        "hybrid compile must not fall back to Tier 0: {compile_stderr}"
    );

    let native = Command::new(dir.join("hybrid")).output().expect("native run");
    assert_eq!(
        String::from_utf8_lossy(&vm.stdout),
        String::from_utf8_lossy(&native.stdout),
        "stdout must match the VM (including native/VM print ordering)"
    );
    assert_eq!(vm.status.success(), native.status.success());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hybrid_uncaught_vm_error_exits_nonzero_like_the_vm() {
    let dir = std::env::temp_dir().join(format!("lk_hybrid_cli_err_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create tmp dir");
    let file = dir.join("boom.lk");
    std::fs::write(
        &file,
        "fn boom(x) { let f = \"x={}\".trim(); println(f, x); error(\"bad: ${x}\"); }\n\
         boom(5);\n\
         return 0;\n",
    )
    .expect("write program");

    let vm = Command::new(bin_path())
        .current_dir(&dir)
        .arg("boom.lk")
        .env("LK_FORCE_VM", "1")
        .output()
        .expect("vm run");
    assert!(!vm.status.success(), "the uncaught error must fail the VM run");

    let compile = Command::new(bin_path())
        .current_dir(&dir)
        .args(["compile", "boom.lk"])
        .env("LK_AOT_HYBRID", "1")
        .output()
        .expect("hybrid compile");
    let compile_stderr = String::from_utf8_lossy(&compile.stderr).into_owned();
    assert!(compile.status.success(), "compile: {compile_stderr}");
    assert!(
        compile_stderr.contains("Tier 1 hybrid"),
        "expected the hybrid link path, got: {compile_stderr}"
    );

    let native = Command::new(dir.join("boom")).output().expect("native run");
    assert!(
        !native.status.success(),
        "the bridged uncaught error must fail the hybrid binary too"
    );
    let native_stderr = String::from_utf8_lossy(&native.stderr).into_owned();
    assert!(
        native_stderr.contains("bad: 5"),
        "the VM's rendered error must reach stderr: {native_stderr}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
