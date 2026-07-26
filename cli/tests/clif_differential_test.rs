//! Cranelift-backend differential harness (`docs/aot/aot-redesign.md` §6).
//! Cranelift is the sole native codegen; each case is compiled with
//! `LK_AOT_NO_FALLBACK=1` so a shape it can't lower fails the compile instead of
//! silently falling back to the Tier 0 VM bundle — guaranteeing the case runs
//! *through Cranelift* — then run and diffed against the bytecode VM. Guards the
//! native coverage (nil/fn-addr, DynVal maps, carriers, trait dispatch, typed
//! lists, hybrid bridge) against regressions.
#![cfg(feature = "aot")]

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
    run_differential(area, cases, NativePath::PureCranelift)
}

/// Whether a case must lower fully through Cranelift, or may degrade.
#[derive(Clone, Copy, PartialEq)]
enum NativePath {
    /// `LK_AOT_NO_FALLBACK=1`: a shape Cranelift cannot lower fails the compile,
    /// so the case is guaranteed to run *through Cranelift*.
    PureCranelift,
    /// Fallback allowed. The case still has to produce VM-identical output — that
    /// is the guarantee being kept — but it may reach it through the hybrid bridge
    /// or the Tier 0 VM bundle. Use this only where the native gap is a recorded
    /// debt (see todos.md), never to paper over a lowering regression.
    MayDegrade,
}

fn run_differential(area: &str, cases: &[Case], native_path: NativePath) {
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
        let mut compile = run_cli(&dir, ["compile", &file]);
        compile.env("LK_AOT_HYBRID", "0");
        match native_path {
            NativePath::PureCranelift => {
                compile.env("LK_AOT_NO_FALLBACK", "1");
            }
            // Explicitly cleared, not merely unset: CI runs this binary with
            // `LK_AOT_NO_FALLBACK=1` in the environment, which the child would
            // otherwise inherit and turn the allowed degradation into a hard
            // compile error.
            NativePath::MayDegrade => {
                compile.env_remove("LK_AOT_NO_FALLBACK");
            }
        }
        let exe = compile.output().expect("spawn native compile");
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
/// A value that came back from the bridge, fed to a *native* helper, whose result
/// is then interpolated.
///
/// Marking a function VM-executed changes the type lattice, and signatures have
/// to re-converge before the module is emitted. They did not: the final pass
/// never wrote `ret_types` back, so `helper` — a native callee whose parameter
/// widens to `Dyn` because its argument is bridge-tainted — kept its
/// pre-marking return type at the call site. Interpolating that result emitted
/// `str.from_i64` on a register pair, which only the Cranelift verifier caught,
/// as an unreadable "Verifier errors". Found by the nightly fresh-seed fuzz
/// (`LK_FUZZ_SEED=30198012768`), reduced here to compile in seconds.
#[test]
fn clif_differential_bridge_taint_reconverges_ret_types() {
    let dir = unique_tmp_dir("bridge_taint");
    let _ = fs::remove_dir_all(&dir);
    create_dir_all(&dir).expect("create tmp dir");
    let file = "taint.lk";
    let src = "fn bridged(x) { let f = \"v={}\".trim(); println(f, x); return [x, x + 1]; }\n\
               fn helper(a, b) { return (b - 25); }\n\
               let got = bridged(1);\n\
               let picked = helper(2, got[0]);\n\
               println(\"p={}\", picked);\n\
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

    let native = Command::new(dir.join("taint"))
        .env("ASAN_OPTIONS", "detect_leaks=0")
        .output()
        .expect("run executable");
    assert_eq!(
        vm_stdout,
        String::from_utf8_lossy(&native.stdout),
        "stdout must match the VM"
    );
    assert_eq!(vm.status.success(), native.status.success());
    let _ = fs::remove_dir_all(&dir);
}

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

/// try/catch equivalence, *without* requiring the native path.
///
/// Renamed off `clif_differential_*` on purpose: `try`/`catch` is now a real
/// statement lowering to `TryBegin`/`TryEnd`, and the MIR lowering has no
/// handler-region support yet, so these cases degrade to the Tier 0 VM bundle.
/// What this test guards is what it always really guarded — that the native
/// artifact behaves exactly like the VM. The property that lapsed (it went
/// *through Cranelift*) is a recorded debt tracked in todos.md and pinned by
/// `AOT_COVERAGE_ALLOW` in check.yml, not something to be silently dropped here.
#[test]
fn try_catch_differential() {
    run_differential(
        "try_catch",
        &[
            // A raise crossing the protected region: the success path runs the
            // body, the failure path binds the raised value.
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
                // `r` is annotated and the literals match `/`'s Float result: a
                // `try` body is now type-checked like any other statement (it used
                // to sit inside a closure the checker did not look into), and
                // `let r = 0; r = div(10, 0);` is a static type error. The path
                // under test — a raise from a nested call, caught, value bound —
                // is unchanged.
                "fn div(a: Int, b: Int) -> Float {\n  if (b == 0) { error(\"zero\"); }\n  return a / b;\n}\nlet r: Float = 0.0;\ntry {\n  r = div(10, 0);\n} catch e {\n  r = -1.0;\n}\nreturn r;\n",
            ),
            // A raised channel error must not leave any lock held across the
            // longjmp: after catching, the registry and channel stay usable
            // (regression: `channel()` raised "Channel not found" while the
            // registry MutexGuard was live, deadlocking every later op).
            new(
                "chan_unknown_id_catch_then_use",
                // The bad id goes through an `Any` binding: a `try` body is now
                // type-checked like any other statement (it used to sit inside a
                // closure the checker did not look into), and `recv(999)` is a
                // static type error. The runtime path under test — an unknown
                // channel id raising, caught, and the channel machinery still
                // usable afterwards — is unchanged.
                "let bad: Any = 999;\ntry { recv(bad); } catch e { println(\"caught\"); }\nlet c = chan(1);\nsend(c, 41);\nprintln(recv(c) + 1);\nreturn 0;\n",
            ),
            // Same discipline on the closed-send raise inside select's arm.
            new(
                "select_closed_send_catch_then_use",
                "use chan as ch;\nlet c = chan(1);\nch.close(c);\ntry {\n  let x = select {\n    case send(c, 1) => \"sent\";\n  };\n  println(x);\n} catch e { println(\"caught\"); }\nlet d = chan(1);\nsend(d, 6);\nprintln(recv(d) * 7);\nreturn 0;\n",
            ),
        ],
        NativePath::MayDegrade,
    );
}

