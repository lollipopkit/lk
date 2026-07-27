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
            // Float and bool sources: the VM converts them (truncating toward
            // zero, 0/1) before reducing to width, so the native path needs
            // the same conversion rather than only accepting integers.
            new("float_source", "let x = 3.9 as i32;\nreturn x;\n"),
            new("float_source_negative", "let x = (0.0 - 3.9) as i32;\nreturn x;\n"),
            // Out of range: Rust's `as` saturates before the width reduction,
            // on both sides — the case a trapping conversion would abort on.
            new("float_source_saturates", "let x = 1.0e30 as i64;\nreturn x;\n"),
            new("bool_source", "let x = true as u8;\nreturn x;\n"),
            // A source that came out of a container is boxed, so the native
            // path unboxes through `dyn.cast_to_i64` rather than reading a
            // register — a different mechanism from the register case above,
            // and the one an output loop in a driver actually hits.
            new(
                "boxed_source_from_list",
                "let xs = [300, 255];\nlet out = 0 as u8;\nfor x in xs { out = out + (x as u8); }\nreturn out;\n",
            ),
            // The boxed path must truncate a Float toward zero and read a Bool
            // as 0/1, exactly as the VM's `cast_source_to_i64` does — the two
            // cases where an `as_i64`-style unbox would raise instead.
            new(
                "boxed_source_float",
                "let xs = [3.9, 0.0 - 3.9];\nfor x in xs { println(x as i32); }\nreturn 0;\n",
            ),
            new(
                "boxed_source_bool",
                "let xs = [true, false];\nlet out = 0 as u8;\nfor x in xs { out = out + (x as u8); }\nreturn out;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// Machine-int *arithmetic* wraps to its width, on both backends.
///
/// The wrap is emitted as a normalisation after the 64-bit operation, reusing
/// the same cast path — so what this really checks is that every lowering
/// entry point (plain, lower-into-register, compound assignment) applies it.
/// A missing one produces a plainly wrong number rather than a crash, which is
/// why it needs a test rather than an assertion.
#[test]
fn machine_int_arithmetic_wraps_differential() {
    run_differential(
        "machine_int_arith",
        &[
            // 300 & 0xFF
            new("add_u8", "let a: u8 = 200;\nlet b: u8 = 100;\nreturn a + b;\n"),
            // 600 & 0xFF
            new("mul_u8", "let a: u8 = 200;\nreturn a * (3 as u8);\n"),
            // 200 sign-extended from 8 bits
            new(
                "add_i8_overflows_negative",
                "let a: i8 = 100;\nreturn a + (100 as i8);\n",
            ),
            // Borrowing past zero on an unsigned width.
            new("sub_u8_underflows", "let a: u8 = 10;\nreturn a - (20 as u8);\n"),
            // 70000 & 0xFFFF
            new("add_u16", "let a: u16 = 60000;\nreturn a + (10000 as u16);\n"),
            // The wrap has to apply at each step, not just the last one.
            new(
                "chained_arithmetic_wraps_each_step",
                "let a: u8 = 200;\nlet b: u8 = 100;\nlet c = a + b;\nreturn c + b;\n",
            ),
            // Division keeps the width rather than promoting to Float the way
            // `Int / Int` does.
            new("div_keeps_width", "let a: u8 = 200;\nreturn a / (3 as u8);\n"),
        ],
        NativePath::PureCranelift,
    );
}

/// A volatile read must survive optimisation.
///
/// Reading one address twice has to produce two accesses: a device register can
/// return different values on consecutive reads, and reading it can have side
/// effects. This is a *disassembly* test rather than a differential one because
/// the failure is invisible at the value level — with the reads collapsed the
/// program still returns a plausible number, just one derived from a single
/// access. That is exactly how the first implementation passed by inspection
/// and failed here: inline Cranelift loads compiled to one `mov` and a `lea`
/// doubling it, because Cranelift has no volatile flag and its egraph pass
/// proved the two loads equal.
#[test]
fn volatile_reads_are_not_collapsed() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("volatile_twice.lk");
    std::fs::write(
        &source,
        "fn read_twice(addr: usize) -> Int {\n\
         \x20   let reg = addr as *mut u32;\n\
         \x20   let a = unsafe { volatile_read_u32(reg) };\n\
         \x20   let b = unsafe { volatile_read_u32(reg) };\n\
         \x20   return a + b;\n\
         }\n\
         return read_twice(0x1000);\n",
    )
    .expect("write source");

    let exe = dir.path().join("volatile_twice");
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
        .args(["compile", source.to_str().expect("utf-8 path")])
        .arg("--output")
        .arg(exe.to_str().expect("utf-8 path"))
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .status()
        .expect("run lk compile");
    assert!(status.success(), "volatile must lower natively");

    let disassembly = std::process::Command::new("objdump")
        .args(["-d", exe.to_str().expect("utf-8 path")])
        .output();
    let Ok(disassembly) = disassembly else {
        // objdump is not everywhere; the compile above is still meaningful.
        return;
    };
    let text = String::from_utf8_lossy(&disassembly.stdout);
    let body: String = text
        .lines()
        .skip_while(|line| !line.contains("<lk_fn_1>:"))
        .take_while(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let accesses = body.matches("lkrt_mmio_read_u32").count();
    assert_eq!(accesses, 2, "expected two volatile reads, got {accesses}:\n{body}");
}

/// A critical section lowers to the right sequence, in the right order.
///
/// Order is the whole point and it is invisible in the return value: masking
/// interrupts *after* the register write, or dropping the barrier, produces a
/// program that returns the same number and races on real hardware. So this
/// checks the emitted call sequence rather than the result.
#[test]
fn critical_section_emits_its_instructions_in_order() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("critical.lk");
    std::fs::write(
        &source,
        "fn critical(addr: usize) -> Int {\n\
         \x20   let reg = addr as *mut u32;\n\
         \x20   let saved = unsafe { cpu_irq_save() };\n\
         \x20   unsafe { volatile_write_u32(reg, 1 as u32); };\n\
         \x20   unsafe { cpu_barrier(); };\n\
         \x20   let v = unsafe { volatile_read_u32(reg) };\n\
         \x20   unsafe { cpu_irq_restore(saved); };\n\
         \x20   return v;\n\
         }\n\
         return critical(0x1000);\n",
    )
    .expect("write source");

    let exe = dir.path().join("critical");
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
        .args(["compile", source.to_str().expect("utf-8 path")])
        .arg("--output")
        .arg(exe.to_str().expect("utf-8 path"))
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .status()
        .expect("run lk compile");
    assert!(status.success(), "a critical section must lower natively");

    let Ok(disassembly) = std::process::Command::new("objdump")
        .args(["-d", exe.to_str().expect("utf-8 path")])
        .output()
    else {
        return; // objdump is not everywhere; the compile above still ran.
    };
    let text = String::from_utf8_lossy(&disassembly.stdout);
    let body: Vec<&str> = text
        .lines()
        .skip_while(|line| !line.contains("<lk_fn_1>:"))
        .take_while(|line| !line.trim().is_empty())
        .collect();

    let expected = [
        "lkrt_cpu_irq_save",
        "lkrt_mmio_write_u32",
        "lkrt_cpu_barrier",
        "lkrt_mmio_read_u32",
        "lkrt_cpu_irq_restore",
    ];
    let mut remaining = expected.iter();
    let mut wanted = remaining.next();
    for line in &body {
        if let Some(name) = wanted
            && line.contains(name)
        {
            wanted = remaining.next();
        }
    }
    assert!(
        wanted.is_none(),
        "missing or out-of-order: still looking for {wanted:?} in:\n{}",
        body.join("\n")
    );
}

