//! Cranelift-backend differential harness (`docs/llvm/aot-redesign.md` §6).
//! Cranelift is the sole native codegen; each case is compiled with
//! `LK_AOT_NO_FALLBACK=1` so a shape it can't lower fails the compile instead of
//! silently falling back to the Tier 0 VM bundle — guaranteeing the case runs
//! *through Cranelift* — then run and diffed against the bytecode VM. Guards the
//! native coverage (nil/fn-addr, DynVal maps, carriers, trait dispatch, typed
//! lists, hybrid bridge) against regressions.
#![cfg(feature = "llvm")]

use std::ffi::OsStr;
use std::fs::{self, File, create_dir_all};
use std::io::Write;
use std::path::{Path as FsPath, PathBuf};
use std::process::Command;

fn bin_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lk"))
}

fn unique_tmp_dir(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("lk_clif_diff_{name}_{}", std::process::id()));
    p
}

fn run_cli<I, S>(dir: &FsPath, args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new(bin_path());
    cmd.current_dir(dir).args(args);
    cmd
}

struct Case {
    name: &'static str,
    source: &'static str,
}

const fn new(name: &'static str, source: &'static str) -> Case {
    Case { name, source }
}

/// Compile each case through Cranelift (forced, no fallback), run it, run the
/// same source under the VM, and require identical stdout and identical
/// success/failure.
fn run_clif_differential(area: &str, cases: &[Case]) {
    let dir = unique_tmp_dir(area);
    let _ = fs::remove_dir_all(&dir);
    create_dir_all(&dir).expect("create tmp dir");

    for case in cases {
        let file = format!("{}.lk", case.name);
        let path = dir.join(&file);
        File::create(&path)
            .and_then(|mut f| f.write_all(case.source.as_bytes()))
            .expect("write case file");

        // VM reference run.
        let vm = run_cli(&dir, [file.as_str()]).output().expect("spawn vm run");
        let vm_stdout = String::from_utf8_lossy(&vm.stdout).into_owned();

        // Native build. Cranelift is the sole native backend; `LK_AOT_NO_FALLBACK`
        // makes a shape it can't lower a hard error instead of a Tier 0 VM bundle,
        // so the case is guaranteed to run *through Cranelift*.
        let exe = run_cli(&dir, ["compile", &file])
            .env("LK_AOT_NO_FALLBACK", "1")
            .env("LK_AOT_HYBRID", "0")
            .output()
            .expect("spawn native compile");
        assert!(
            exe.status.success(),
            "[{area}/{}] Cranelift native compile failed: {}",
            case.name,
            String::from_utf8_lossy(&exe.stderr)
        );

        let native = Command::new(dir.join(case.name))
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .output()
            .expect("spawn compiled executable");
        let native_stdout = String::from_utf8_lossy(&native.stdout).into_owned();

        assert_eq!(
            vm_stdout,
            native_stdout,
            "[{area}/{}] stdout diverged (vm vs clif): vm={:?} clif={:?} stderr(vm)={} stderr(clif)={}",
            case.name,
            vm.status,
            native.status,
            String::from_utf8_lossy(&vm.stderr),
            String::from_utf8_lossy(&native.stderr)
        );
        assert_eq!(
            vm.status.success(),
            native.status.success(),
            "[{area}/{}] success/failure diverged: vm={:?} clif={:?}",
            case.name,
            vm.status,
            native.status,
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn clif_differential_scalars_and_flow() {
    run_clif_differential(
        "scalars",
        &[
            new("arith", "return 1 + 2 * 3;\n"),
            new("div_guard", "let x = 20;\nlet y = 4;\nreturn x / y;\n"),
            new("float_add", "return 1.5 + 2.5;\n"),
            new("if_else", "let x = 9;\nif (x < 5) { return 10; }\nreturn 20;\n"),
            new(
                "while_sum",
                "let s = 0;\nlet i = 1;\nwhile (i <= 10) { s = s + i; i = i + 1; }\nreturn s;\n",
            ),
            new("direct_call", "fn add(a, b) {\n  return a + b;\n}\nreturn add(2, 3);\n"),
            new("nil_literal", "let x = nil;\nif (x == nil) { return 1; }\nreturn 0;\n"),
        ],
    );
}

#[test]
fn clif_differential_containers_and_dyn() {
    run_clif_differential(
        "containers",
        &[
            new("list_len", "let xs = [1, 2, 3];\nreturn xs.len();\n"),
            new("list_index", "let xs = [10, 20, 30];\nreturn xs[1] + xs[2];\n"),
            new("map_len", "let m = {\"a\": 1, \"b\": 2};\nreturn m.len();\n"),
            new(
                "map_get",
                "let m = {\"a\": 1, \"b\": 2};\nprintln(m[\"a\"]);\nreturn m[\"b\"];\n",
            ),
            new("map_absent", "let m = {\"a\": 1};\nreturn m[\"z\"];\n"),
            new("str_concat", "let s = \"a\" + \"b\";\nreturn s.len();\n"),
        ],
    );
}

#[test]
fn clif_differential_higher_order() {
    run_clif_differential(
        "higher_order",
        &[
            // Function-address constants (`Const::FnAddr`) — a lambda passed as a
            // callback into a runtime HOF helper.
            new(
                "map_double",
                "let xs = [1, 2, 3];\nlet ys = xs.map(|x| x * 2);\nreturn ys[0] + ys[1] + ys[2];\n",
            ),
            new(
                "filter_sum",
                "let xs = [1, 2, 3, 4];\nlet ev = xs.filter(|x| x % 2 == 0);\nreturn ev.len();\n",
            ),
            new(
                "reduce_sum",
                "let xs = [1, 2, 3, 4, 5];\nreturn xs.reduce(0, |a, b| a + b);\n",
            ),
        ],
    );
}

/// A hybrid program (a helper that doesn't lower natively bridges to the VM):
/// compiled through Cranelift with `LK_AOT_HYBRID` on and forced clif-only, the
/// stderr must show the Cranelift hybrid link (not a fallback) and stdout must
/// match the VM — including native/VM print ordering across the bridge.
#[test]
fn clif_differential_hybrid_bridge() {
    let dir = unique_tmp_dir("hybrid");
    let _ = fs::remove_dir_all(&dir);
    create_dir_all(&dir).expect("create tmp dir");
    let file = "hybrid.lk";
    // `report`/`geti` use `println(fmt, x)` (a bridged shape); `geti`'s result
    // flows back through `lk_hybrid_call_r` and feeds native arithmetic.
    let src = "fn report(x) { let f = \"acc={}\".trim(); println(f, x); }\n\
               fn geti(x) { let f = \"i={}\".trim(); println(f, x); return x + 1; }\n\
               let acc = 0;\n\
               for i in 0..10 { acc += i; }\n\
               report(acc);\n\
               println(geti(3) + 10);\n\
               println(\"done\");\n\
               return 0;\n";
    File::create(dir.join(file))
        .and_then(|mut f| f.write_all(src.as_bytes()))
        .expect("write program");

    let vm = run_cli(&dir, [file]).env("LK_FORCE_VM", "1").output().expect("vm run");
    let vm_stdout = String::from_utf8_lossy(&vm.stdout).into_owned();

    let compile = run_cli(&dir, ["compile", file])
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "1")
        .output()
        .expect("hybrid compile");
    let compile_stderr = String::from_utf8_lossy(&compile.stderr).into_owned();
    assert!(compile.status.success(), "hybrid compile failed: {compile_stderr}");
    assert!(
        compile_stderr.contains("Tier 1 hybrid"),
        "expected the hybrid link path, got: {compile_stderr}"
    );

    let native = Command::new(dir.join("hybrid"))
        .env("ASAN_OPTIONS", "detect_leaks=0")
        .output()
        .expect("run hybrid executable");
    assert_eq!(
        vm_stdout,
        String::from_utf8_lossy(&native.stdout),
        "hybrid stdout must match the VM (including native/VM ordering)"
    );
    assert_eq!(vm.status.success(), native.status.success());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn clif_differential_try_catch() {
    run_clif_differential(
        "try_catch",
        &[
            // `try$call` through the lkrt `setjmp` trampoline: the success path
            // runs the body, the failure path binds the raised value.
            new(
                "catch_raise",
                "let out = 0;\ntry {\n  error(\"boom\");\n  out = 1;\n} catch e {\n  out = 2;\n}\nreturn out;\n",
            ),
            new(
                "catch_skipped",
                "let out = 0;\ntry {\n  out = 5;\n} catch e {\n  out = 9;\n}\nreturn out;\n",
            ),
            new(
                "catch_with_arg",
                "fn div(a, b) {\n  if (b == 0) { error(\"zero\"); }\n  return a / b;\n}\nlet r = 0;\ntry {\n  r = div(10, 0);\n} catch e {\n  r = -1;\n}\nreturn r;\n",
            ),
        ],
    );
}