/// `as` casts, with the native path pinned: the point of these is that the two
/// backends agree *bit for bit*, not merely that both produce something.
///
/// The VM masks inside its `i64` carrier and sign-extends back; Cranelift does
/// `ireduce` then `sextend`/`uextend`. Those are different mechanisms, so this
/// is where a divergence would show up.
#[test]
fn machine_int_cast_differential() {
    run_differential(
        "machine_int_cast",
        &[
            // Narrowing truncates rather than erroring: 300 & 0xFF.
            new("narrow_u8", "let x = 300 as u8;\nreturn x;\n"),
            // Sign extension back into the carrier — the case most likely to
            // diverge between a mask and an `ireduce`.
            new("sign_extend_i8", "let x = 255 as i8;\nreturn x;\n"),
            new("sign_extend_i8_min", "let x = 128 as i8;\nreturn x;\n"),
            new("sign_extend_i16", "let x = 65535 as i16;\nreturn x;\n"),
            // Negative source, unsigned target: reinterpretation, not clamping.
            new("negative_to_u32", "let x = (0 - 1) as u32;\nreturn x;\n"),
            new("negative_to_u8", "let x = (0 - 1) as u8;\nreturn x;\n"),
            // The second cast must see the first one's result, not the original.
            new("chained", "let x = 300 as u8 as u32;\nreturn x;\n"),
            // Full width is a no-op on both sides.
            new("identity_i64", "let x = (0 - 1) as i64;\nreturn x;\n"),
            // Pointer width follows the carrier on a 64-bit host.
            new("usize_passthrough", "let x = 42 as usize;\nreturn x;\n"),
        ],
        NativePath::PureCranelift,
    );
}