/// Port I/O lowers to opaque `lkrt` calls, and two reads of one port stay two.
///
/// The same reasoning as `volatile_reads_are_not_collapsed`, for a different
/// address space: a UART's status port answers differently on each read, so
/// collapsing a poll loop's read is a hang rather than a wrong number. The ABI
/// marks the reads `WritesHost` to prevent it; this checks that it holds after
/// lowering, not merely that the annotation is present.
#[test]
fn port_reads_are_not_collapsed() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("ports.lk");
    std::fs::write(
        &source,
        "fn poll(port: Int) -> Int {\n\
         \x20   let a = unsafe { port_in_u8(port) };\n\
         \x20   let b = unsafe { port_in_u8(port) };\n\
         \x20   return (a as Int) + (b as Int);\n\
         }\n\
         return poll(0x3f8);\n",
    )
    .expect("write source");

    let exe = dir.path().join("ports");
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
        .args(["compile", source.to_str().expect("utf-8 path")])
        .arg("--output")
        .arg(exe.to_str().expect("utf-8 path"))
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .status()
        .expect("run lk compile");
    assert!(status.success(), "port I/O must lower natively");

    let Ok(disassembly) = std::process::Command::new("objdump")
        .args(["-d", exe.to_str().expect("utf-8 path")])
        .output()
    else {
        // objdump is not everywhere; the compile above is still meaningful.
        return;
    };
    let text = String::from_utf8_lossy(&disassembly.stdout);
    let body: String = text
        .lines()
        .skip_while(|line| !line.contains("<lk_fn_1>:"))
        .take_while(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let accesses = body.matches("lkrt_port_in_u8").count();
    assert_eq!(accesses, 2, "expected two port reads, got {accesses}:\n{body}");
}

