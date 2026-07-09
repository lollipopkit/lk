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
fn hybrid_bridged_results_flow_back_and_match_the_vm() {
    // v2 return bridge: consumed results come back as `LkDyn` through
    // `lk_hybrid_call_r` and feed native Dyn arithmetic/display — every
    // scalar kind, byte-identical to the VM.
    let dir = std::env::temp_dir().join(format!("lk_hybrid_cli_ret_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create tmp dir");
    let file = dir.join("ret.lk");
    std::fs::write(
        &file,
        "fn geti(x) { let f = \"i={}\".trim(); println(f, x); return x + 1; }\n\
         fn getf(x) { let f = \"f={}\".trim(); println(f, x); return x * 2.5; }\n\
         fn getb(x) { let f = \"b={}\".trim(); println(f, x); return x > 2; }\n\
         fn gets(x) { let f = \"s={}\".trim(); println(f, x); return \"long-answer-\" + x; }\n\
         fn getn(x) { let f = \"n={}\".trim(); println(f, x); return nil; }\n\
         println(geti(3) + 10);\n\
         println(getf(2.0) + 1.0);\n\
         println(getb(3));\n\
         println(gets(\"tail\"));\n\
         println(getn(1) == nil);\n\
         return 0;\n",
    )
    .expect("write program");

    let vm = Command::new(bin_path())
        .current_dir(&dir)
        .arg("ret.lk")
        .env("LK_FORCE_VM", "1")
        .output()
        .expect("vm run");
    assert!(vm.status.success(), "vm: {}", String::from_utf8_lossy(&vm.stderr));

    let compile = Command::new(bin_path())
        .current_dir(&dir)
        .args(["compile", "ret.lk"])
        .env("LK_AOT_HYBRID", "1")
        .output()
        .expect("hybrid compile");
    let compile_stderr = String::from_utf8_lossy(&compile.stderr).into_owned();
    assert!(compile.status.success(), "compile: {compile_stderr}");
    assert!(
        compile_stderr.contains("Tier 1 hybrid"),
        "expected the hybrid link path, got: {compile_stderr}"
    );
    assert!(
        !compile_stderr.contains("falling back"),
        "hybrid compile must not fall back to Tier 0: {compile_stderr}"
    );

    let native = Command::new(dir.join("ret")).output().expect("native run");
    assert_eq!(
        String::from_utf8_lossy(&vm.stdout),
        String::from_utf8_lossy(&native.stdout),
        "bridged results must match the VM byte-for-byte"
    );
    assert_eq!(vm.status.success(), native.status.success());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hybrid_bridged_containers_deep_convert_and_match_the_vm() {
    // v2 C5: list/map returns deep-convert through the wrapper-injected lkrt
    // constructor table — element reads, Dyn arithmetic, nested indexing and
    // whole-container display all byte-identical to the VM (map entry order
    // = the VM's own iteration order, replayed insert-by-insert).
    let dir = std::env::temp_dir().join(format!("lk_hybrid_cli_cont_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create tmp dir");
    let file = dir.join("cont.lk");
    std::fs::write(
        &file,
        "fn rows(n) { let f = \"n={}\".trim(); println(f, n); return [n, n * 10, 2.5]; }\n\
         fn user(name) { let f = \"u={}\".trim(); println(f, name); \
         return {\"name\": name, \"score\": 95, \"tags\": [1, 2]}; }\n\
         let r = rows(3);\n\
         println(r);\n\
         println(r[1]);\n\
         let u = user(\"Alice\");\n\
         println(u);\n\
         println(u.score + 5);\n\
         println(u.tags[1]);\n\
         return 0;\n",
    )
    .expect("write program");

    let vm = Command::new(bin_path())
        .current_dir(&dir)
        .arg("cont.lk")
        .env("LK_FORCE_VM", "1")
        .output()
        .expect("vm run");
    assert!(vm.status.success(), "vm: {}", String::from_utf8_lossy(&vm.stderr));

    let compile = Command::new(bin_path())
        .current_dir(&dir)
        .args(["compile", "cont.lk"])
        .env("LK_AOT_HYBRID", "1")
        .output()
        .expect("hybrid compile");
    let compile_stderr = String::from_utf8_lossy(&compile.stderr).into_owned();
    assert!(compile.status.success(), "compile: {compile_stderr}");
    assert!(
        compile_stderr.contains("Tier 1 hybrid"),
        "expected the hybrid link path, got: {compile_stderr}"
    );
    assert!(
        !compile_stderr.contains("falling back"),
        "hybrid compile must not fall back to Tier 0: {compile_stderr}"
    );

    let native = Command::new(dir.join("cont")).output().expect("native run");
    assert_eq!(
        String::from_utf8_lossy(&vm.stdout),
        String::from_utf8_lossy(&native.stdout),
        "bridged containers must match the VM byte-for-byte"
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