/// A file import that carries constants as well as functions.
///
/// The native path bundles imports at compile time, and a bundled module's
/// entry — the only code that would run its top-level assignments — is the one
/// function the merge drops. So its constants are folded into each read
/// instead. That is a rewrite of the program, and the only thing that shows it
/// was faithful is the two backends still agreeing.
#[test]
fn bundled_import_constants_match_the_vm() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("dep.lk"),
        "const BASE = 0x3f8;\n\
         const SCALE = 2.5;\n\
         const LABEL = \"dep\";\n\
         const ON = true;\n\
         fn offset(n: Int) -> Int { return BASE + n; }\n\
         fn scaled(n: Int) -> Float { return n * SCALE; }\n\
         fn label() -> String { return LABEL; }\n\
         fn flag() -> Bool { return ON; }\n",
    )
    .expect("write dep");
    let main = dir.path().join("main.lk");
    std::fs::write(
        &main,
        "use { offset, scaled, label, flag, BASE } from \"dep\";\n\
         println(offset(8));\n\
         println(scaled(4));\n\
         println(label());\n\
         println(flag());\n\
         println(BASE);\n\
         return 0;\n",
    )
    .expect("write main");

    let vm = Command::new(bin_path())
        .current_dir(dir.path())
        .arg("main.lk")
        .output()
        .expect("spawn vm run");
    assert!(
        vm.status.success(),
        "vm run failed: {}",
        String::from_utf8_lossy(&vm.stderr)
    );

    let exe = dir.path().join("main");
    let compile = Command::new(bin_path())
        .current_dir(dir.path())
        .args(["compile", "main.lk"])
        .arg("--output")
        .arg(&exe)
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .output()
        .expect("spawn native compile");
    assert!(
        compile.status.success(),
        "a module of constants and functions must lower natively: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let native = Command::new(&exe).output().expect("spawn compiled executable");

    assert_eq!(
        String::from_utf8_lossy(&vm.stdout),
        String::from_utf8_lossy(&native.stdout),
        "bundled constants diverged between the backends"
    );
}

/// A bundled module may import another file.
///
/// The bundler walks the import graph rather than one level of it, and the
/// lowering resolves a nested module's names — which never appear in the
/// importing file's own import list — through the flattened namespace the
/// merge produces.
#[test]
fn nested_bundled_imports_match_the_vm() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir(dir.path().join("lib")).expect("mkdir lib");
    std::fs::write(
        dir.path().join("lib/bits.lk"),
        "const MASK = 0xff;\nfn low_byte(v: Int) -> Int { return v & MASK; }\n",
    )
    .expect("write bits");
    std::fs::write(
        dir.path().join("lib/dev.lk"),
        "use { low_byte } from \"bits\";\n\
         const BASE = 0x3f8;\n\
         fn reg(offset: Int) -> Int { return low_byte(BASE + offset); }\n",
    )
    .expect("write dev");
    std::fs::write(
        dir.path().join("main.lk"),
        "use { reg } from \"lib/dev\";\n\
         use { low_byte } from \"lib/bits\";\n\
         println(reg(5));\n\
         println(low_byte(0x1234));\n\
         return 0;\n",
    )
    .expect("write main");

    let vm = Command::new(bin_path())
        .current_dir(dir.path())
        .arg("main.lk")
        .output()
        .expect("spawn vm run");
    assert!(
        vm.status.success(),
        "vm run failed: {}",
        String::from_utf8_lossy(&vm.stderr)
    );

    let exe = dir.path().join("main");
    let compile = Command::new(bin_path())
        .current_dir(dir.path())
        .args(["compile", "main.lk"])
        .arg("--output")
        .arg(&exe)
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .output()
        .expect("spawn native compile");
    assert!(
        compile.status.success(),
        "a module importing another module must lower natively: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let native = Command::new(&exe).output().expect("spawn compiled executable");
    assert_eq!(
        String::from_utf8_lossy(&vm.stdout),
        String::from_utf8_lossy(&native.stdout),
        "a nested import diverged between the backends"
    );
}

/// A container at a bundled module's top level is refused, not flattened.
///
/// Bundling merges modules into one, which would *share* the container with
/// the importer; the VM gives each module its own copy. The two answers differ
/// as soon as anything mutates it, so the bundler rejects the shape rather
/// than producing a program that computes something the VM would not.
#[test]
fn bundled_module_container_constants_are_refused() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("table.lk"),
        "const NAMES = [\"zero\", \"one\"];\nfn get() -> List<String> { return NAMES; }\n",
    )
    .expect("write dep");
    std::fs::write(
        dir.path().join("main.lk"),
        "use { get } from \"table\";\nprintln(get().len());\nreturn 0;\n",
    )
    .expect("write main");

    let compile = Command::new(bin_path())
        .current_dir(dir.path())
        .args(["compile", "main.lk"])
        .arg("--output")
        .arg(dir.path().join("main"))
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .output()
        .expect("spawn native compile");
    assert!(
        !compile.status.success(),
        "a shared container must not compile silently"
    );
    let stderr = String::from_utf8_lossy(&compile.stderr);
    assert!(
        stderr.contains("container at its top level"),
        "the refusal should say what is wrong: {stderr}"
    );
}

/// An exported-but-unused function in a bundled module does not fail the build.
///
/// Bundled functions are reached by name, which the bytecode reachability scan
/// cannot follow, so they were all rooted. A module exports more than any one
/// importer uses, and lowering a function nothing calls can fail the whole
/// module for a shape that never runs — its parameter types have no call site
/// to be observed from, so they are not even known.
#[test]
fn an_unused_bundled_function_does_not_fail_the_module() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("lib.lk"),
        // `each` is never called: its list parameter has no observed type.
        "fn used(n: Int) -> Int { return n + 1; }\n\
         fn each(xs: List<Int>) -> Int { let s = 0; for x in xs { s = s + x; } return s; }\n",
    )
    .expect("write dep");
    std::fs::write(
        dir.path().join("main.lk"),
        "use { used } from \"lib\";\nprintln(used(1));\nreturn 0;\n",
    )
    .expect("write main");

    let compile = Command::new(bin_path())
        .current_dir(dir.path())
        .args(["compile", "main.lk"])
        .arg("--output")
        .arg(dir.path().join("main"))
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .output()
        .expect("spawn native compile");
    assert!(
        compile.status.success(),
        "an unused export must not fail the module: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
}

/// A boxed container index lowers, rather than failing the module.
///
/// Iterating a list yields a `Maybe` carrier; passing that as an argument
/// boxes it. So `fn at(xs, i) { return xs[i]; }` called from `for i in idx`
/// sees a `Dyn` index — an ordinary shape that had no lowering, which made a
/// two-function library fail to compile with an error naming neither function.
#[test]
fn a_boxed_container_index_matches_the_vm() {
    run_clif_differential(
        "boxed_index",
        &[
            new(
                "read",
                "fn at(xs: List<Int>, i: Int) -> Int { return xs[i]; }\n\
                 let xs = [10, 20, 30];\n\
                 let idx = [0, 2];\n\
                 let total = 0;\n\
                 for i in idx { total = total + at(xs, i); }\n\
                 return total;\n",
            ),
            new(
                "write",
                "fn put(xs: List<Int>, i: Int, v: Int) { xs[i] = v; }\n\
                 let xs = [0, 0, 0];\n\
                 let idx = [0, 2];\n\
                 for i in idx { put(xs, i, 7); }\n\
                 return xs[0] + xs[2];\n",
            ),
            // A non-integer index is rejected by the type checker before it
            // reaches the lowering, so the unbox only ever sees an integer in
            // a well-typed program. It still goes through the runtime's tag
            // check rather than reading the payload blind, because `Dyn` is
            // also what an untyped path produces.
        ],
    );
}

/// A module that writes through a container parameter is not bundled.
///
/// Bundling flattens the modules together, so the callee would get the
/// caller's container by reference; the VM runs them as separate modules with
/// separate heaps and copies arguments across the boundary (see
/// `copy_runtime_positional_args_to_frame`). The two disagree the moment the
/// callee writes — `xs[0]` reads 0 under the VM and 7 under a flattened build
/// — and nothing reports it. So the bundler declines.
///
/// What is checked here is the refusal and its wording. That the fallback then
/// produces the VM's answer is the Tier 0 path's own guarantee, and exercising
/// it here would drag a cargo build of the embedded runtime into a unit test.
#[test]
fn a_module_that_mutates_a_parameter_is_not_bundled() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("m.lk"),
        "fn put(xs: List<Int>, i: Int, v: Int) { xs[i] = v; }\n",
    )
    .expect("write dep");
    std::fs::write(
        dir.path().join("main.lk"),
        "use { put } from \"m\";\nlet xs = [0, 0, 0];\nput(xs, 0, 7);\nprintln(xs[0]);\nreturn 0;\n",
    )
    .expect("write main");

    // An object build has no fallback to take, so the refusal has to name the
    // cause rather than the unlowerable instruction it would otherwise become.
    let strict = Command::new(bin_path())
        .current_dir(dir.path())
        .args(["compile", "object:x86_64-unknown-none", "main.lk"])
        .arg("--output")
        .arg(dir.path().join("main.o"))
        .output()
        .expect("spawn object compile");
    assert!(!strict.status.success(), "a shared container must not compile silently");
    let stderr = String::from_utf8_lossy(&strict.stderr);
    assert!(
        stderr.contains("container parameter"),
        "the refusal should say what is wrong: {stderr}"
    );
}

/// A module that only *reads* its container parameters still bundles.
///
/// The refusal above has to be narrow, or every library that takes a list
/// stops compiling natively.
#[test]
fn a_module_that_only_reads_a_parameter_still_bundles() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("m.lk"),
        "fn total(xs: List<Int>) -> Int { let s = 0; for x in xs { s = s + x; } return s; }\n\
         fn at(xs: List<Int>, i: Int) -> Int { return xs[i]; }\n\
         fn size(xs: List<Int>) -> Int { return xs.len(); }\n",
    )
    .expect("write dep");
    std::fs::write(
        dir.path().join("main.lk"),
        "use { total, at, size } from \"m\";\n\
         let xs = [1, 2, 3];\n\
         println(total(xs));\n\
         println(at(xs, 1));\n\
         println(size(xs));\n\
         return 0;\n",
    )
    .expect("write main");

    let vm = Command::new(bin_path())
        .current_dir(dir.path())
        .arg("main.lk")
        .output()
        .expect("spawn vm run");
    let exe = dir.path().join("main");
    let compile = Command::new(bin_path())
        .current_dir(dir.path())
        .args(["compile", "main.lk"])
        .arg("--output")
        .arg(&exe)
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .output()
        .expect("spawn native compile");
    assert!(
        compile.status.success(),
        "a read-only module must still bundle: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let native = Command::new(&exe).output().expect("spawn compiled executable");
    assert_eq!(
        String::from_utf8_lossy(&vm.stdout),
        String::from_utf8_lossy(&native.stdout),
        "a read-only bundled module diverged"
    );
}

/// An imported function's signature is visible to the type checker.
///
/// Without it the name is `Any`: a range bound, a condition and a cast all
/// need better than that, so a program that reads perfectly well needs
/// annotations that say nothing — and a call with the wrong number of
/// arguments is not checked at all, surfacing much later from the native
/// lowering as "opcode CallDirect is not natively lowerable", which names
/// neither the call nor the reason.
#[test]
fn an_imported_signature_is_checked() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("lib.lk"),
        "fn add(a: Int, b: Int) -> Int { return a + b; }\nfn count() -> Int { return 3; }\n",
    )
    .expect("write dep");
    // A range bound and a cast, neither of which accepts `Any`.
    std::fs::write(
        dir.path().join("ok.lk"),
        "use { add, count } from \"lib\";\n\
         let total = 0;\n\
         for i in 0..count() { total = total + add(i, 1); }\n\
         return total;\n",
    )
    .expect("write ok");
    std::fs::write(dir.path().join("bad.lk"), "use { add } from \"lib\";\nreturn add(1);\n").expect("write bad");

    let ok = Command::new(bin_path())
        .current_dir(dir.path())
        .args(["check", "ok.lk"])
        .output()
        .expect("spawn check");
    assert!(
        ok.status.success(),
        "an imported signature should make annotations unnecessary: {}",
        String::from_utf8_lossy(&ok.stderr)
    );

    let bad = Command::new(bin_path())
        .current_dir(dir.path())
        .args(["check", "bad.lk"])
        .output()
        .expect("spawn check");
    assert!(
        !bad.status.success(),
        "a wrong-arity call across a module must be caught"
    );
    let stderr = String::from_utf8_lossy(&bad.stderr);
    assert!(
        stderr.contains("arguments"),
        "the error should be about the call, not an opcode: {stderr}"
    );
}

/// An argument's type is checked against an *annotated* parameter.
///
/// The distinction matters more than the check: an unannotated parameter also
/// ends up with a type, because inference gives it one from the body, but that
/// is a derivation rather than a claim. `fn scale(x) { return x * 2.5; }` may
/// settle on `Int` for `x`, and rejecting `scale(4.0)` against it would reject
/// on something the program never said.
#[test]
fn argument_types_are_checked_against_annotations() {
    let dir = tempfile::tempdir().expect("temp dir");
    let check = |name: &str, source: &str| {
        std::fs::write(dir.path().join(name), source).expect("write source");
        Command::new(bin_path())
            .current_dir(dir.path())
            .args(["check", name])
            .output()
            .expect("spawn check")
    };

    let annotated = check(
        "annotated.lk",
        "fn add(a: Int, b: Int) -> Int { return a + b; }\nreturn add(1, \"x\");\n",
    );
    assert!(!annotated.status.success(), "a wrong argument type must be caught");
    let stderr = String::from_utf8_lossy(&annotated.stderr);
    assert!(
        stderr.contains("Argument 2") && stderr.contains("expected Int"),
        "the error should name the position and the types: {stderr}"
    );

    let inferred = check("inferred.lk", "fn scale(x) { return x * 2.5; }\nreturn scale(4.0);\n");
    assert!(
        inferred.status.success(),
        "an inferred parameter type is not a claim to check against: {}",
        String::from_utf8_lossy(&inferred.stderr)
    );

    // A machine-integer parameter takes an integer literal without a cast.
    // They do not convert implicitly — that is what makes `u8 + Int` an error
    // — but a literal has no type of its own to preserve.
    let literal = check(
        "literal.lk",
        "fn port(number: u16) -> Int { return number as Int; }\nreturn port(0x3f8);\n",
    );
    assert!(
        literal.status.success(),
        "an integer literal should reach a machine-int parameter: {}",
        String::from_utf8_lossy(&literal.stderr)
    );
}
