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

/// A case whose program is generated rather than written out — a 400-element
/// literal is not something to paste into a test file. The leak lives as long
/// as the test process, which is what `&'static str` here means anyway.
fn generated(name: &'static str, source: String) -> Case {
    Case {
        name,
        source: Box::leak(source.into_boxed_str()),
    }
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

/// `xs.slice(a, b)` is a **window**, and the two engines have to agree on what
/// that means — not merely on the elements it reports.
///
/// They did not, for a while: the VM returned a view and Cranelift returned a
/// copy of the same elements. Every one of these cases printed the same thing
/// on both engines *except* the one that writes to the source, which is the
/// only one that can tell a view from a copy. That is the shape of divergence a
/// differential corpus exists to catch, so it is pinned here rather than left
/// to whoever next reads both lowerings side by side.
#[test]
fn clif_differential_list_windows() {
    run_clif_differential(
        "windows",
        &[
            new(
                "window_reads",
                "let xs = [3, 1, 4, 1, 5];\n\
                 let w = xs.slice(1, 4);\n\
                 println(w.len());\n\
                 println(w[0]);\n\
                 println(w[-1]);\n\
                 println(w);\n\
                 return w[2];\n",
            ),
            // Out of the window is nil on both sides, never the source's
            // element at that position.
            new(
                "window_out_of_range",
                "let xs = [3, 1, 4, 1, 5];\n\
                 let w = xs.slice(1, 3);\n\
                 println(w[2]);\n\
                 println(w[-3]);\n\
                 return w.is_empty();\n",
            ),
            // `.get(i)` is `[i]` that answers nil rather than failing — same
            // rule for the index, negative included. The dispatch tables in
            // `core_methods` used to say a negative was simply out of range,
            // which no program could observe (the compiler lowers `.get()` to
            // `GetIndex`) and which neither engine did.
            new(
                "get_indexes_like_brackets",
                "let xs = [10, 20, 30, 40];\n\
                 let w = xs.slice(1, 3);\n\
                 println(xs.get(-1));\n\
                 println(xs.get(4));\n\
                 println(w.get(0));\n\
                 println(w.get(-1));\n\
                 return w.get(5);\n",
            ),
            // The one that distinguishes a view from a copy.
            new(
                "window_sees_the_source_change",
                "let xs = [10, 20, 30, 40];\n\
                 let w = xs.slice(1, 3);\n\
                 println(w[0]);\n\
                 xs[1] = 99;\n\
                 println(w[0]);\n\
                 xs.push(50);\n\
                 return w.len();\n",
            ),
            new(
                "window_iterates_and_copies",
                "let xs = [1, 2, 3, 4, 5];\n\
                 let w = xs.slice(1, 4);\n\
                 let sum = 0;\n\
                 for v in w {\n  sum = sum + v;\n}\n\
                 println(sum);\n\
                 println(w.to_list());\n\
                 return w.to_list().len();\n",
            ),
            // A window on a window resolves against the original, and bounds
            // past the end clamp rather than raising.
            new(
                "window_of_a_window",
                "let xs = [0, 1, 2, 3, 4];\n\
                 let w = xs.slice(1, 99);\n\
                 let inner = w.slice(1, 3);\n\
                 println(w.len());\n\
                 println(inner.len());\n\
                 println(inner[0]);\n\
                 return inner[1];\n",
            ),
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
/// A declared width survives a function boundary — and being inlined.
///
/// Every machine-integer rule is chosen from a compile-time fact about a
/// register, and there were two places that fact was never written down: a
/// *parameter* declared `u8`/`u64`/…, and a `let` inside a body the compiler
/// chose to *inline*. Everything held for an annotated local and an `as` cast,
/// which is what every earlier test and every driver that casts on entry
/// happens to use.
///
/// So `fn f(a: u8) -> u8 { return a + 1; }` answered 256, `a * 2` on 200
/// answered 400, and `fn f(a: u64, b: u64) { return a > b; }` compared two
/// addresses as if they were signed. A helper taking a register value is the
/// ordinary shape of driver code.
///
/// Absolute, and it has to be: the fact is missing in the *compiler*, so both
/// backends are handed the same wrong instruction and agree with each other
/// perfectly.
#[test]
fn a_declared_width_crosses_a_function_boundary() {
    let dir = unique_tmp_dir("param_width");
    let _ = fs::remove_dir_all(&dir);
    create_dir_all(&dir).expect("create tmp dir");
    let file = "params.lk";
    // Each helper is small enough to be inlined at the call site *and* is
    // compiled out of line for the module, so both paths are exercised by the
    // same source.
    let src = "const WIDE: u32 = 0xFFFFFFFF;\n\
               fn from_const() -> u32 { return WIDE + 1; }\n\
               // A closure inherited none of these facts either.\n\
               fn in_closure() -> u32 { let f = || WIDE + 1; return f(); }\n\
               fn add_u8(a: u8) -> u8 { return a + 1; }\n\
               fn mul_u8(a: u8) -> u8 { return a * 2; }\n\
               fn sub_u8(a: u8) -> u8 { return a - 1; }\n\
               fn add_i8(a: i8) -> i8 { return a + 1; }\n\
               fn shr_u64(a: u64) -> u64 { return a >> 32; }\n\
               fn gt_u64(a: u64, b: u64) -> Bool { return a > b; }\n\
               fn half_u64(a: u64) -> u64 { return a / 2; }\n\
               // A `let` inside the body, which is the half that only breaks\n\
               // once the function is inlined.\n\
               fn neg_u32(bits: u32) -> u32 { let zero: u32 = 0; return zero - bits; }\n\
               println(add_u8(255 as u8));\n\
               println(mul_u8(200 as u8));\n\
               println(sub_u8(0 as u8));\n\
               println(add_i8(127 as i8));\n\
               println(shr_u64(0xFFFF800000000000 as u64));\n\
               println(gt_u64(0x8000000000000000 as u64, 1 as u64));\n\
               println(half_u64(0xFFFFFFFFFFFFFFFF as u64));\n\
               println(neg_u32(0xFFFFFF80 as u32));\n\
               // A declared field width, which is the same fact one more hop\n\
               // away: the register a field lands in came out of a container\n\
               // and carries nothing of its own.\n\
               struct Reg { value: u32 }\n\
               let r = Reg { value: 0xFFFFFFFF as u32 };\n\
               println(r.value + 1);\n\
               println(r.value / 2);\n\
               // A top-level `const`, which is what a driver's register map is\n\
               // made of — `drivers/e1000.lk` has seventeen — read through\n\
               // `GetGlobal` into a register that carries nothing.\n\
               println(WIDE + 1);\n\
               println(from_const());\n\
               println(in_closure());\n";
    let expected = concat!(
        "0\n144\n255\n-128\n",
        "4294934528\ntrue\n9223372036854775807\n128\n",
        "0\n2147483647\n",
        "0\n0\n0\n",
    );
    File::create(dir.join(file))
        .and_then(|mut f| f.write_all(src.as_bytes()))
        .expect("write program");

    let vm = run_cli(&dir, [file]).env("LK_FORCE_VM", "1").output().expect("vm run");
    assert_eq!(
        String::from_utf8_lossy(&vm.stdout),
        expected,
        "vm stderr: {}",
        String::from_utf8_lossy(&vm.stderr)
    );

    let compile = run_cli(&dir, ["compile", file])
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .output()
        .expect("native compile");
    assert!(
        compile.status.success(),
        "native compile failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let native = Command::new(dir.join("params"))
        .env("ASAN_OPTIONS", "detect_leaks=0")
        .output()
        .expect("run executable");
    assert_eq!(
        String::from_utf8_lossy(&native.stdout),
        expected,
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Sizing a PCI BAR: two's complement at the register's own width.
///
/// `drivers/pci.lk` asks a device how large its region is the only way the bus
/// allows — write all ones, read back, and the lowest bit the device still
/// leaves set is the size. Turning that into a number is `-bits` at 32 bits, or
/// equivalently `~bits + 1`, and both forms are asserted here because both are
/// what someone writes and they must not disagree.
///
/// What makes it a test rather than a tautology is the width. On the `i64`
/// carrier every one of these values has 32 high zero bits that the register
/// never had, so a complement or a negation that runs at 64 bits answers
/// something with no relation to a BAR size. The four cases are real region
/// sizes, and the expected values were computed elsewhere.
#[test]
fn a_pci_bar_sizes_at_its_own_width() {
    let dir = unique_tmp_dir("bar_size");
    let _ = fs::remove_dir_all(&dir);
    create_dir_all(&dir).expect("create tmp dir");
    let file = "bar.lk";
    let src = "fn size_of(probed: u32, mask: u32) -> u32 {\n\
               \x20   let bits = probed & mask;\n\
               \x20   let zero: u32 = 0;\n\
               \x20   return zero - bits;\n\
               }\n\
               fn size_by_complement(probed: u32, mask: u32) -> u32 {\n\
               \x20   let bits = probed & mask;\n\
               \x20   return (~bits) + 1;\n\
               }\n\
               let mem: u32 = 0xFFFFFFF0;\n\
               let io: u32 = 0xFFFFFFFC;\n\
               println(size_of(0xFFFFFF80 as u32, mem));\n\
               println(size_of(0xFFFFF000 as u32, mem));\n\
               println(size_of(0xFFFFFFE1 as u32, io));\n\
               println(size_of(0xF0000000 as u32, mem));\n\
               println(size_by_complement(0xFFFFFF80 as u32, mem) == size_of(0xFFFFFF80 as u32, mem));\n\
               println(size_by_complement(0xF0000000 as u32, mem) == size_of(0xF0000000 as u32, mem));\n";
    let expected = "128\n4096\n32\n268435456\ntrue\ntrue\n";
    File::create(dir.join(file))
        .and_then(|mut f| f.write_all(src.as_bytes()))
        .expect("write program");

    let vm = run_cli(&dir, [file]).env("LK_FORCE_VM", "1").output().expect("vm run");
    assert_eq!(
        String::from_utf8_lossy(&vm.stdout),
        expected,
        "vm stderr: {}",
        String::from_utf8_lossy(&vm.stderr)
    );

    let compile = run_cli(&dir, ["compile", file])
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .output()
        .expect("native compile");
    assert!(
        compile.status.success(),
        "native compile failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let native = Command::new(dir.join("bar"))
        .env("ASAN_OPTIONS", "detect_leaks=0")
        .output()
        .expect("run executable");
    assert_eq!(
        String::from_utf8_lossy(&native.stdout),
        expected,
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A hardware descriptor, packed and unpacked, with the answers computed
/// elsewhere.
///
/// This is `drivers/idt.lk`'s `set_gate` in miniature: a 64-bit handler address
/// split across three fields that are not adjacent, the two words written, and
/// the address rebuilt from them. Nothing else in the suite exercises that
/// shape, and the only thing that currently notices a mistake in it is a QEMU
/// run that triple-faults — which says the machine died, not which shift was
/// wrong.
///
/// The address has bit 63 set, which is what a higher-half kernel's handler
/// looks like and what makes the shifts *mean* something: `handler >> 32` is a
/// logical shift because the value is a `u64`, and the same expression on an
/// `Int` carrier would sign-extend. The two masked fields would survive that —
/// the mask hides it — so the unmasked `>> 32` is here as well, which is the
/// shape a driver writes to take the high half of a 64-bit BAR.
///
/// Expected values computed independently (Python, arbitrary-precision), not
/// read back from this implementation.
#[test]
fn a_gate_descriptor_packs_and_unpacks() {
    let dir = unique_tmp_dir("gate_pack");
    let _ = fs::remove_dir_all(&dir);
    create_dir_all(&dir).expect("create tmp dir");
    let file = "gate.lk";
    let src = "let handler: u64 = 0xFFFF800012345678;\n\
               let selector: u64 = 0x08;\n\
               let dpl: u64 = 0;\n\
               let kind: u64 = 0x8E;\n\
               let low = (handler & 0xFFFF)\n\
               \x20   | (selector << 16)\n\
               \x20   | ((kind | (dpl << 5)) << 40)\n\
               \x20   | (((handler >> 16) & 0xFFFF) << 48);\n\
               let high = (handler >> 32) & 0xFFFFFFFF;\n\
               println(low);\n\
               println(high);\n\
               let rebuilt = (low & 0xFFFF) | (((low >> 48) & 0xFFFF) << 16) | (high << 32);\n\
               println(rebuilt);\n\
               println(rebuilt == handler);\n\
               println(handler >> 32);\n";
    let expected = "1311829522123347576\n4294934528\n18446603336526616184\ntrue\n4294934528\n";
    File::create(dir.join(file))
        .and_then(|mut f| f.write_all(src.as_bytes()))
        .expect("write program");

    let vm = run_cli(&dir, [file]).env("LK_FORCE_VM", "1").output().expect("vm run");
    assert_eq!(
        String::from_utf8_lossy(&vm.stdout),
        expected,
        "vm stderr: {}",
        String::from_utf8_lossy(&vm.stderr)
    );

    let compile = run_cli(&dir, ["compile", file])
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .output()
        .expect("native compile");
    assert!(
        compile.status.success(),
        "native compile failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let native = Command::new(dir.join("gate"))
        .env("ASAN_OPTIONS", "detect_leaks=0")
        .output()
        .expect("run executable");
    assert_eq!(
        String::from_utf8_lossy(&native.stdout),
        expected,
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A `Maybe` reaching one call site widens the parameter for all of them.
///
/// `Sig::observe_param` records a nullable argument as `Dyn`, and the join is
/// per *parameter* — so one call passing `s.byte_at(i)` makes the parameter
/// `Dyn` for the call that passes `10` as well. Everything inside the callee
/// then has a boxed operand where it wants a number.
///
/// That is correct and it used to be free, because `byte_at` answered an `I64`.
/// It began answering a `Maybe` — honestly: an index past the end is nil — and
/// `bare-metal-x86` stopped lowering, on `put_char`, a function that never
/// touches a string. What fixes it is unboxing where a scalar is *required*,
/// which is the rule `read_index_scalar` already documented.
///
/// Absolute rather than differential: both engines agree on the answer either
/// way. What differs is whether the native build exists at all.
#[test]
fn a_boxed_argument_still_lowers_where_a_number_is_required() {
    let dir = unique_tmp_dir("boxed_param");
    let _ = fs::remove_dir_all(&dir);
    create_dir_all(&dir).expect("create tmp dir");
    let file = "boxed.lk";
    // `sink` is called with a `Maybe` and with a plain `Int`, which is what
    // makes its parameter `Dyn`; the body then does arithmetic, a comparison
    // and a shift on it — three separate `read_typed_scalar` consumers.
    //
    // The parameter is declared `Int?` because that is what `byte_at` answers,
    // and an `Int?` argument no longer passes for a declared `Int` (nullability
    // used to be erased by the numeric-promotion rule). The lowering under test
    // is unchanged: the parameter is still `Dyn` at both call sites.
    let src = "fn sink(b: Int?) -> Int {\n\
               \x20   if (b == 8) {\n\
               \x20       return 0;\n\
               \x20   }\n\
               \x20   return (b + 1) >> 1;\n\
               }\n\
               fn walk(text: String) -> Int {\n\
               \x20   let total = 0;\n\
               \x20   for i in 0..text.len() {\n\
               \x20       total = total + sink(text.byte_at(i));\n\
               \x20   }\n\
               \x20   return total + sink(10);\n\
               }\n\
               println(walk(\"hi\"));\n";
    File::create(dir.join(file))
        .and_then(|mut f| f.write_all(src.as_bytes()))
        .expect("write program");

    let vm = run_cli(&dir, [file]).env("LK_FORCE_VM", "1").output().expect("vm run");
    let expected = String::from_utf8_lossy(&vm.stdout).into_owned();
    assert!(
        !expected.trim().is_empty(),
        "the VM printed nothing: {}",
        String::from_utf8_lossy(&vm.stderr)
    );

    let compile = run_cli(&dir, ["compile", file])
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .output()
        .expect("native compile");
    assert!(
        compile.status.success(),
        "a boxed parameter must still lower: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let native = Command::new(dir.join("boxed"))
        .env("ASAN_OPTIONS", "detect_leaks=0")
        .output()
        .expect("run executable");
    assert_eq!(String::from_utf8_lossy(&native.stdout), expected);
    let _ = fs::remove_dir_all(&dir);
}

/// The whole machine-integer matrix, answers written out.
///
/// Six rounds of work went into this family one operator at a time — shift,
/// compare, divide, modulo, `as Float`, complement, display — each found by a
/// program that gave a wrong answer rather than by a test. This is the net
/// underneath all of it: every width, signed and unsigned, at the edge where it
/// wraps. Absolute, because a compiler-level mistake is made once and both
/// backends inherit it.
#[test]
fn machine_integer_edges_answer_the_same_on_both_engines() {
    let dir = unique_tmp_dir("int_matrix");
    let _ = fs::remove_dir_all(&dir);
    create_dir_all(&dir).expect("create tmp dir");
    let file = "matrix.lk";
    // `0 - 1` rather than `-1` where a negative is wanted: this language has no
    // unary minus, which is also why a negative literal cannot be told from a
    // wide bit pattern by shape.
    let src = "let s8: i8 = 127;\n\
               println(s8 + 1);\n\
               let s8min: i8 = 0 - 128;\n\
               println(s8min - 1);\n\
               let s16: i16 = 32767;\n\
               println(s16 + 1);\n\
               let s32: i32 = 2147483647;\n\
               println(s32 + 1);\n\
               let neg: i8 = 0 - 1;\n\
               println(neg >> 1);\n\
               let u8max: u8 = 255;\n\
               println(u8max + 1);\n\
               let u8zero: u8 = 0;\n\
               println(u8zero - 1);\n\
               let u16max: u16 = 65535;\n\
               println(u16max + 1);\n\
               let u32max: u32 = 4294967295;\n\
               println(u32max + 1);\n\
               let u8big: u8 = 200;\n\
               println(u8big * 2);\n\
               let top32: u32 = 0x80000000;\n\
               println(top32 >> 31);\n\
               println(top32 / 2);\n\
               let all64: u64 = 0xFFFFFFFFFFFFFFFF;\n\
               println(all64 / 2);\n\
               println(all64 % 10);\n\
               let top64: u64 = 0x8000000000000000;\n\
               let one64: u64 = 1;\n\
               println(top64 > one64);\n\
               println(one64 < top64);\n\
               println((top64 as u32) as Int);\n\
               println((all64 as u8) as Int);\n\
               println(~top32 as Int);\n";
    let expected = concat!(
        "-128\n127\n-32768\n-2147483648\n-1\n",
        "0\n255\n0\n0\n144\n",
        "1\n1073741824\n9223372036854775807\n5\n",
        "true\ntrue\n0\n255\n2147483647\n",
    );
    File::create(dir.join(file))
        .and_then(|mut f| f.write_all(src.as_bytes()))
        .expect("write program");

    let vm = run_cli(&dir, [file]).env("LK_FORCE_VM", "1").output().expect("vm run");
    assert_eq!(
        String::from_utf8_lossy(&vm.stdout),
        expected,
        "vm stderr: {}",
        String::from_utf8_lossy(&vm.stderr)
    );

    let compile = run_cli(&dir, ["compile", file])
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .output()
        .expect("native compile");
    assert!(
        compile.status.success(),
        "native compile failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let native = Command::new(dir.join("matrix"))
        .env("ASAN_OPTIONS", "detect_leaks=0")
        .output()
        .expect("run executable");
    assert_eq!(
        String::from_utf8_lossy(&native.stdout),
        expected,
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A 64-bit mask can be *written*, and still cannot be written where it does not
/// belong.
///
/// `0x8000_0000_0000_0000` used to be refused with "literal
/// -9223372036854775808 is out of range for u64" — a number nobody typed, from
/// the carrier having run out of room. It is a bit pattern at that radix, so it
/// is now parsed as the `u64` it is. The second half of the test is the reason
/// this could not simply relax the range check: `-1` has the same carrier as
/// `0xFFFF_FFFF_FFFF_FFFF` and must stay refused.
#[test]
fn full_width_radix_literals_are_writable() {
    let dir = unique_tmp_dir("wide_literal");
    let _ = fs::remove_dir_all(&dir);
    create_dir_all(&dir).expect("create tmp dir");
    let file = "wide.lk";
    let src = "const PAGE_NX: u64 = 0x8000000000000000;\n\
               const ALL_ONES: u64 = 0xFFFFFFFFFFFFFFFF;\n\
               println(PAGE_NX);\n\
               println(ALL_ONES);\n\
               let mask: u64 = 0xFFFF000000000000;\n\
               println(PAGE_NX & mask);\n\
               let which = match ALL_ONES { 0xFFFFFFFFFFFFFFFF => 1, _ => 0 };\n\
               println(which);\n\
               let addr: usize = 0xFFFFFFFFFFFFFFFF;\n\
               println(addr);\n\
               let half: usize = 0x8000000000000000;\n\
               println(half / 2);\n";
    // `usize` too, and that is not a freebie: the range check used to probe
    // `u32` for pointer widths — "assume the smaller" — so the identical value
    // passed as `u64` and was refused as `usize`.
    let expected = "9223372036854775808\n18446744073709551615\n9223372036854775808\n1\n\
                    18446744073709551615\n4611686018427387904\n";
    File::create(dir.join(file))
        .and_then(|mut f| f.write_all(src.as_bytes()))
        .expect("write program");

    let vm = run_cli(&dir, [file]).env("LK_FORCE_VM", "1").output().expect("vm run");
    assert_eq!(
        String::from_utf8_lossy(&vm.stdout),
        expected,
        "vm stderr: {}",
        String::from_utf8_lossy(&vm.stderr)
    );

    let compile = run_cli(&dir, ["compile", file])
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .output()
        .expect("native compile");
    assert!(
        compile.status.success(),
        "native compile failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let native = Command::new(dir.join("wide"))
        .env("ASAN_OPTIONS", "detect_leaks=0")
        .output()
        .expect("run executable");
    assert_eq!(String::from_utf8_lossy(&native.stdout), expected);

    // Still refused, and for the widths a bit pattern genuinely does not fit.
    for (name, source, wanted) in [
        ("neg.lk", "let y: u8 = -1;\n", "out of range"),
        (
            "wide_u32.lk",
            "let y: u32 = 0xFFFFFFFFFFFFFFFF;\n",
            "out of range for u32",
        ),
        // Pointer width being 64 bits does not make it signless.
        ("neg_usize.lk", "let y: usize = 0 - 1;\n", "out of range for usize"),
    ] {
        File::create(dir.join(name))
            .and_then(|mut f| f.write_all(source.as_bytes()))
            .expect("write program");
        let out = run_cli(&dir, ["check", name]).output().expect("check run");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!out.status.success(), "[{name}] should have been refused: {text}");
        assert!(text.contains(wanted), "[{name}] wanted {wanted:?}, got: {text}");
    }
    let _ = fs::remove_dir_all(&dir);
}

/// What a `u64` above `i64::MAX` prints, absolutely — not just identically.
///
/// A differential cannot see this one: two backends that both hand the carrier
/// to an `i64` formatter agree with each other perfectly, and print a physical
/// address as a negative number. So the expected digits are written out, and
/// both engines are held to them.
#[test]
fn u64_renders_unsigned_on_both_engines() {
    let dir = unique_tmp_dir("u64_render");
    let _ = fs::remove_dir_all(&dir);
    create_dir_all(&dir).expect("create tmp dir");
    let file = "u64render.lk";
    // `one << 63` rather than the literal: `9223372036854775808` does not fit an
    // `i64`, and the lexer has no way yet to tell a `u64` literal from an
    // overflowing one (`let y: u8 = -1` has to stay refused).
    let src = "let one: u64 = 1;\n\
               let top = one << 63;\n\
               println(top);\n\
               println(\"${top}\");\n\
               println(top + 5);\n\
               let narrow: u32 = 4294967295;\n\
               println(narrow);\n\
               println((top + 2) >> 1);\n";
    // The last line is the reason arithmetic has to carry the width at all: the
    // shift asks its left operand how wide it is, and a bare `a + b` used to
    // answer "no idea" — so it shifted arithmetically and produced a *wrong
    // value*, not merely a wrong rendering.
    let expected = "9223372036854775808\n9223372036854775808\n9223372036854775813\n4294967295\n\
                    4611686018427387905\n";
    File::create(dir.join(file))
        .and_then(|mut f| f.write_all(src.as_bytes()))
        .expect("write program");

    let vm = run_cli(&dir, [file]).env("LK_FORCE_VM", "1").output().expect("vm run");
    assert_eq!(
        String::from_utf8_lossy(&vm.stdout),
        expected,
        "vm stderr: {}",
        String::from_utf8_lossy(&vm.stderr)
    );

    let compile = run_cli(&dir, ["compile", file])
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .output()
        .expect("native compile");
    assert!(
        compile.status.success(),
        "native compile failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let native = Command::new(dir.join("u64render"))
        .env("ASAN_OPTIONS", "detect_leaks=0")
        .output()
        .expect("run executable");
    assert_eq!(
        String::from_utf8_lossy(&native.stdout),
        expected,
        "native stderr: {}",
        String::from_utf8_lossy(&native.stderr)
    );
    let _ = fs::remove_dir_all(&dir);
}

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
/// A container a top-level `let` holds and functions mutate.
///
/// This is the shape that miscompiled, and it did so in the way that is worst:
/// silently. A `List<i64>` boxed into a `Dyn` global is *re-represented* — the
/// two are different memory — so `list_h.i64_to_dyn` built a second container,
/// the global held that one, and the entry went on reading the first. Both
/// backends ran, neither complained, and they printed different numbers.
///
/// A container global keeps its own type now, so the slot holds the handle and
/// there is one list. Where the slot cannot stay typed the lowering refuses, so
/// the program falls back and is still right — which is why these cases are
/// checked against the VM rather than against a lowering outcome.
#[test]
fn global_container_differential() {
    run_differential(
        "global_container",
        &[
            // The reproduction, at its smallest.
            new(
                "list_pushed_from_a_function",
                "let xs: List<Int> = [];\nfn add() { xs.push(1); }\nadd();\nadd();\nreturn xs.len();\n",
            ),
            // Non-empty, which is where the two views were both visible: the
            // native build saw the initial element and none of the pushes.
            new(
                "list_starts_non_empty",
                "let xs: List<Int> = [9];\nfn add() { xs.push(1); }\nadd();\nreturn xs.len();\n",
            ),
            // No annotation, so the element type comes from the pushes.
            new(
                "list_without_an_annotation",
                "let xs = [];\nfn add() { xs.push(1); }\nadd();\nreturn xs.len();\n",
            ),
            // A map, which has the same handle-versus-copy question.
            new(
                "map_written_from_a_function",
                "let m: Map<String, Int> = {};\nfn put(k: String) { m[k] = 1; }\nput(\"a\");\nput(\"b\");\nreturn m.len();\n",
            ),
            // A container the *entry* owns, handed to two sibling functions that
            // both pass it on to a third.
            //
            // Not a global at all, and that plurality is the point. One caller
            // passing a container down does not reproduce anything; two callers
            // of the same mutator do, because the container's type is settled
            // from whichever call the signature fixpoint looked at first. This
            // shape printed an empty list — natively, with no fallback and no
            // warning — under an attempted lowering change, and it took
            // bisecting `examples/syntax/defer.lk` to find it. `defer` had
            // nothing to do with it; the example was just the first program
            // with two siblings in it.
            new(
                "two_siblings_share_the_entrys_container",
                "fn note(xs: List<Int>, n: Int) -> Int { xs.push(n); return n; }\n                 fn first(xs: List<Int>, which: Int) -> Int {\n  if (which == 1) { note(xs, 91); return 0 - 1; }\n  note(xs, 92);\n  return 33;\n}\n                 fn second(xs: List<Int>) -> Int {\n  let r = note(xs, 3);\n  note(xs, 4);\n  return r;\n}\n                 let xs: List<Int> = [];\nfirst(xs, 1);\nsecond(xs);\nreturn xs.len();\n",
            ),
            // Read back through the function too, so a build where the two
            // views are swapped fails as loudly as one where they are split.
            new(
                "written_and_read_through_the_function",
                "let xs: List<Int> = [];\nfn add() { xs.push(7); }\nfn total() -> Int {\n  let sum = 0;\n  for i in 0..xs.len() { sum = sum + (xs[i] as Int); }\n  return sum;\n}\nadd();\nadd();\nreturn total();\n",
            ),
        ],
        // Fallback allowed: a slot the lowering cannot keep typed refuses, and
        // the answer still has to be the VM's. That is the guarantee — not that
        // every one of these lowers.
        NativePath::MayDegrade,
    );
}

/// Machine integers, which is what a driver's arithmetic is made of.
///
/// There was no differential coverage for these at all, which is a gap worth
/// closing on its own: a `u32` register write has to be exactly 32 bits and has
/// to *wrap* rather than promote, and both backends have to agree about that or
/// a driver computes a different value depending on how it was built.
///
/// The wrapping cases are the point. `255u8 + 1u8` is `0`, not `256` — the width
/// decides, not the arithmetic — and the same for `u16` at 65535 and for a `u32`
/// at the top of its range. A build that promoted to `Int` somewhere would pass
/// every non-wrapping case here and fail these.
#[test]
fn machine_int_differential() {
    run_differential(
        "machine_int",
        &[
            new(
                "bitwise_or",
                "let a: u8 = 0x0f;\nlet b: u8 = 0xf0;\nreturn (a | b) as Int;\n",
            ),
            new(
                "bitwise_and",
                "let a: u16 = 0xff0f;\nlet b: u16 = 0x0ff0;\nreturn (a & b) as Int;\n",
            ),
            new(
                "shift_right",
                "let a: u16 = 0x1234;\nlet b: u16 = 8;\nreturn ((a >> b) & 0xff) as Int;\n",
            ),
            new(
                "shift_left",
                "let a: u8 = 0x0f;\nlet b: u8 = 1;\nreturn (a << b) as Int;\n",
            ),
            // The width decides, not the arithmetic.
            new("u8_wraps", "let a: u8 = 255;\nlet b: u8 = 1;\nreturn (a + b) as Int;\n"),
            new(
                "u16_wraps",
                "let a: u16 = 65535;\nlet b: u16 = 1;\nreturn (a + b) as Int;\n",
            ),
            new(
                "u32_wraps",
                "let a: u32 = 4294967295;\nlet b: u32 = 2;\nreturn (a + b) as Int;\n",
            ),
            // Subtraction under zero wraps the same way, which is how a driver
            // computing a ring index one short of the base finds out.
            new(
                "u8_wraps_down",
                "let a: u8 = 0;\nlet b: u8 = 1;\nreturn (a - b) as Int;\n",
            ),
            // Multiplication past the width, which is where a promotion to
            // `Int` would be least visible: the low bits are still right.
            new(
                "u8_multiplies",
                "let a: u8 = 200;\nlet b: u8 = 3;\nreturn (a * b) as Int;\n",
            ),
            // A literal beside a machine integer takes its width, and wraps at
            // it. This is the shape driver code is made of — `reg + 1`,
            // `count - 1`, `mask << 1` — and it took two halves: the checker
            // accepting it, and the compiler normalising the literal to the
            // width *before* the operation. With only the first, `255u8 + 1`
            // answered 256 while the type said `u8`.
            new("literal_wraps_up", "let a: u8 = 255;\nreturn (a + 1) as Int;\n"),
            new("literal_wraps_down", "let a: u8 = 0;\nreturn (a - 1) as Int;\n"),
            new("literal_on_the_left", "let a: u8 = 255;\nreturn (1 + a) as Int;\n"),
            new("literal_multiplies", "let a: u8 = 200;\nreturn (a * 3) as Int;\n"),
            new(
                "literal_in_a_wider_width",
                "let a: u32 = 4294967295;\nreturn (a + 2) as Int;\n",
            ),
            // `>>` on a `u64` is a *logical* shift, and this is the one width
            // where that is not automatic.
            //
            // Every value rides an `i64` carrier, so for a `u8`, `u16` or `u32`
            // the high bits are zero and an arithmetic shift has no sign to
            // replicate — it happens to be right. A `u64` fills the carrier: bit
            // 63 *is* the sign bit, and `(1u64 << 63) >> 63` answered -1 instead
            // of 1, silently, on both backends. That value is a physical
            // address, a page-table entry, the high half of a 64-bit BAR.
            new(
                "u64_shifts_logically",
                "let one: u64 = 1;\nlet top = one << 63;\nreturn (top >> 63) as Int;\n",
            ),
            new(
                "u64_shifts_logically_partway",
                "let one: u64 = 1;\nlet top = one << 63;\nreturn (top >> 32) as Int;\n",
            ),
            // The width has to survive the `<<` for the `>>` to know: until it
            // did, there was nothing left to consult by the time the second
            // shift was lowered.
            new(
                "width_survives_a_shift",
                "let one: u32 = 1;\nlet top = one << 31;\nreturn (top >> 31) as Int;\n",
            ),
            // `u64` compares and divides unsigned, and this needed the rewrite
            // to reach *three* lowering paths: a comparison producing a value, a
            // comparison feeding a call argument, and a condition. Each was
            // found by a case the previous fix left failing.
            new(
                "u64_compares_unsigned",
                "let one: u64 = 1;\nlet top = one << 63;\nif (top > one) { return 1; }\nreturn 0;\n",
            ),
            new(
                "u64_compares_unsigned_as_a_value",
                "let one: u64 = 1;\nlet top = one << 63;\nif (top < one) { return 1; }\nreturn 0;\n",
            ),
            new(
                "u64_divides_unsigned",
                "let one: u64 = 1;\nlet top = one << 63;\nlet two: u64 = 2;\nreturn (top / two) as Int;\n",
            ),
            new(
                "u64_halves_to_one",
                "let one: u64 = 1;\nlet n = one << 63;\nlet steps = 0;\nwhile (n > one) { n = n / (one + one); steps = steps + 1; }\nreturn steps;\n",
            ),
            // A literal beside a `u64` joins the *unsigned* operation.
            //
            // This is where two correct features composed into a wrong answer.
            // The checker gives a literal the width of the operand beside it, so
            // `top / 2` type-checks as a `u64` division — and the compiler asked
            // for two *proven* operands before choosing the unsigned form, which
            // a literal never is. It divided signed and answered a negative.
            new(
                "u64_divides_a_literal_unsigned",
                "let one: u64 = 1;\nlet top = one << 63;\nreturn (top / 2) as Int;\n",
            ),
            new(
                "u64_mods_a_literal_unsigned",
                "let one: u64 = 1;\nlet top = one << 63;\nreturn (top % 3) as Int;\n",
            ),
            new(
                "u64_compares_a_literal_unsigned",
                "let one: u64 = 1;\nlet top = one << 63;\nif (top > 5) { return 1; }\nreturn 0;\n",
            ),
            // `reg > 0` and `count < 8` are what driver code is made of, at every
            // width.
            new(
                "u32_compares_a_literal",
                "let a: u32 = 7;\nif (a > 3) { return 1; }\nreturn 0;\n",
            ),
            new(
                "u8_compares_a_literal",
                "let a: u8 = 0;\nif (a > 0) { return 1; }\nreturn 0;\n",
            ),
            // `u64 as Float` reads the carrier as unsigned. The last conversion
            // in this family, and the one whose result does not *look* wrong
            // until it is compared with zero.
            new(
                "u64_converts_to_float_unsigned",
                "let one: u64 = 1;\nlet top = one << 63;\nif ((top as Float) > 0.0) { return 1; }\nreturn 0;\n",
            ),
            new(
                "i64_converts_to_float_signed",
                "let a = 0 - 8;\nif ((a as Float) < 0.0) { return 1; }\nreturn 0;\n",
            ),
            // A mask keeps its width, and so does a reassignment.
            //
            // Both were found by converting a PCI driver to compute a BAR's size
            // at the register's own width. `mask = 0xfffffffc;` on a `u32` was
            // refused — the literal-takes-the-width rule reached `let`, addition
            // and comparison but not assignment — and `probed & mask` came back
            // as `Any`, because `&` desugars to a call whose result the checker
            // did not type. Every piece around it checked; the whole did not.
            //
            // The size itself is the two's complement of the probed bits. In
            // `Int` that had to be spelled `((0xffffffff - bits) + 1) & 0xffffffff`:
            // a subtraction standing in for a complement and a mask standing in
            // for the wrap. At the register's width it is a subtraction from
            // zero.
            new(
                "bar_size_at_the_registers_width",
                "fn size(probed_raw: Int, io: Int) -> Int {\n                 \x20 let mask: u32 = 0xfffffff0;\n                 \x20 if (io == 1) { mask = 0xfffffffc; }\n                 \x20 let probed = probed_raw as u32;\n                 \x20 let bits = probed & mask;\n                 \x20 if (bits == 0) { return 0; }\n                 \x20 let zero: u32 = 0;\n                 \x20 return (zero - bits) as Int;\n                 }\n                 return size(0xfff00000, 0) + size(0xffffff00, 0);\n",
            ),
            new(
                "bitwise_keeps_the_width",
                "let a: u32 = 0xf0f0f0f0;\nlet b = a & 0xffff;\nlet c = b | 0x10000;\nreturn (c + 1) as Int;\n",
            ),
            // `~x` on a machine integer is *that width's* complement.
            //
            // Every value rides an `i64` carrier, so complementing a `u32` set
            // the 32 bits above it too: `~(0xff as u32)` answered
            // `0xFFFFFFFFFFFFFF00`, which reads back as -256. It went unnoticed
            // because the shape people write is `a & ~b`, where the `&` masks
            // the strays away — and the one that does not, `~mask` on its own,
            // is exactly what a driver writes to clear a field.
            new(
                "complement_wraps_to_the_width",
                "let a: u8 = 0x0f;\nreturn (~a) as Int;\n",
            ),
            new("complement_u32", "let a: u32 = 0xff;\nreturn (~a) as Int;\n"),
            new(
                "complement_clears_a_bit",
                "let flags: u32 = 0xff;\nlet bit: u32 = 0x80;\nreturn (flags & ~bit) as Int;\n",
            ),
            // `~x` on a machine integer is *that width's* complement.
            //
            // Every value rides an `i64` carrier, so complementing a `u32` set
            // the 32 bits above it too: `~(0xff as u32)` answered
            // `0xFFFFFFFFFFFFFF00`, which reads back as -256. It stayed
            // unnoticed because the shape people write is `a & ~b`, where the
            // `&` masks the strays away — and the one that does not, `~mask` on
            // its own, is exactly what a driver writes to clear a field.
            new(
                "complement_wraps_to_the_width",
                "let a: u8 = 0x0f;\nreturn (~a) as Int;\n",
            ),
            new("complement_u32", "let a: u32 = 0xff;\nreturn (~a) as Int;\n"),
            new(
                "complement_clears_a_bit",
                "let flags: u32 = 0xff;\nlet bit: u32 = 0x80;\nreturn (flags & ~bit) as Int;\n",
            ),
            new("complement_u64", "let one: u64 = 1;\nreturn ((~one) >> 32) as Int;\n"),
            // A signed comparison is still signed, which is the property the
            // change must not have taken away.
            new(
                "i64_compares_signed",
                "let a = 0 - 1;\nif (a < 1) { return 1; }\nreturn 0;\n",
            ),
            // And a signed shift is still arithmetic, which is the property the
            // change must not have taken away.
            new(
                "i8_shifts_arithmetically",
                "let a: i8 = 0 - 128;\nlet s: i8 = 7;\nreturn (a >> s) as Int;\n",
            ),
            // And through a function, so the width survives a call boundary —
            // the shape every driver helper has.
            new(
                "width_survives_a_call",
                "fn combine(hi: u16, lo: u16) -> u16 { return (hi << 8) | lo; }\n                 let a: u16 = 0x12;\nlet b: u16 = 0x34;\nreturn combine(a, b) as Int;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// `s.byte_at(i)` — the one string read that allocates nothing.
///
/// It exists for freestanding code: `char_at` next to it answers a *string* of
/// one character, which means an allocation, which means it cannot be used from
/// an interrupt handler or before there is a heap. What is pinned here is that
/// the native path answers what the VM answers, including at the two edges
/// where "out of range" has to be a value rather than a raise — a kernel's
/// print loop is bounded by `len`, and a raise per character would be a cost
/// paid on every message that is not a mistake.
#[test]
fn string_byte_at_differential() {
    run_differential(
        "string_byte_at",
        &[
            new(
                "sum_of_bytes",
                "let s = \"net: ok\";\nlet sum = 0;\nfor i in 0..s.len() { sum = sum + s.byte_at(i); }\nreturn sum;\n",
            ),
            new("first_byte", "let s = \"net\";\nreturn s.byte_at(0);\n"),
            new("last_byte", "let s = \"net\";\nreturn s.byte_at(s.len() - 1);\n"),
            // Both edges answer -1 rather than raising, and both have to answer
            // the *same* -1 on both backends.
            new("past_the_end", "let s = \"net\";\nreturn s.byte_at(99);\n"),
            new("before_the_start", "let s = \"net\";\nreturn s.byte_at(0 - 1);\n"),
            new("empty_string", "let s = \"\";\nreturn s.byte_at(0);\n"),
            // A byte past ASCII: the answer is a byte, not a character, so a
            // two-byte character is two answers.
            new(
                "multibyte_is_bytes",
                "let s = \"é\";\nreturn s.len() * 1000 + s.byte_at(0);\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

#[test]
fn try_catch_differential() {
    run_differential(
        "try_catch",
        &[
            // The shapes that used to reject, one per fix. Each lowers now, so
            // what these check is the part lowering cannot: that the answer is
            // the VM's. Two of the answers tried along the way *compiled* and
            // computed something else — see `aot/lower/src/try_region.rs`.
            //
            // A container the body mutates through a handle it shares with the
            // parent. It needs no cell: the mutation is already visible. Giving
            // it one — which reading `ListPush a=log` as a write to `log` did —
            // sent it round `dyn.from_list` / `dyn.as_list`, which is what lost
            // the push.
            new(
                "body_mutates_outer_list",
                "let log = [1];\ntry {\n  log.push(2);\n  log.push(3);\n} catch e {\n  log.push(9);\n}\nreturn log.len();\n",
            ),
            // The same, with a raise partway: the pushes before it must be
            // visible, and the handler's must land on the same list.
            new(
                "body_mutates_then_raises",
                "fn boom() { error(\"x\"); return 0; }\nlet log = [1];\ntry {\n  log.push(2);\n  boom();\n  log.push(3);\n} catch e {\n  log.push(9);\n}\nreturn log.len();\n",
            ),
            // A `Bool` parameter in scope. It is 0/1 — a machine word — but was
            // missing from the trampoline's word list, so the body got it as
            // `I64` and rejected on reading it: a `try` inside *any* function
            // taking a bool dropped its module to the VM, while the same
            // function with an `Int` parameter lowered.
            new(
                "body_reads_bool_param",
                "fn probe(c: Bool) -> Int {\n  let r = try { if c { error(\"boom\"); } 7 } catch e { -1 };\n  return r;\n}\nprintln(probe(false));\nprintln(probe(true));\nreturn 0;\n",
            ),
            // The `try`-as-expression shape that was on file as unlowerable:
            // a call, then a value-producing region, then both used.
            new(
                "value_region_after_a_call",
                "fn compute() -> Int { return 10; }\nfn probe(c: Bool) -> Int {\n  let q = compute();\n  let r = try { if c { error(\"boom\"); } 7 } catch e { -1 };\n  return q + r;\n}\nprintln(probe(false));\nprintln(probe(true));\nreturn 0;\n",
            ),
            // A `Float` parameter. The trampoline's signature is all
            // `long long`, so a float arrives in an *integer* register and the
            // body reads it back out of those bits (`Inst::BitsToFloat`).
            // Declaring the parameter `F64` instead — which is what "a float is
            // eight bytes, so it crosses" gets you — compiled and *segfaulted*.
            //
            // The arithmetic is what pins the bit-cast's direction: a wrong one
            // still runs and answers something. `1.5 * 2.0 + 0.5` is `3.5`, not
            // a denormal.
            new(
                "body_reads_float_param",
                "fn probe(f: Float, g: Float) -> Float {\n  let r = try { if f > 100.0 { error(\"boom\"); } f * g + 0.5 } catch e { -1.5 };\n  return r;\n}\nprintln(probe(1.5, 2.0));\nprintln(probe(0.25, 8.0));\nprintln(probe(1000.0, 1.0));\nprintln(probe(-3.5, 2.0));\nreturn 0;\n",
            ),
            // The same with a mixed parameter list, so the float is not the only
            // input crossing.
            new(
                "body_reads_mixed_params",
                "fn probe(f: Float, s: String, xs: List<Int>) -> Int {\n  let r = try { if f > 1.0 { error(\"boom\"); } xs.len() } catch e { -1 };\n  return r + s.len();\n}\nprintln(probe(0.5, \"ab\", [1, 2, 3]));\nprintln(probe(2.0, \"ab\", [1, 2, 3]));\nreturn 0;\n",
            ),
            // A container the body only *reads*. It travels in as a parameter,
            // which needs the trampoline's argument buffer to carry a handle —
            // a pointer is a machine word, and declaring every input `I64`
            // rejected this on its first instruction.
            new(
                "body_reads_outer_list",
                "let xs = [4, 5, 6];\nlet n = 0;\ntry {\n  n = xs.len();\n} catch e {\n  n = -1;\n}\nreturn n;\n",
            ),
            // A value that comes back out of its cell already boxed: nothing to
            // unbox, and nothing to reinterpret.
            new(
                "body_assigns_dyn_then_raises",
                "fn boom() { error(\"x\"); return 0; }\nfn pick(f) { if (f) { return 1; } return \"s\"; }\nlet v = pick(true);\ntry {\n  v = pick(false);\n  boom();\n} catch e {\n  v = pick(true);\n}\nreturn typeof(v);\n",
            ),
            // Two regions in one function, the second's body writing a register
            // the first's call window used. The parent's own writes and another
            // region's body writes are not the same thing.
            new(
                "two_regions_sharing_a_temporary",
                "fn add(a: Int, b: Int) -> Int { return a + b; }\nlet ok = 0;\ntry {\n  ok = add(2, 3);\n} catch e {\n  ok = -1;\n}\nlet mid = ok;\ntry {\n  ok = add(mid, 1);\n} catch e {\n  ok = -2;\n}\nreturn ok;\n",
            ),
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

/// `xs.clear()` on every list carrier, pinned to pure Cranelift.
///
/// One of the four mutating list methods that lowered for *no* carrier at all
/// (`pop` / `insert` / `remove_at` are the others). `clear` goes first because
/// it does not look at the element type — so it lands as one macro over all four
/// carriers rather than as four functions, three of which would have been
/// forgotten. That is the shape this file keeps recording: an operation whose
/// carriers were filled in one at a time and then not finished.
///
/// It answers the receiver, which is what the VM's `clear` returns — the same
/// handle, now empty. Pinning the *handle* matters: a copy would print the same
/// thing and leave the original untouched.
#[test]
fn clear_covers_every_list_carrier() {
    run_differential(
        "list_clear",
        &[
            new(
                "each_carrier",
                "println([1, 2, 3].clear());\nprintln([1.5, 2.5].clear());\nprintln([\"x\", \"y\"].clear());\nprintln([1, \"s\"].clear());\nprintln([].clear());\nreturn 0;\n",
            ),
            // The receiver is the same list, so the binding sees it emptied.
            new(
                "clears_in_place",
                "let a = [1, 2, 3];\nlet same = a.clear();\nprintln(a);\nprintln(a.len());\nprintln(same);\nprintln(a.is_empty());\nreturn 0;\n",
            ),
            new(
                "clear_then_reuse",
                "let a = [1, 2];\na.clear();\na.push(9);\nprintln(a);\nprintln(a.len());\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// `reverse` and `index_of` on every list carrier, pinned to pure Cranelift.
///
/// Both existed for `Int` alone, so `[1.5, 2.5].reverse()` dropped its whole
/// module to the VM — the right answer, three times slower, which is the gap
/// neither the differential corpus nor the coverage gate can see on its own.
///
/// `index_of` takes exactly the needle its `contains` takes on the same carrier,
/// because in the VM both answer through one `typed_list_position`. That is what
/// the `2` against a float list checks from both sides: `[1.0, 2.0]` finds it
/// (an `Int` needle coerces) and `[1.5, 2.5]` does not (2 is not 2.5) — a
/// lowering that skipped the coercion would answer nil for the first.
#[test]
fn reverse_and_index_of_cover_every_list_carrier() {
    run_differential(
        "list_reverse_index_of",
        &[
            new(
                "reverse_each_carrier",
                "println([1, 2, 3].reverse());\nprintln([1.5, 2.5, 3.5].reverse());\n\
                 println([\"a\", \"b\", \"c\"].reverse());\nprintln([1, \"s\", 2.5].reverse());\n\
                 println([].reverse());\nreturn 0;\n",
            ),
            // Non-mutating: the receiver still reads in its original order.
            new(
                "reverse_leaves_the_receiver",
                "let a = [1.5, 2.5];\nlet b = a.reverse();\nprintln(a);\nprintln(b);\n\
                 let s = [\"x\", \"y\"];\nprintln(s.reverse());\nprintln(s);\nreturn 0;\n",
            ),
            new(
                "index_of_each_carrier",
                "println([1, 2, 3].index_of(2));\nprintln([1, 2, 3].index_of(9));\n\
                 println([1.5, 2.5].index_of(2.5));\nprintln([\"a\", \"bb\"].index_of(\"bb\"));\n\
                 println([\"a\", \"bb\"].index_of(\"zz\"));\nprintln([1, \"s\", 2.5].index_of(\"s\"));\n\
                 println([1, \"s\", 2.5].index_of(2.5));\nprintln([1, \"s\"].index_of(9));\n\
                 println([].index_of(1));\nreturn 0;\n",
            ),
            new(
                "index_of_needle_coercion",
                "println([1.0, 2.0].index_of(2));\nprintln([1.5, 2.5].index_of(2));\n\
                 println([1.0, 2.0].contains(2));\nprintln([1.5, 2.5].contains(2));\nreturn 0;\n",
            ),
            // `index_of` answers `Int?`, so its result has to survive the things
            // a nullable does: a comparison, `!`, and `??`.
            new(
                "index_of_result_is_nullable",
                "let i = [\"a\", \"bb\"].index_of(\"bb\");\nprintln(i == 1);\nprintln(i!);\n\
                 let miss = [1.5].index_of(9.5);\nprintln(miss == nil);\nprintln(miss ?? -1);\nreturn 0;\n",
            ),
            new(
                "take_and_skip_each_carrier",
                "println([1, 2, 3, 4].take(2));\nprintln([1.5, 2.5, 3.5].take(2));\n\
                 println([\"a\", \"b\", \"c\"].take(2));\nprintln([1, \"s\", 2.5].take(2));\n\
                 println([1, 2, 3, 4].skip(2));\nprintln([1.5, 2.5, 3.5].skip(2));\n\
                 println([\"a\", \"b\", \"c\"].skip(2));\nprintln([1, \"s\", 2.5].skip(2));\nreturn 0;\n",
            ),
            // A count past the end clamps; a negative one raises, and the message
            // is stdout here because `catch` renders it.
            new(
                "take_and_skip_edges",
                "println([1.5].take(0));\nprintln([1.5].take(99));\nprintln([1.5].skip(99));\n\
                 println([].take(1));\n\
                 println(try { \"${[1.5, 2.5].take(-1)}\" } catch e { \"caught: ${e}\" });\n\
                 println(try { \"${[\"a\"].skip(-2)}\" } catch e { \"caught: ${e}\" });\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// `1 + f(x)` evaluates `f(x)` **once**.
///
/// The immediate form of an int binary op wants the constant on the right, so
/// `const + expr` lowered `expr` first to ask whether its value is a proven
/// `Int`. When the answer was no — which it is for any call without an annotated
/// return type — the code fell through and lowered `expr` *again*, leaving the
/// first lowering's instructions in the stream. So the operand ran twice: `1 +
/// side(7)` called `side` twice, and `return 1 + f(n - 1)` cost 2^n calls —
/// `f(5)` made 63 instead of 6, `f(50)` never finished, and `f(20000)` was a
/// hang where the VM's own depth limit is 100000.
///
/// It survived because the *answer* stays right for a pure function. Only a
/// counted side effect shows it, which is what these cases count. Both copies of
/// the lowering had it (`lower_bin_op` and `lower_into`), so both are exercised
/// here: `let a = …` goes through one and `println(…)`/`return` through the other.
#[test]
fn a_constant_on_the_left_evaluates_the_other_side_once() {
    run_differential(
        "commuted_immediate",
        &[
            new(
                "each_operator_shape",
                "let log = [];\nfn side(x) { log.push(x); return x; }\n\
                 let a = 1 + side(1);\nlet b = side(2) + 1;\nlet c = 2 * side(3);\n\
                 let d = 10 - side(4);\nlet e = 1 + side(5) + 1;\n\
                 println(\"${a} ${b} ${c} ${d} ${e}\");\nprintln(log);\nreturn 0;\n",
            ),
            // The count, not the answer: the answer was always right.
            new(
                "the_recursive_call_count",
                "let calls = 0;\nfn f(n) { calls = calls + 1; if (n <= 0) { return 0; } return 1 + f(n - 1); }\n\
                 println(f(5));\nprintln(calls);\nreturn 0;\n",
            ),
            // A depth the old lowering could not reach. Deliberately *not* the
            // f(50) that first showed the bug, and deliberately not an f(1000)
            // beside it either: 2^n calls is a hang, and a guard that hangs CI
            // instead of failing it is the mute failure mode this whole file
            // exists to avoid. 18 is the largest depth whose broken cost (262143
            // calls) is still finite, so this fails fast rather than never — and
            // the fixed version reaches the VM's own 100000-frame limit happily,
            // which `f(20000)` was checked against by hand.
            new(
                "a_depth_the_doubling_could_not_reach",
                "fn f(n) { if (n <= 0) { return 0; } return 1 + f(n - 1); }\nprintln(f(18));\nreturn 0;\n",
            ),
            // Through the other lowering: a statement-position expression and an
            // argument, neither of which goes through `lower_bin_op`'s `let`.
            new(
                "the_other_lowering",
                "let log = [];\nfn side(x) { log.push(x); return x; }\n\
                 println(1 + side(9));\nprintln(log.len());\n\
                 fn wrap(v) { return v; }\nprintln(wrap(2 * side(9)));\nprintln(log.len());\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// `sort` on the `Float` and `String` carriers — and the NaN that made writing
/// it find a panic on *both* backends.
///
/// `sort` existed for `Int` alone. The obvious extension is wrong in a way that
/// does not show on ordinary data: the VM's float comparator was
/// `partial_cmp(..).unwrap_or(Equal)`, which is not a total order once a NaN is
/// present (the NaN reads equal to every value while those values stay ordered),
/// and Rust's `sort_by` detects that and panics. So `[NaN, …].sort()` aborted the
/// interpreter — a Rust panic, so `try` could not catch it — and whether it fired
/// depended on the data: 601 elements went through, 60 did not.
///
/// Both sides now order the NaN (`val::compare_floats`, mirrored by lkrt's
/// `compare_floats`): all NaNs equal, every NaN above every number, `-0.0` and
/// `0.0` still equal because `==` says so. The boxed carrier has no `sort`
/// lowering on purpose — its order spans kinds, which is a mirror that wants its
/// own conformance test.
#[test]
fn sort_covers_the_float_and_string_carriers_including_nan() {
    let nan_list = (0..60)
        .map(|i| {
            if i % 4 == 0 {
                "nan".to_string()
            } else {
                format!("{}.5", 60 - i)
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    run_differential(
        "list_sort",
        &[
            new(
                "sort_each_carrier",
                "println([3, 1, 2].sort());\nprintln([3.5, 1.5, 2.5].sort());\n\
                 println([\"b\", \"a\", \"C\", \"aa\", \"\"].sort());\n\
                 println([].sort());\nprintln([1.5].sort());\nreturn 0;\n",
            ),
            // Non-mutating, like `reverse`.
            new(
                "sort_leaves_the_receiver",
                "let a = [2.5, 1.5];\nprintln(a.sort());\nprintln(a);\nreturn 0;\n",
            ),
            // Byte order, so uppercase sorts before lowercase and a multi-byte
            // character sorts by its UTF-8 bytes.
            new(
                "string_order_is_by_bytes",
                "println([\"é\", \"e\", \"z\", \"Z\"].sort());\nprintln([\"ab\", \"a\", \"b\"].sort());\nreturn 0;\n",
            ),
            new(
                "negative_zero_stays_equal_to_zero",
                "println([-0.0, 0.0, -1.5].sort());\nprintln([0.0, -0.0].sort());\n\
                 println(-0.0 == 0.0);\nreturn 0;\n",
            ),
            generated(
                "a_nan_no_longer_aborts_either_backend",
                format!(
                    "let z = 0.0;\nlet nan = z / z;\nlet xs = [{nan_list}];\n\
                     let sorted = xs.sort();\nprintln(sorted.len());\nprintln(sorted);\nreturn 0;\n"
                ),
            ),
            new(
                "nan_sorts_above_every_number",
                "let z = 0.0;\nlet nan = z / z;\n\
                 println([nan, 1.0].sort());\nprintln([1.0, nan].sort());\nprintln([nan, nan].sort());\n\
                 println([nan, 1.0, -1.0].sort());\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// `pop` / `insert` / `remove_at` on every list carrier, plus `first` / `last`
/// on the boxed one. Pinned to pure Cranelift.
///
/// None of the three mutators had a lowering on *any* carrier, so a single
/// `xs.pop()` anywhere in a program dropped the whole module to the VM — and
/// `first`/`last` covered three carriers and left the boxed one out.
///
/// The interesting parts are the edges, not the happy path: an empty `pop` is
/// nil rather than a raise, `insert` accepts `len` (that is where an append
/// goes) while `remove_at` does not, a negative index counts from the end for
/// both, and each of the four range failures has its own wording — which a
/// `catch` turns into stdout, so the text is part of the answer.
///
/// The `Int` inserted into a `Float` list is here because the two backends
/// disagree about the *representation* underneath: the VM rebuilds the list as
/// mixed and stores an `Int`, the lowering coerces to `f64` and keeps the
/// carrier. That is unobservable — the static type is `Float` either way, the
/// display agrees, and `/` is float division regardless — and `push`/`set`
/// already made the same choice. Pinned so it stays unobservable.
#[test]
fn pop_insert_and_remove_at_cover_every_list_carrier() {
    run_differential(
        "list_mutators",
        &[
            new(
                "pop_each_carrier",
                "let a = [1, 2, 3];\nprintln(\"${a.pop()} ${a}\");\n\
                 let b = [1.5, 2.5];\nprintln(\"${b.pop()} ${b}\");\n\
                 let c = [\"x\", \"y\"];\nprintln(\"${c.pop()} ${c}\");\n\
                 let d = [1, \"s\"];\nprintln(\"${d.pop()} ${d}\");\n\
                 println([].pop());\nreturn 0;\n",
            ),
            new(
                "insert_each_carrier",
                "let a = [1, 2, 3];\na.insert(1, 9);\nprintln(a);\n\
                 let b = [1.5];\nb.insert(0, 0.5);\nprintln(b);\n\
                 let c = [\"b\"];\nc.insert(0, \"a\");\nprintln(c);\n\
                 let d = [1, \"s\"];\nd.insert(1, 2.5);\nprintln(d);\n\
                 let e = [1, 2];\ne.insert(2, 9);\nprintln(e);\n\
                 let f = [1, 2];\nf.insert(-1, 9);\nprintln(f);\nreturn 0;\n",
            ),
            new(
                "remove_at_each_carrier",
                "let a = [1, 2, 3];\nprintln(\"${a.remove_at(1)} ${a}\");\n\
                 let b = [1.5, 2.5];\nprintln(\"${b.remove_at(-1)} ${b}\");\n\
                 let c = [\"a\", \"b\"];\nprintln(\"${c.remove_at(0)} ${c}\");\n\
                 let d = [1, \"s\"];\nprintln(\"${d.remove_at(0)} ${d}\");\nreturn 0;\n",
            ),
            new(
                "first_and_last_including_the_boxed_carrier",
                "println([1, 2, 3].first());\nprintln([1, 2, 3].last());\n\
                 println([1.5, 2.5].first());\nprintln([\"a\", \"b\"].last());\n\
                 println([1, \"s\"].first());\nprintln([1, \"s\"].last());\n\
                 println([].first());\nprintln([].last());\nreturn 0;\n",
            ),
            new(
                "mutator_index_edges",
                "println(try { \"${[1, 2].insert(-9, 5)}\" } catch e { \"${e}\" });\n\
                 println(try { \"${[1, 2].insert(3, 5)}\" } catch e { \"${e}\" });\n\
                 println(try { \"${[1.5].remove_at(-9)}\" } catch e { \"${e}\" });\n\
                 println(try { \"${[\"a\"].remove_at(2)}\" } catch e { \"${e}\" });\n\
                 println(try { \"${[].remove_at(0)}\" } catch e { \"${e}\" });\nreturn 0;\n",
            ),
            new(
                "an_int_into_a_float_list",
                "let a = [1.5, 2.5];\na.insert(0, 2);\nprintln(a);\nprintln(a[0]);\n\
                 println(a[0] / 4);\nprintln(a[0] == 2);\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// Container literals past the instruction's operand ceiling.
///
/// `NewList` names its element window as (u8 base, u8 len) and `NewMap` names
/// twice as many registers, so the compiler refused a literal over 255 elements
/// or 127 entries — with a compiler-internal message, and *only* when constant
/// folding did not apply. An all-literal `[0, …, 399]` became a heap constant
/// and compiled; changing one element to a variable made the same list a
/// compile error. 255 was the operand width, never a rule about lists.
///
/// Long literals now build empty and push the tail one element at a time
/// through a scratch register that is handed straight back — holding all of
/// them at once hits the same ceiling from the register side, which is what the
/// first attempt here did (`dst` landed at 256).
///
/// Pinned against the VM because the interesting parts are not the length: the
/// boundary elements either side of 255, left-to-right evaluation order of
/// element expressions that have side effects, element type widening across the
/// boundary, and a duplicate map key still resolving last-wins when the two
/// writes take different routes.
#[test]
fn long_container_literals_lower_and_agree() {
    let elements = (0..399).map(|i| (i * 2).to_string()).collect::<Vec<_>>().join(", ");
    let entries = (0..200)
        .map(|i| format!("\"k{i}\": {}", i * 3))
        .collect::<Vec<_>>()
        .join(", ");
    let duplicates = (0..130)
        .map(|i| format!("\"d{i}\": {i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mixed = (0..300)
        .map(|i| if i % 2 == 0 { i.to_string() } else { format!("\"{i}\"") })
        .collect::<Vec<_>>()
        .join(", ");
    let tapped = (0..300).map(|i| format!("tap({i})")).collect::<Vec<_>>().join(", ");
    run_differential(
        "long_literals",
        &[
            generated(
                "list_past_the_window",
                format!(
                    "let x = 7;\nlet a = [{elements}, x];\n\
                     println(\"${{a.len()}} ${{a[0]}} ${{a[254]}} ${{a[255]}} ${{a[256]}} ${{a[398]}} ${{a[399]}}\");\nreturn 0;\n"
                ),
            ),
            generated(
                "map_past_the_window",
                format!(
                    "let x = 7;\nlet m = {{{entries}, \"kx\": x}};\n\
                     println(\"${{m.len()}} ${{m[\"k0\"]}} ${{m[\"k126\"]}} ${{m[\"k127\"]}} ${{m[\"k199\"]}} ${{m[\"kx\"]}}\");\nreturn 0;\n"
                ),
            ),
            // The last write wins whether it lands inside `NewMap` or in a
            // `SetIndex` after it.
            generated(
                "duplicate_key_past_the_window",
                format!(
                    "let d = {{{duplicates}, \"d0\": 999}};\nprintln(\"${{d.len()}} ${{d[\"d0\"]}}\");\nreturn 0;\n"
                ),
            ),
            // A list whose elements stop being one type across the boundary.
            generated(
                "mixed_elements_past_the_window",
                format!(
                    "let s = \"z\";\nlet m = [{mixed}, s];\n\
                     println(\"${{m.len()}} ${{m[0]}} ${{m[1]}} ${{m[299]}} ${{m[300]}}\");\nreturn 0;\n"
                ),
            ),
            // Element expressions are evaluated left to right, and the tail is
            // no exception.
            generated(
                "evaluation_order_past_the_window",
                format!(
                    "let log = [];\nfn tap(v) {{ log.push(v); return v; }}\nlet b = [{tapped}];\n\
                     println(\"${{b.len()}} ${{log.len()}} ${{log[0]}} ${{log[299]}} ${{b[299]}}\");\nreturn 0;\n"
                ),
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// `slice` and `contains` on the non-`Int` list carriers, pinned to pure
/// Cranelift.
///
/// A list-method sweep (6 list shapes x 34 methods) found no wrong answers and
/// 62 fallbacks, and the fallbacks were lopsided: `Int` lists lacked 5 methods,
/// `Float` 18 and `String` 15. Two of those were pure dispatch-table gaps —
/// `list_h.{f64,str,dyn}_slice_from` and `{f64,dyn}_contains` had been in the
/// ABI all along, and only the `i64` arm was written. The rest need runtime
/// helpers that do not exist yet.
///
/// `i64` slices to a *window*; these slice to a fresh list. Both are what
/// `slice` means — the window is an optimisation the other carriers lack, not a
/// different answer, which is what pinning them against the VM checks.
#[test]
fn slice_and_contains_cover_the_other_carriers() {
    run_differential(
        "list_carrier_methods",
        &[
            new(
                "slice_from_on_each_carrier",
                "println([1.5, 2.5, 3.5].slice(1));\nprintln([\"a\", \"b\", \"c\"].slice(1));\nprintln([1, \"b\", 2.5].slice(1));\nreturn 0;\n",
            ),
            new(
                "slice_edges",
                "println([1.5, 2.5].slice(0));\nprintln([1.5, 2.5].slice(9));\nprintln([\"a\"].slice(1));\nreturn 0;\n",
            ),
            new(
                "contains_on_float_and_dyn",
                "println([1.5, 2.5].contains(2.5));\nprintln([1.5, 2.5].contains(9.5));\nprintln([1, \"b\"].contains(1));\nprintln([1, \"b\"].contains(\"b\"));\nprintln([1, \"b\"].contains(7));\nreturn 0;\n",
            ),
            // An Int needle against a Float list coerces, as `==` does.
            new(
                "contains_coerces_numbers",
                "println([1.0, 2.0].contains(2));\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// `xs.chain(ys)` over every list pairing, pinned to pure Cranelift.
///
/// `chain` is `+` spelled as a method, and the operator path has always covered
/// every pairing: same-typed keeps its carrier, cross-typed chains boxed. The
/// method path had exactly one arm, `List<Int>` twice — so `line.chain([byte])`
/// with a boxed element did not lower. One operation, two implementations, and
/// only one of them complete.
///
/// The x86 bare-metal kernel is where that showed up: eleven of its eighteen
/// native-lowering blockers were this one method.
#[test]
fn chain_covers_every_list_pairing() {
    run_differential(
        "list_chain_method",
        &[
            // The shape from the kernel: a typed list chained with a one-element
            // list whose element is boxed.
            new(
                "typed_receiver_boxed_argument",
                "let xs = [1, \"s\"];\nlet a = [1, 2];\nprintln(a.chain([xs[0]!]));\nreturn 0;\n",
            ),
            new(
                "same_typed_pairings",
                "println([1, 2].chain([3]));\nprintln([1.5].chain([2.5]));\nprintln([\"a\"].chain([\"b\"]));\nreturn 0;\n",
            ),
            // Repeated chaining in a loop, which is how the kernel builds a line.
            new(
                "chained_in_a_loop",
                "let line = [0];\nlet i = 0;\nwhile i < 6 {\n  line = line.chain([i * 2]);\n  i = i + 1;\n}\nprintln(line);\nprintln(line.len());\nreturn 0;\n",
            ),
            new(
                "empty_operands",
                "println([1].chain([]));\nprintln([].chain([1]));\nprintln([].chain([]));\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// Integer and float edge values, pinned to pure Cranelift.
///
/// `i64::MIN % -1` **panicked the interpreter**. Integer division overflow is a
/// panic in Rust — in release too, because the hardware traps — so `%` on those
/// operands aborted the process, which no `try` can catch and no differential
/// can compare: the VM side produces no output to diff against. The native side
/// answered `0`. Both `%` (three sites) and `math.floor`'s integer division now
/// wrap, which is what the rest of the language's integer arithmetic already
/// did and what the native side already computed.
#[test]
fn integer_and_float_edges_agree() {
    run_differential(
        "numeric_edges",
        &[
            // The crash, and its floor-division sibling.
            new(
                "division_overflow_wraps",
                "let mn = -9223372036854775808;\nlet d = -1;\nprintln(mn % d);\nuse math;\nprintln(math.floor(mn / d));\nprintln(mn / d);\nreturn 0;\n",
            ),
            new(
                "int_wrapping",
                "let mx = 9223372036854775807;\nlet mn = -9223372036854775808;\nprintln(mx + 1);\nprintln(mn - 1);\nprintln(mx * 2);\nprintln(-mn);\nprintln(0 - mn);\nreturn 0;\n",
            ),
            new(
                "signed_remainder_and_floor",
                "use math;\nprintln(7 % -3);\nprintln(-7 % 3);\nprintln(math.floor(7 / -3));\nprintln(math.floor(-7 / 3));\nreturn 0;\n",
            ),
            // Zero, signed zero, the infinities and NaN — including that NaN is
            // not equal to itself and that both zeroes compare equal.
            new(
                "float_specials",
                "let z = 0.0;\nlet nz = -0.0;\nprintln(z == nz);\nprintln(1.0 / z);\nprintln(-1.0 / z);\nprintln(z / z);\nprintln(z / z == z / z);\nprintln(\"${z} ${nz} ${1.0 / z} ${z / z}\");\nprintln(1e300 * 1e300);\nreturn 0;\n",
            ),
            // A float past the integer range, and NaN, cast to Int.
            new(
                "float_to_int_casts",
                "let big = 1e19;\nlet nan = 0.0 / 0.0;\nprintln(big as Int);\nprintln(-big as Int);\nprintln(nan as Int);\nprintln(9223372036854775807 as Float);\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// Struct update syntax with a typed-map overlay, pinned to pure Cranelift.
///
/// The sixth instance of one mistake: `to_dyn_map_handle` converted a typed map
/// to the boxed carrier and claimed "iteration order is preserved — the rebuild
/// replays the source order". Re-inserting a table's entries into a fresh table
/// in its *iteration* order is a different insertion sequence from the one that
/// built it, so the copy need not iterate the same way.
///
/// `P { ..base, x: 42 }` reaches it: the overlay is the `{x: 42}` field literal,
/// a typed map, and the overlay's order is the tail of the merged result's. The
/// overlay is now walked where it lives — nothing is copied, so there is no
/// order to lose.
#[test]
fn a_struct_update_keeps_the_field_order() {
    run_differential(
        "struct_update_order",
        &[
            new(
                "int_overlay",
                "struct P { a: Int, b: Int, c: Int }\nlet p = P { a: 1, b: 2, c: 3 };\nlet q = P { ..p, b: 9 };\nprintln(q);\nprintln(q.b);\nreturn 0;\n",
            ),
            // A wide struct, so the field maps grow past one table size and the
            // insertion sequence actually matters.
            new(
                "many_fields",
                "struct W { f0: Int, f1: Int, f2: Int, f3: Int, f4: Int, f5: Int, f6: Int, f7: Int, f8: Int, f9: Int }\nlet w = W { f0: 0, f1: 1, f2: 2, f3: 3, f4: 4, f5: 5, f6: 6, f7: 7, f8: 8, f9: 9 };\nprintln(W { ..w, f5: 50 });\nprintln(W { ..w, f0: 100, f9: 900 });\nreturn 0;\n",
            ),
            // Float and Bool overlays ride different carriers.
            new(
                "float_and_bool_overlays",
                "struct F { x: Float, y: Float }\nlet f = F { x: 1.5, y: 2.5 };\nprintln(F { ..f, y: 9.5 });\nstruct B { p: Bool, q: Bool }\nlet b = B { p: true, q: false };\nprintln(B { ..b, q: true });\nreturn 0;\n",
            ),
            // A mixed overlay is the boxed carrier, which was always fine —
            // here to keep both paths under the same gate.
            new(
                "mixed_overlay",
                "struct M { a: Int, b: String }\nlet m = M { a: 1, b: \"x\" };\nprintln(M { ..m, a: 2, b: \"y\" });\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// A **caught** error's message, pinned to pure Cranelift.
///
/// The loud-failure contract compares success and stdout, not the text of a
/// failure — and that is right for an *uncaught* one, whose text is the host's
/// wrapper. It says nothing about a caught one, and there the message **is**
/// stdout: `catch e { println(e) }` prints it.
///
/// They did not match. `assert` differed by a capital letter; every dynamic
/// type error said `runtime type error` where the VM names the operator and
/// both operand kinds; a list store past the end said `runtime error` where the
/// VM says `list index 9 out of bounds`; `Set.add(1.5)` lost the `set.add()
/// value:` prefix.
///
/// The wording is the VM's, warts included — a string of 8 bytes reports as
/// `Object` because the VM formats a value's *representation* rather than its
/// type. That is filed as its own (VM-side) fix; mirroring it here is what
/// makes the two backends agree in the meantime.
#[test]
fn a_caught_errors_message_matches() {
    run_differential(
        "caught_error_text",
        &[
            new(
                "dynamic_type_errors_name_their_operands",
                "let xs = [1, \"a\"];\nlet out = try {\n  let a = xs[0]!;\n  let b = xs[1]!;\n  println(a - b);\n  \"no-raise\"\n} catch e {\n  \"caught: ${e}\"\n};\nprintln(out);\nreturn 0;\n",
            ),
            new(
                "unary_minus_and_not",
                "let xs = [\"a\"];\nprintln(try { -xs[0]!; \"no\" } catch e { \"caught: ${e}\" });\nlet ys = [1];\nprintln(try { !ys[0]!; \"no\" } catch e { \"caught: ${e}\" });\nreturn 0;\n",
            ),
            new(
                "ordering_across_kinds",
                "let xs = [1, \"a\"];\nlet out = try {\n  println(xs[0]! < xs[1]!);\n  \"no-raise\"\n} catch e {\n  \"caught: ${e}\"\n};\nprintln(out);\nreturn 0;\n",
            ),
            new(
                "list_store_out_of_bounds",
                "let xs = [1];\nprintln(try { xs[9] = 2; \"no\" } catch e { \"caught: ${e}\" });\nprintln(try { xs[-9] = 2; \"no\" } catch e { \"caught: ${e}\" });\nreturn 0;\n",
            ),
            new(
                // The key rule is a *check-time* error wherever the key's type is
                // certainly wrong (`s.add(1.5)` no longer compiles). It stays a
                // run-time one exactly where the checker is deliberately
                // conservative — a union may be the Int at run time — so that is
                // the shape this reaches it through, and the shape whose message
                // has to match on both ends.
                "float_member_and_key",
                "fn opaque(v: Any) -> Any { return v; }\nlet s = Set([]);\nprintln(try { s.add(opaque(1.5)); \"no\" } catch e { \"caught: ${e}\" });\nlet m = {};\nprintln(try { m[opaque(1.5)] = 1; \"no\" } catch e { \"caught: ${e}\" });\nreturn 0;\n",
            ),
            new(
                "assert_is_lowercase",
                "println(try { assert(1 == 2); \"no\" } catch e { \"caught: ${e}\" });\nprintln(try { assert(false, \"nope\"); \"no\" } catch e { \"caught: ${e}\" });\nreturn 0;\n",
            ),
            // The kinds a message can name, including the representation wart:
            // a string of 8 bytes is `Object`, one of 7 is `String`.
            new(
                "operand_kind_names",
                "let xs = [1, \"ab\", \"aaaaaaaaaa\", 2.5, true, [1], {\"a\": 1}];\nprintln(try { xs[0]! - xs[1]!; \"no\" } catch e { \"${e}\" });\nprintln(try { xs[0]! - xs[2]!; \"no\" } catch e { \"${e}\" });\nprintln(try { xs[0]! - xs[4]!; \"no\" } catch e { \"${e}\" });\nprintln(try { xs[0]! - xs[5]!; \"no\" } catch e { \"${e}\" });\nprintln(try { xs[0]! - xs[6]!; \"no\" } catch e { \"${e}\" });\nprintln(try { xs[1]! * xs[3]!; \"no\" } catch e { \"${e}\" });\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// `+` and `==` across types, pinned to pure Cranelift.
///
/// From an operator x type-pair sweep (11 values x 13 operators). Two things
/// came out of it, and only one of them was a missing feature.
///
/// **`==` across kinds is a constant.** Every arm paired a kind with itself, so
/// `1 == "a"`, `true == [1]`, `nil == 2.5` and a hundred other pairings — each
/// a `false` the VM computes without hesitating — took the whole program down.
/// Both kinds are known at lower time, so the answer folds.
///
/// **`+` was answering the wrong question.** The `Str + Dyn` arm unboxed the
/// Dyn with `as_str`, which *raises* unless it holds a string, on the belief
/// that the VM "only accepts Str + Str here". `Executor::dynamic_add` says
/// otherwise, in this order: numbers, then two maps merge, then a list on
/// either side concatenates, then a string on either side display-concatenates.
/// So `"v=" + x` with a boxed Int is `v=1` and `"p=" + xs` with a boxed list is
/// the *list* `["p=", 1, 2]` — the old arm aborted both.
#[test]
fn mixed_type_addition_and_equality() {
    run_differential(
        "mixed_type_ops",
        &[
            new(
                "cross_kind_equality_is_false",
                "println(1 == \"a\");\nprintln(1 != \"a\");\nprintln(true == 1);\nprintln(nil == 0);\nprintln(nil == false);\nprintln([1] == {\"a\": 1});\nprintln(2.5 == \"x\");\nprintln(Set([1]) == [1]);\nreturn 0;\n",
            ),
            new(
                "numeric_kinds_still_coerce",
                "println(1 == 1.0);\nprintln(1 != 1.0);\nprintln([1] == [1.0]);\nprintln(nil == nil);\nreturn 0;\n",
            ),
            new(
                "string_plus_scalar_displays",
                "println(1 + \"ab\");\nprintln(\"ab\" + 1);\nprintln(2.5 + \"x\");\nprintln(\"x\" + 2.5);\nprintln(true + \"x\");\nprintln(\"x\" + nil);\nprintln(\"a\" + \"b\");\nreturn 0;\n",
            ),
            // The wrong answer: a boxed operand that is not a string.
            new(
                "string_plus_boxed_scalar",
                "let xs = [1, \"a\", 2.5, true];\nfor x in xs { println(\"v=\" + x); }\nreturn 0;\n",
            ),
            // A list operand outranks a string one, so this is a list.
            new(
                "a_list_operand_outranks_a_string",
                "let xs = [[1, 2], \"s\"];\nlet a = xs[0]!;\nprintln(\"t=${a}\");\nprintln(\"p=\" + a);\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// A typed map boxed into a container keeps its own entry order.
///
/// Boxing used to mean `str_i64_to_dyn` — rebuilding the map into a
/// `str -> Dyn` one by re-inserting in iteration order. A fresh table filled by
/// a different insertion sequence has a different layout, so once the history
/// includes deletions the copy iterates differently from the original, and
/// `println([m])` listed its entries in an order the VM never produces. A
/// **wrong answer**, not a fallback, and it was reachable two ways: a struct
/// field holding a map (long-standing) and a map inside a list or map (new with
/// the boxable-element work).
///
/// The rule against it was already written down on `DYN_RAW`: boxing must not
/// re-represent a container. A typed map now boxes in place under a tag naming
/// its carrier. The one surviving rebuild is equality's, which is order-free.
///
/// The deletions are the point of these cases — without them the two layouts
/// coincide and the bug hides.
#[test]
fn a_boxed_typed_map_keeps_its_order() {
    run_differential(
        "typed_map_boxing_order",
        &[
            new(
                "in_a_list_after_deletions",
                "let m = {};\nlet i = 0;\nwhile i < 200 {\n  m[\"key_number_${i}\"] = i;\n  i = i + 1;\n}\nlet j = 0;\nwhile j < 60 {\n  m.delete(\"key_number_${j * 3}\");\n  j = j + 1;\n}\nm[\"late\"] = 1;\nprintln([m]);\nprintln({\"w\": m});\nreturn 0;\n",
            ),
            new(
                "in_a_struct_field_after_deletions",
                "struct S { m: Map<String, Int> }\nlet m = {};\nlet i = 0;\nwhile i < 200 {\n  m[\"key_number_${i}\"] = i;\n  i = i + 1;\n}\nlet j = 0;\nwhile j < 60 {\n  m.delete(\"key_number_${j * 3}\");\n  j = j + 1;\n}\nprintln(S { m: m });\nreturn 0;\n",
            ),
            // Boxing in place means the box and the original are one map.
            new(
                "boxing_keeps_identity",
                "let m = {\"a\": 1};\nlet holder = [m];\nm[\"b\"] = 2;\nprintln(holder);\nprintln(m);\nreturn 0;\n",
            ),
            // Equality still crosses representations: a typed carrier against a
            // boxed map is the same map written two ways.
            new(
                "equality_across_representations",
                "println({\"a\": 1} == {\"a\": 1.0});\nprintln([{\"a\": 1}] == [{\"a\": 1}]);\nprintln([{\"a\": 1}] == [{\"a\": 2}]);\nprintln({\"k\": {\"a\": 1}} == {\"k\": {\"a\": 1}});\nreturn 0;\n",
            ),
            // Int-keyed maps ride the same path: boxed in place, displayed and
            // iterated off the carrier.
            new(
                "int_keyed_maps_box_and_iterate",
                "let m = {1: 10, 2: 20, 5: 50};\nprintln([m]);\nprintln({\"w\": m});\nprintln(m == {5: 50, 1: 10, 2: 20});\nlet n = 0;\nfor pair in m { n = n + 1; }\nprintln(n);\nfor pair in m { println(pair); }\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// `for x in s` over a `Set`, and `for b in bytes`, pinned to pure Cranelift.
///
/// A set's *iteration* order is its hash order — unlike its display order,
/// which is imposed — so this needs the mirror discipline. It could not have it
/// while `lkset` kept its own four-variant key that folded both string shapes
/// into one: membership agreed with the VM and the **hash** did not, and the
/// way that showed up was iteration never being lowered at all. One `RtKey`,
/// one hash, and `set_iteration_order_matches_the_vm` can then say something.
///
/// `Bytes` iterates its byte values in order — no hash anywhere.
#[test]
fn sets_and_bytes_iterate_natively() {
    run_differential(
        "set_bytes_iter",
        &[
            // Enough members to force several table growths, so the order is a
            // real check rather than a small set's coincidence.
            new(
                "many_int_members",
                "let s = Set([]);\nlet i = 0;\nwhile i < 40 {\n  s.add(i * 3 - 7);\n  i = i + 1;\n}\nlet out = [];\nfor x in s { out.push(x); }\nprintln(out);\nreturn 0;\n",
            ),
            // Short (inline) and long (heap) keys mixed: the two shapes hash
            // differently, which is exactly what one shared key type buys.
            new(
                "short_and_long_string_members",
                "let s = Set([\"alpha\", \"b\", \"gamma_long_key\", \"d\", \"another_long_one\"]);\nfor y in s { println(y); }\nreturn 0;\n",
            ),
            new(
                "iterate_after_mutation",
                "let s = Set([1, 2, 3]);\ns.delete(2);\ns.add(9);\nfor x in s { println(x); }\nprintln(s.len());\nreturn 0;\n",
            ),
            new(
                "bytes_iterate_in_order",
                "use bytes;\nfor b in bytes.from_string(\"hey\") { println(b); }\nlet n = 0;\nfor b in bytes.from_string(\"\") { n = n + 1; }\nprintln(n);\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// `Set` and `Bytes` as boxed values, pinned to pure Cranelift.
///
/// Neither had a `LkDyn` tag, so neither could be *boxed* — and boxing is how a
/// value enters a mixed container, a struct field, or a returned position. So
/// `[s]` and `{"k": b}` had no lowering, for a reason that had nothing to do
/// with sets or byte buffers: the dynamic carrier did not cover every value the
/// language has. `DYN_SET` and `DYN_BYTES` close that, and each tags the handle
/// in place — no rebuild, so identity and any mutation ride along.
#[test]
fn sets_and_bytes_are_boxable() {
    run_differential(
        "dyn_set_bytes",
        &[
            new(
                "set_in_containers",
                "let s = Set([2, 1]);\nprintln([s]);\nprintln({\"k\": s});\nprintln([s, s]);\nreturn 0;\n",
            ),
            new(
                "bytes_in_containers",
                "use bytes;\nlet b = bytes.from_string(\"hi\");\nprintln([b]);\nprintln({\"k\": b});\nreturn 0;\n",
            ),
            // Boxing tags in place, so a mutation after the box is visible
            // through it — the same handle, not a copy.
            new(
                "boxing_keeps_identity",
                "let s = Set([1]);\nlet holder = [s];\ns.add(2);\nprintln(holder);\nprintln(s);\nreturn 0;\n",
            ),
            new(
                "returned_and_compared",
                "fn id(a) { return a; }\nlet s = Set([1, 2]);\nprintln(id(s));\nprintln(id(s) == Set([2, 1]));\nuse bytes;\nlet b = bytes.from_string(\"ab\");\nprintln(id(b));\nprintln(id(b) == bytes.from_string(\"ab\"));\nreturn 0;\n",
            ),
            // Mixed with other element types, and nested one level down.
            new(
                "mixed_and_nested",
                "use bytes;\nlet s = Set([1]);\nlet b = bytes.from_string(\"x\");\nprintln([1, s, \"t\", b]);\nprintln({\"a\": [s], \"b\": b});\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// `Set` display and `==`, pinned to pure Cranelift.
///
/// A set displays sorted, because its hash iteration order is not something to
/// show. It sorted the *rendered text* rather than the members, so
/// `Set([1, 2, 10, 20, 3])` printed `Set([1,10,2,20,3])` — an order that is
/// neither insertion, nor value, nor anything a reader can use. That is fixed
/// in the VM (`RuntimeMapKey::display_order`) and mirrored natively.
///
/// Mirrored is the easy word here: the display order is *imposed*, not the hash
/// order, and imposed on the members' values — so both sides compare content
/// and no hasher or layout can drift them apart. This is the one container
/// display that needs no mirror discipline.
#[test]
fn sets_display_sorted_and_compare_natively() {
    run_differential(
        "set_display_eq",
        &[
            // The shape that was wrong: numbers whose decimal texts sort
            // differently from their values, and negatives.
            new(
                "numbers_sort_by_value",
                "println(Set([1, 2, 10, 20, 3]));\nprintln(Set([-1, -2, 5]));\nprintln(Set([100, 99, 9]));\nreturn 0;\n",
            ),
            // Strings sort lexicographically across the 7-byte short/long
            // split, which a variant-order comparison would get wrong.
            new(
                "strings_sort_by_content",
                "println(Set([\"ab\", \"aaaaaaaaaa\", \"z\"]));\nprintln(Set([\"b\", \"a\"]));\nreturn 0;\n",
            ),
            // Kinds group before values compare.
            new(
                "mixed_kinds_group",
                "let s = Set([]);\ns.add(nil);\ns.add(true);\ns.add(1);\ns.add(\"a\");\ns.add(false);\ns.add(-5);\nprintln(s);\nprintln(s.len());\nreturn 0;\n",
            ),
            new(
                "empty_and_duplicates",
                "println(Set([]));\nprintln(Set([1, 1, 2]));\nprintln(Set([1, 1, 2]).len());\nreturn 0;\n",
            ),
            new(
                "equality_is_order_free",
                "println(Set([1, 2]) == Set([2, 1]));\nprintln(Set([1, 2]) == Set([1, 3]));\nprintln(Set([1]) == Set([1, 2]));\nprintln(Set([]) == Set([]));\nreturn 0;\n",
            ),
            // In a template and after a mutation.
            new(
                "template_and_mutation",
                "let s = Set([3, 1]);\nprintln(\"s=${s}\");\ns.add(2);\ns.delete(3);\nprintln(s);\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// `==` over maps and structs, pinned to pure Cranelift.
///
/// Every *list* pairing compared natively; no *map* pairing did, not even
/// `{"a": 1} == {"a": 1}`. Both sides now box to a `Dyn` map and `dyn.eq`
/// decides — order-free, key by key, with the VM's numeric coercion.
///
/// That route was not usable as it stood: a struct instance is a marked map,
/// and `dyn_eq_inner` compared only the entries, so it would have answered
/// `true` for `P{x:1} == Q{x:1}` and `P{x:1} == {"x":1}` where the VM answers
/// `false`. The mark decides all three cases at once — every declared struct
/// has an id and a plain map has none — so it is read first.
#[test]
fn maps_and_structs_compare_natively() {
    run_differential(
        "map_struct_eq",
        &[
            new(
                "typed_and_boxed_maps",
                "println({\"a\": 1} == {\"a\": 1});\nprintln({\"a\": 1} == {\"a\": 2});\nprintln({\"a\": 1} == {\"a\": 1, \"b\": 2});\nprintln({\"a\": 1, \"b\": 2} == {\"b\": 2, \"a\": 1});\nprintln({\"a\": 1} == {\"b\": 1});\nprintln({} == {});\nreturn 0;\n",
            ),
            // A value's *number* coerces across the two maps' element types,
            // but a bool is not a number.
            new(
                "numeric_coercion_and_bools",
                "println({\"a\": 1} == {\"a\": 1.0});\nprintln({\"a\": 1.5} == {\"a\": 1.5});\nprintln({\"a\": true} == {\"a\": true});\nprintln({\"a\": 1} == {\"a\": true});\nprintln({\"a\": 1, \"b\": \"x\"} == {\"a\": 1, \"b\": \"x\"});\nreturn 0;\n",
            ),
            new(
                "nested_values",
                "println({\"a\": [1, 2]} == {\"a\": [1, 2]});\nprintln({\"a\": [1, 2]} == {\"a\": [1, 3]});\nprintln({\"a\": {\"b\": 1}} == {\"a\": {\"b\": 1}});\nreturn 0;\n",
            ),
            // The struct mark: same shape, different type, and struct against
            // the bare map with the same fields.
            new(
                "struct_identity",
                "struct P { x: Int }\nstruct Q { x: Int }\nprintln(P{x:1} == P{x:1});\nprintln(P{x:1} == P{x:2});\nprintln(P{x:1} == Q{x:1});\nprintln(P{x:1} == {\"x\": 1});\nprintln({\"x\": 1} == P{x:1});\nreturn 0;\n",
            ),
            // Through a container, where `dyn_eq` recurses into the arm rather
            // than being called on it directly.
            new(
                "struct_identity_nested",
                "struct P { x: Int }\nstruct Q { x: Int }\nprintln([P{x:1}] == [P{x:1}]);\nprintln([P{x:1}] == [Q{x:1}]);\nprintln({\"k\": P{x:1}} == {\"k\": Q{x:1}});\nreturn 0;\n",
            ),
            // `Map<str, Bool>.len()` was missing from the `Len` table, though
            // it rides the same carrier as `Map<str, Int>`.
            new(
                "bool_map_len",
                "let m = {\"a\": true, \"b\": false};\nprintln(m.len());\nprintln(m);\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// A map literal whose values are computed, and the display of a typed map.
///
/// Two holes that met in the middle. `NewMap` — the opcode for a literal whose
/// values are not all constants, `{"k": a}` — had no lowering at all, so a
/// program that built a record from anything it had computed fell back whole;
/// the list spelling `[a, a + 1]` always lowered, which is what kept it
/// invisible. And displaying a *typed* map was refused on a ruling that
/// predates `lkrt/src/vm_mirror.rs`: the ruling said the two runtimes do not
/// share a map's iteration order, and the mirror's entire job is that they do.
/// The `MapStrDyn` arm had already been let through, so `println({"a": 1})`
/// cost a program its lowering while `println({"a": 1, "b": "x"})` did not.
///
/// Pinned to pure Cranelift: byte-exact display is the acceptance criterion —
/// key quoting, `,`/`:` separators, and above all the entry order.
#[test]
fn a_computed_map_literal_lowers_and_a_typed_map_displays() {
    run_differential(
        "map_literal_and_display",
        &[
            new(
                "computed_int_values",
                "let a = 5;\nlet m = {\"i\": a, \"j\": a + 1};\nprintln(m);\nprintln(m[\"j\"] ?? 0);\nreturn 0;\n",
            ),
            new(
                "computed_from_calls",
                "fn f(x: Int) -> Int { return x * 2; }\nlet m = {\"a\": f(3), \"b\": f(4)};\nprintln(m);\nreturn 0;\n",
            ),
            new(
                "float_and_bool_values",
                "let f = 1.5;\nlet b = true;\nprintln({\"f\": f, \"g\": f * 2.0});\nprintln({\"b\": b, \"c\": !b});\nreturn 0;\n",
            ),
            new(
                "heterogeneous_values_box",
                "let a = 5;\nlet s = \"v\";\nlet f = 1.5;\nprintln({\"i\": a, \"s\": s, \"f\": f, \"n\": nil});\nreturn 0;\n",
            ),
            // Int keys, whose order is the *stage-1* table's: the VM runs no
            // stage 2 for a non-string key, so the native carrier is keyed by
            // `vm_mirror::IntKey` (hashing as `RtKey::Int`) and filled in
            // literal order. Rehashing into an `FxMap<i64, _>`, which is what
            // it used to do, made `{1: 1.5, 2: 2.5}` come out `1,2` against
            // the VM's `2,1`.
            new(
                "int_keys",
                "let a = 5;\nlet m = {1: a, 3: a + 1};\nprintln(m);\nprintln(m[1] ?? 0);\nprintln({1: 1.5, 2: 2.5});\nprintln({7: 1, 2: 2, 9: 3, 4: 4, 1: 5});\nprintln({-3: 1.5, 7: 2.5, 0: 0.5});\nreturn 0;\n",
            ),
            // The same, built by runtime stores rather than a literal: the
            // insertion sequence is the program's, and both sides replay it.
            new(
                "int_keys_stored_one_by_one",
                "let m = {10: 1};\nlet i = 0;\nwhile i < 20 {\n  m[i * 7] = i;\n  i = i + 1;\n}\nprintln(m);\nprintln(m.len());\nreturn 0;\n",
            ),
            // Enough keys to force several table growths, so the order is a
            // real check rather than one small map's coincidence.
            new(
                "many_keys_keep_the_vm_order",
                "let m = {};\nlet i = 0;\nwhile i < 40 {\n  m[\"k${i}\"] = i * 3;\n  i = i + 1;\n}\nprintln(m);\nprintln(m.len());\nreturn 0;\n",
            ),
            // The constant spelling of the same map, which took the display
            // refusal too.
            new(
                "constant_map_displays",
                "println({\"a\": 1, \"b\": 2});\nprintln({\"a\": 1.5});\nprintln({\"a\": true});\nprintln({});\nreturn 0;\n",
            ),
            // A map inside a template and inside a list. The list case is the
            // one that printed `{\"a\":1}` where the VM printed `[{\"a\":1}]`:
            // no arm of `NewList` could box a typed map, so the destination
            // kept only the argument-pack view and the call read *that*.
            new(
                "nested_in_a_template_and_a_list",
                "let a = 1;\nlet m = {\"a\": a};\nprintln(\"m=${m}\");\nprintln([m]);\nprintln([m, m]);\nreturn 0;\n",
            ),
            // A duplicate key keeps the last value, both spellings.
            new(
                "duplicate_key_keeps_the_last",
                "let a = 5;\nprintln({\"d\": a, \"d\": a + 1});\nreturn 0;\n",
            ),
            new(
                "container_values_box",
                "let a = 5;\nprintln({\"n\": [a, a + 1], \"m\": {\"k\": a}});\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// A container reassigned inside a `try` body, pinned to pure Cranelift.
///
/// A register the body assigns crosses back through an output cell, and a
/// container had no way out of one — so `try { xs = […]; } catch e { }` dropped
/// the program to the VM. An *already boxed* container round-trips by pointer
/// (`dyn.from_list` only tags the handle), which is what makes this sound; a
/// typed one still refuses, because its boxing is an element-wise copy and the
/// round trip would hand back a different list.
#[test]
fn a_dyn_container_crosses_a_try_region() {
    run_differential(
        "try_container_cell",
        &[
            new(
                "dyn_list_reassigned",
                "let xs = [1, \"a\"];\ntry { xs = [2, \"b\"]; } catch e { }\nprintln(xs);\nreturn 0;\n",
            ),
            new(
                "dyn_map_reassigned",
                "let m = {\"a\": 1, \"b\": \"x\"};\ntry { m = {\"a\": 2, \"b\": \"y\"}; } catch e { }\nprintln(m);\nreturn 0;\n",
            ),
            // The typed containers, which need the *raw* cell: their boxing is
            // an element-wise copy, so a boxed round trip would hand back a
            // different handle.
            new(
                "typed_list_reassigned",
                "let xs = [1];\ntry { xs = [2, 3]; } catch e { }\nprintln(xs);\nreturn 0;\n",
            ),
            new(
                "typed_map_reassigned",
                "let m = {\"k\": 1};\ntry { m = {\"k\": 2}; } catch e { }\nprintln(m[\"k\"] ?? 0);\nreturn 0;\n",
            ),
            new(
                "set_reassigned",
                "let s = Set([1, 2]);\ntry { s = Set([3]); } catch e { }\nprintln(s.len());\nreturn 0;\n",
            ),
            new(
                "bytes_reassigned",
                "use bytes;\nlet b = bytes.from_string(\"a\");\ntry { b = bytes.from_string(\"bc\"); } catch e { }\nprintln(bytes.len(b));\nreturn 0;\n",
            ),
            // The body raises before assigning: the cell still holds the value
            // the caller seeded it with, which is what the VM shows.
            new(
                "raised_before_assigning",
                "let xs = [1, \"a\"];\ntry { error(\"boom\"); xs = [2, \"b\"]; } catch e { }\nprintln(xs);\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// Several regions in one function, pinned to pure Cranelift.
///
/// A body that writes a register and does not carry it back leaves the parent's
/// copy poisoned, and a later read of one is how the fixpoint discovers which
/// registers need a cell. The read used to say only *which register*, so the
/// cell went to every region in the function — including regions whose body had
/// merely reused that register number as a scratch. The parent has no
/// definition for such a register at its own region's start, so seeding its
/// cell read it before pc 0 and the whole function fell back: adding a second
/// `try` to a function that had one cost the *first* one its lowering.
///
/// The poison now names the body that left it, so the cell goes to that one.
#[test]
fn several_try_regions_share_a_function() {
    run_differential(
        "try_multi_region",
        &[
            // The shape that failed: a container region, then an unrelated
            // scalar one. The container body's literal lands in a scratch
            // register that the scalar variable happens to reuse.
            new(
                "container_region_then_scalar_region",
                "let a = [1];\ntry { a = [2]; } catch e { }\nlet b = 3;\ntry { b = 4; } catch e { }\nprintln(a);\nprintln(b);\nreturn 0;\n",
            ),
            new(
                "three_regions_three_types",
                "let a = [1];\nlet m = {\"k\": 1};\nlet s = \"x\";\ntry { a = [2, 3]; } catch e { }\ntry { m = {\"k\": 9, \"j\": 2}; } catch e { }\ntry { s = \"y\"; a = [7]; } catch e { }\nprintln(\"${a} ${m[\"k\"] ?? 0} ${s}\");\nreturn 0;\n",
            ),
            // A region inside a loop, after a region outside it: the poison is
            // per block, and the loop header's phi has to see the cell's value.
            new(
                "region_then_region_in_a_loop",
                "let a = [1];\ntry { a = [5]; } catch e { }\nlet n = 0;\nfor i in 0..4 {\n  try { n = n + i + a[0]!; } catch e { }\n}\nprintln(n);\nreturn 0;\n",
            ),
            // Both regions raise: each cell keeps what the parent seeded.
            new(
                "both_regions_raise",
                "let a = [1];\nlet b = 3;\ntry { error(\"x\"); a = [2]; } catch e { }\ntry { error(\"y\"); b = 4; } catch e { }\nprintln(a);\nprintln(b);\nreturn 0;\n",
            ),
            // Inside a called function rather than the entry, and the second
            // region reads what the first one wrote.
            new(
                "regions_in_a_function_chained",
                "fn f(k: Int) -> Int {\n  let acc = [0];\n  try { acc = [k, k + 1]; } catch e { }\n  let t = 0;\n  try { t = acc[1]! * 2; } catch e { t = -1; }\n  return t + acc[0]!;\n}\nprintln(f(3));\nprintln(f(10));\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// A `try` body that `return`s from the enclosing function, pinned to pure
/// Cranelift.
///
/// The body is outlined into a function of its own, so a `return` written in it
/// would return from *that* function. It used to be a rejection, which made the
/// statement form fall back while the value form lowered — the same function,
/// two spellings, one three times slower. The body now has a third channel
/// beside "the value" and "it raised": a flag cell and a value cell, checked on
/// the ok edge.
#[test]
fn a_try_body_may_return_from_its_function() {
    run_differential(
        "try_body_return",
        &[
            new(
                "int_return_and_fallthrough",
                "fn f(n: Int) -> Int {\n  let v = try { if (n > 0) { return 10; } 0 } catch e { -1 };\n  return v;\n}\nprintln(f(1));\nprintln(f(-1));\nprintln(f(0));\nreturn 0;\n",
            ),
            // Every carrier the value cell has to hand back.
            new(
                "string_return",
                "fn f(n: Int) -> String {\n  let v = try { if (n > 0) { return \"big\"; } \"small\" } catch e { \"err\" };\n  return v;\n}\nprintln(f(1));\nprintln(f(0));\nreturn 0;\n",
            ),
            new(
                "bool_return",
                "fn f(n: Int) -> Bool {\n  let v = try { if (n > 0) { return true; } false } catch e { false };\n  return v;\n}\nprintln(f(1));\nprintln(f(0));\nreturn 0;\n",
            ),
            // A return *and* a raise from the same body: the two channels must
            // not be confused for each other.
            new(
                "return_or_raise",
                "fn f(n: Int) -> Int {\n  let v = try { if (n > 0) { return 10; } error(\"neg\"); 0 } catch e { -1 };\n  return v;\n}\nprintln(f(1));\nprintln(f(-1));\nreturn 0;\n",
            ),
            // A body where *every* path returns. It has no ok edge in the
            // bytecode (the compiler emits no jump over the handler), which I
            // first read as needing its own protocol — it does not: the ok edge
            // simply always takes the return branch.
            new(
                "every_path_returns",
                "fn f(n: Int) -> Int {\n  try { return n * 2; } catch e { return -1; }\n}\nprintln(f(3));\nreturn 0;\n",
            ),
            new(
                "every_path_returns_or_raises",
                "fn f(n: Int) -> Int {\n  try { if (n < 0) { error(\"neg\"); } return n; } catch e { return -1; }\n}\nprintln(f(3));\nprintln(f(-1));\nreturn 0;\n",
            ),
            // A handler that falls through while the body returns.
            new(
                "body_returns_handler_falls_through",
                "fn f(n: Int) -> Int {\n  try { return n; } catch e { }\n  return 0;\n}\nprintln(f(7));\nreturn 0;\n",
            ),
            // Two returns and a fallthrough in one body.
            new(
                "two_returns_and_a_fallthrough",
                "fn f(n: Int) -> Int {\n  let v = try { if (n > 0) { return n; } if (n < -5) { return -n; } 0 } catch e { -1 };\n  return v;\n}\nprintln(f(3));\nprintln(f(-9));\nprintln(f(-1));\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// `task.join_all` over task handles, pinned to pure Cranelift.
///
/// Variadic, so no ABI row can describe it — a row has one arity — and it was
/// the last thing in the concurrency surface that dropped a program to the VM.
/// The element display is the point of the string case: a `Dyn` list has to
/// quote exactly as the VM's typed list does.
#[test]
fn join_all_over_handles_lowers_natively() {
    run_differential(
        "join_all",
        &[
            new(
                "several_tasks",
                "use task;\nlet a = spawn(|| 1);\nlet b = spawn(|| 2);\nprintln(task.join_all(a, b));\nreturn 0;\n",
            ),
            new(
                "one_task",
                "use task;\nlet a = spawn(|| 1);\nprintln(task.join_all(a));\nreturn 0;\n",
            ),
            new(
                "string_and_mixed_elements",
                "use task;\nlet a = spawn(|| \"x\");\nlet b = spawn(|| \"y z\");\nprintln(task.join_all(a, b));\nlet c = spawn(|| 1);\nlet d = spawn(|| \"s\");\nprintln(task.join_all(c, d));\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// A closure that calls another closure, pinned to pure Cranelift.
///
/// Composing two lambdas is most of what having them is for, and it dropped the
/// whole program to the VM: a captured `f` lives in a *cell*, and what goes into
/// that cell is a lowering-time reference rather than a value, so the store had
/// nothing to read and the callee's capture had nothing to mean.
#[test]
fn a_closure_may_call_another_closure() {
    run_differential(
        "closure_composition",
        &[
            new(
                "compose_two",
                "let f = |x| x + 1;\nlet g = |x| f(x) * 2;\nprintln(g(1));\nreturn 0;\n",
            ),
            // A named `fn` is the same kind of reference.
            new(
                "call_a_named_function",
                "fn inc(x: Int) -> Int { return x + 1; }\nlet g = |x| inc(x) * 2;\nprintln(g(1));\nreturn 0;\n",
            ),
            // Twice in one body, and a three-deep chain.
            new(
                "call_it_twice",
                "let f = |x| x + 1;\nlet g = |x| f(f(x));\nprintln(g(1));\nreturn 0;\n",
            ),
            new(
                "chain_of_three",
                "let f = |x| x + 1;\nlet g = |x| x * 2;\nlet h = |x| g(f(x));\nprintln(h(1));\nreturn 0;\n",
            ),
            // Calling one *and* writing a capture, the two closure facts at once.
            new(
                "call_and_assign_a_capture",
                "let acc = 0;\nlet f = |x| x + 1;\nlet g = |x| { acc = acc + f(x); };\ng(1);\ng(2);\nprintln(acc);\nreturn 0;\n",
            ),
            // A plain alias of a lambda.
            new(
                "alias_a_lambda",
                "let f = |x| x + 1;\nlet g = f;\nprintln(g(1));\nreturn 0;\n",
            ),
            // A lambda *argument* whose body calls a captured lambda. Its whole
            // environment is static, so it is erased and the typed `map_fn` fast
            // path — which calls the callback with the element and nothing else —
            // accepts it.
            new(
                "captured_lambda_inside_a_map_callback",
                "let f = |x| x + 1;\nprintln([1,2,3].map(|x| f(x)));\nprintln([1,2,3].filter(|x| f(x) > 2));\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// Closures that **assign** to what they captured.
///
/// A capture travelled as a hidden trailing argument holding the cell's content
/// at the call site — right for one the body reads, with nowhere to put a
/// write. So an accumulating closure, which is most of what closures are for,
/// dropped the whole program to the VM. Pinned to pure Cranelift: the bug is a
/// silent fallback, so "both print 7" is not the property under test.
#[test]
fn a_closure_may_assign_to_its_capture() {
    run_differential(
        "mutable_captures",
        &[
            new(
                "accumulate_int",
                "let acc = 0;\nlet add = |v| { acc = acc + v; };\nadd(3);\nadd(4);\nprintln(acc);\nreturn 0;\n",
            ),
            // The closure both reads and returns the capture it wrote.
            new(
                "read_write_and_return",
                "let hits = 0;\nlet bump = |n| { hits = hits + n; return hits; };\nprintln(bump(1));\nprintln(bump(2));\nprintln(hits);\nreturn 0;\n",
            ),
            // Non-integer carriers: a string rebuilt, a bool flipped, a float
            // scaled. Each boxes through the cell and comes back.
            new(
                "string_bool_float_captures",
                "let log = \"\";\nlet flag = false;\nlet f = 0.5;\nlet step = |s| { log = log + s + \";\"; flag = !flag; f = f * 2.0; };\nstep(\"a\");\nstep(\"b\");\nprintln(log);\nprintln(flag);\nprintln(f);\nreturn 0;\n",
            ),
            // One capture written, one only read — the read-only one must keep
            // passing by value rather than being dragged into a cell.
            new(
                "written_and_read_only_captures",
                "let base = 10;\nlet total = 0;\nlet add = |v| { total = total + v + base; };\nadd(1);\nadd(2);\nprintln(total);\nprintln(base);\nreturn 0;\n",
            ),
            // Two closures sharing one cell, and a call inside a loop (the
            // write-back has to survive the loop-header phi).
            new(
                "two_closures_one_cell_in_a_loop",
                "let n = 0;\nlet inc = || { n = n + 1; };\nlet dec = || { n = n - 1; };\nfor i in 0..5 { inc(); }\ndec();\nprintln(n);\nreturn 0;\n",
            ),
            // A rebinding write, not a mutation through the handle: the cell
            // carries a whole new list.
            new(
                "capture_rebound_to_a_new_list",
                "let xs = [1];\nlet reset = || { xs = [9, 9]; };\nreset();\nprintln(xs);\nreturn 0;\n",
            ),
            // A closure nested in a closure writes what its parent captured:
            // the cell has to pass *through* the parent by pointer, and the
            // parent's own capture becomes a cell because of the child's write.
            new(
                "nested_closure_writes_the_outer_capture",
                "let total = 0;\nlet outer = |v| {\n  let inner = |w| { total = total + w; };\n  inner(v);\n  inner(v);\n};\nouter(3);\nprintln(total);\nreturn 0;\n",
            ),
            // Three levels: the requirement propagates the whole chain.
            new(
                "three_levels_of_nesting",
                "let total = 0;\nlet l1 = |a| {\n  let l2 = |b| {\n    let l3 = |c| { total = total + c; };\n    l3(b);\n  };\n  l2(a);\n};\nl1(5);\nl1(2);\nprintln(total);\nreturn 0;\n",
            ),
            // The inner closure writes one capture and reads another, so only
            // one of them may become a cell.
            new(
                "nested_writes_one_capture_reads_another",
                "let base = 100;\nlet acc = 0;\nlet outer = |v| {\n  let inner = |w| { acc = acc + w + base; };\n  inner(v);\n};\nouter(1);\nouter(2);\nprintln(acc);\nprintln(base);\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// Both arities of `slice`, in both spellings, pinned to pure Cranelift.
///
/// A string had one- and two-argument forms; a list had only the two-argument
/// one, so `xs.slice(1)` dropped the program to the VM. And `string.slice(s, a,
/// b)` — the *module* spelling of what the method path had always lowered — had
/// no row at all: two spellings of one operation, only one of them fast.
#[test]
fn every_slice_spelling_lowers_natively() {
    run_differential(
        "slice_spellings",
        &[new(
            "list_string_and_bytes",
            "use string;\nuse bytes;\nlet xs = [1,2,3];\nprintln(xs.slice(1));\nprintln(xs.slice(1, 3));\nprintln(string.slice(\"hello\", 1, 3));\nprintln(string.slice(\"hello\", 1));\nprintln(\"hello\".slice(1));\nlet b = bytes.from_string(\"abcde\");\nprintln(b.slice(1));\nreturn 0;\n",
        )],
        NativePath::PureCranelift,
    );
}

/// `Bytes` as a native value, pinned to pure Cranelift.
///
/// It had no carrier at all, so `"hi".bytes()`, every `bytes` module member, and
/// `base64.decode` / `hex.decode` dropped the whole program to the VM. Display
/// (`Bytes([104,105])`) and equality (by *content*) are the two things a bare
/// handle integer could not have expressed.
#[test]
fn bytes_are_a_native_value() {
    run_differential(
        "bytes_value",
        &[
            new(
                "module_surface",
                "use bytes;\nlet b = bytes.from_string(\"hi\");\nprintln(b);\nprintln(bytes.len(b));\nprintln(bytes.is_empty(b));\nprintln(bytes.to_string_utf8(b));\nprintln(bytes.to_string_lossy(b));\nprintln(bytes.get(b, 0) ?? -1);\nprintln(bytes.get(b, -1) ?? -1);\nprintln(bytes.get(b, 9) ?? -1);\nprintln(bytes.concat(b, b));\nreturn 0;\n",
            ),
            // The method spellings, and `len` through the container fast path.
            new(
                "method_surface",
                "println(\"hi\".bytes());\nprintln(\"hi\".bytes().len());\nprintln(\"\".bytes().is_empty());\nprintln(\"\".bytes());\nreturn 0;\n",
            ),
            // Indexing, which is also what `b.get(i)` compiles to, and the
            // two-argument `slice`. Negative counts from the end and out of
            // range is nil — the same rule every container reads by, which the
            // `bytes` *module* did not have until now (`b.slice(1, -1)` through
            // the method dispatch answered while `bytes.slice(b, 1, -1)` raised).
            new(
                "indexing_and_slicing",
                "use bytes;\nlet b = bytes.from_string(\"abcde\");\nprintln(b[0]);\nprintln(b[-1]);\nprintln(b[9] ?? -1);\nprintln(b.get(-1) ?? -1);\nprintln(bytes.slice(b, 1));\nprintln(bytes.slice(b, 1, -1));\nprintln(b.slice(1, -1));\nreturn 0;\n",
            ),
            // List interop, the last two members of the module — and the
            // out-of-range raise, which is the whole point of `from_list`
            // taking bytes rather than truncating whatever it is handed.
            new(
                "list_interop",
                "use bytes;\nlet b = bytes.from_list([104,105]);\nprintln(b);\nprintln(bytes.to_list(b));\nprintln(bytes.from_list([]));\nprintln(bytes.to_list(bytes.from_string(\"\")));\ntry { bytes.from_list([300]); println(\"no\"); } catch e { println(\"caught\"); }\ntry { bytes.from_list([-1]); println(\"no\"); } catch e { println(\"caught\"); }\nreturn 0;\n",
            ),
            // Content equality, not handle identity.
            new(
                "content_equality",
                "use bytes;\nlet a = bytes.from_string(\"hi\");\nprintln(a == bytes.from_string(\"hi\"));\nprintln(a == bytes.from_string(\"ho\"));\nprintln(a != bytes.from_string(\"ho\"));\nreturn 0;\n",
            ),
            // The decoders answer `Bytes`, and raise catchably on bad input.
            new(
                "decoders_answer_bytes",
                "use bytes;\nuse encoding;\nlet d = encoding.base64.decode(\"aGk=\");\nprintln(d);\nprintln(bytes.to_string_utf8(d));\nprintln(encoding.hex.decode(\"6869\") == d);\ntry { encoding.base64.decode(\"!!!\"); println(\"no\"); } catch e { println(\"caught\"); }\ntry { encoding.hex.decode(\"zz\"); println(\"no\"); } catch e { println(\"caught\"); }\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// `base64` / `hex` / `url` text, pinned to pure Cranelift.
///
/// lkrt uses the same crates the stdlib module does, so the bytes are identical
/// rather than merely equivalent — and the `url` pair is the one that had to be
/// fixed before it could be mirrored: the encoder was form-encoding (a space
/// became `+`) while the decoder only undid `%XX`, so it did not round-trip.
#[test]
fn text_codecs_lower_natively() {
    run_differential(
        "text_codec",
        &[
            new(
                "base64_and_hex_encode",
                "use encoding;\nprintln(encoding.base64.encode(\"hi\"));\nprintln(encoding.hex.encode(\"hi\"));\nreturn 0;\n",
            ),
            new(
                "url_component_round_trip",
                "use encoding;\nlet s = \"a b&c=d\";\nlet e = encoding.url.encode_component(s);\nprintln(e);\nprintln(encoding.url.decode_component(e));\nprintln(encoding.url.decode_component(e) == s);\nreturn 0;\n",
            ),
            // A malformed escape raises, catchably, with the same three messages.
            new(
                "url_decode_raises_on_a_bad_escape",
                "use encoding;\ntry { println(encoding.url.decode_component(\"%\")); } catch e { println(\"caught1\"); }\ntry { println(encoding.url.decode_component(\"%zz\")); } catch e { println(\"caught2\"); }\nprintln(encoding.url.decode_component(\"%41\"));\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// A submodule reached through its parent, pinned to pure Cranelift.
///
/// `encoding.json.parse(s)` compiles to a `CallMethodK` whose *receiver* is the
/// module object `encoding.json`, and two things were missing: reading a
/// submodule off its parent gave a module *function* rather than another module,
/// and a module-object receiver had no arm at all. So the chain stopped at the
/// first dot and the program fell back, while `use { json } from encoding;`
/// lowered — same answer, three times slower, which no differential test can
/// see.
#[test]
fn a_submodule_reached_through_its_parent_lowers_natively() {
    run_differential(
        "nested_module",
        &[
            new(
                "encoding_json_through_its_parent",
                "use encoding;\nprintln(encoding.json.parse(\"[1,2]\"));\nreturn 0;\n",
            ),
            // The same member through the selective import, which always
            // lowered: both spellings, one answer.
            new(
                "encoding_json_through_a_selective_import",
                "use { json } from encoding;\nprintln(json.parse(\"[1,2]\"));\nreturn 0;\n",
            ),
            // `io.std` is the other shape: a submodule whose parent had no row
            // at all, so even the name did not bind.
            new(
                "io_std_through_its_parent",
                "use io;\nlet out = io.std.stdout();\nprintln(io.std.write(out, \"a\"));\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// The `chan` module's own spelling, pinned to pure Cranelift.
///
/// The surrounding channel cases allow degradation because raises through
/// `select` are recorded debt; these must not, because the bug they cover is
/// exactly a *silent* drop to the VM. `chan` is both a builtin constructor and
/// a module under one name, `builtin_for_name` claimed it first, and the member
/// read found no value — so `chan.new(1)` fell back while `chan(1)` lowered,
/// with both printing the same answer.
#[test]
fn chan_module_lowers_natively() {
    run_differential(
        "chan_module",
        &[
            // The module spelling of the whole channel surface, including the
            // blocking pair the module used to lack: `chan` resolves to a
            // builtin constructor *and* a module under the same name, and the
            // member read was losing to the constructor, so `chan.new(1)` was
            // dropping the program to the VM while `chan(1)` lowered.
            new(
                "chan_module_spelling_blocking_and_polling",
                "use chan;\nlet c = chan.new(2);\nchan.send(c, 7);\nprintln(chan.try_send(c, 8));\nprintln(chan.recv(c));\nprintln(chan.try_recv(c) ?? -1);\nprintln(chan.len(c));\nprintln(chan.capacity(c));\nprintln(chan.is_closed(c));\nchan.close(c);\nprintln(chan.is_closed(c));\nreturn 0;\n",
            ),
            // `0` is unbuffered, not unbounded — lkrt had kept the retired rule,
            // and answered `true` to both sends.
            new(
                "chan_capacity_zero_is_unbuffered",
                "use chan;\nlet c = chan.new(0);\nprintln(chan.try_send(c, 1));\nprintln(chan.try_send(c, 2));\nprintln(chan.len(c));\ntry { chan.new(-1); println(\"no\"); } catch e { println(\"caught\"); }\nreturn 0;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// `as` casts, with the native path pinned: the point of these is that the two
/// backends agree *bit for bit*, not merely that both produce something.
///
/// The VM masks inside its `i64` carrier and sign-extends back; Cranelift does
/// `ireduce` then `sextend`/`uextend`. Those are different mechanisms, so this
/// is where a divergence would show up.
/// `try` regions that lower natively: the body becomes a function, and the
/// `setjmp` happens in `lkrt`'s C frame because Cranelift cannot emit one.
///
/// Both outcomes are here. A body that raises must reach the handler with the
/// raised value intact, and a body that returns must skip it — a region that
/// only ever worked on one of those paths would pass half a test.
#[test]
fn try_region_differential() {
    run_differential(
        "try_region",
        &[
            new(
                "caught",
                "fn boom() { error(404); return 0; }\nlet b = 0;\ntry { boom(); } catch code { b = code; }\nreturn b;\n",
            ),
            new(
                "not_raised",
                "fn fine() { return 7; }\nlet b = 0;\ntry { fine(); } catch e { b = 1; }\nreturn b;\n",
            ),
            // The body reads the enclosing function's locals. They are its
            // *parameters* once it is outlined, discovered by lowering it and
            // seeing which registers had no definition inside — so an argument
            // arriving in the wrong order or under the wrong number shows up
            // here as the wrong branch being taken.
            new(
                "reads_outer_locals",
                "fn checked(a: Int, b: Int) -> Int {\n  if (b == 0) { error(\"zero\"); }\n  return a - b;\n}\n\
                 let x = 10;\nlet y = 0;\nlet out = 0;\ntry { checked(x, y); } catch e { out = 1; }\nreturn out;\n",
            ),
            new(
                "reads_outer_locals_no_raise",
                "fn checked(a: Int, b: Int) -> Int {\n  if (b == 0) { error(\"zero\"); }\n  return a - b;\n}\n\
                 let x = 10;\nlet y = 3;\nlet out = 0;\ntry { checked(x, y); } catch e { out = 1; }\nreturn out;\n",
            ),
            // The body *assigns* an enclosing local. It cannot travel in a
            // register — the body runs in a frame of its own — so it goes
            // through a cell, written as the body goes rather than on the way
            // out: a raise half way through must leave behind what was already
            // assigned, which is what the VM shows.
            new(
                "writes_outer_local",
                "fn fine() { return 7; }\nlet a = 0;\ntry { fine(); a = 1; } catch e { a = 2; }\nreturn a;\n",
            ),
            new(
                "writes_then_raises",
                "fn boom() { error(\"x\"); return 0; }\nlet a = 0;\ntry { a = 5; boom(); a = 9; } catch e { }\nreturn a;\n",
            ),
            // Not just integers: what crosses back out of a cell is decided per
            // type, and a type with no unboxer rejects rather than guesses.
            new(
                "writes_outer_bool",
                "fn boom() { error(\"x\"); return 0; }\nlet flag = false;\n\
                 try { flag = true; boom(); } catch e { }\nif (flag) { return 1; }\nreturn 0;\n",
            ),
            new(
                "writes_outer_string",
                "fn boom() { error(\"x\"); return 0; }\nlet s = \"before\";\n\
                 try { s = \"during\"; boom(); } catch e { }\nreturn s.len();\n",
            ),
            // A raise from two frames down still lands in the nearest handler:
            // the trampoline's frame is what `longjmp` targets, not the body's.
            new(
                "deep",
                "fn inner() { error(\"deep\"); return 0; }\nfn outer() { return inner(); }\n\
                 let b = 0;\ntry { outer(); } catch e { b = 1; }\nreturn b;\n",
            ),
        ],
        NativePath::PureCranelift,
    );
}

/// Function pointers: an exported function's address, and a call through it.
///
/// Not a *differential* test in the usual sense — the VM refuses both builtins,
/// because an interpreter has no code addresses to hand out and returning a
/// fake one would produce a program that runs interpreted and jumps into
/// nothing when compiled. What is checked is that the native side computes the
/// answer, which is the whole of the feature: a driver table is an array of
/// these.
#[test]
fn function_pointers_are_native_only() {
    use std::process::Command;

    let dir = std::env::temp_dir().join(format!("lk_fnptr_{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let source = dir.join("fnptr.lk");
    fs::write(
        &source,
        "#[export(\"probe_add\")]\nfn probe_add(a: Int, b: Int) -> Int {\n    return a + b;\n}\n\n\
         let p = unsafe { symbol_address(\"probe_add\") };\nprintln(unsafe { call_address_2(p, 20, 22) });\n",
    )
    .expect("write source");

    // The VM refuses, by name.
    let vm = Command::new(env!("CARGO_BIN_EXE_lk"))
        .arg(&source)
        .output()
        .expect("run vm");
    let message = String::from_utf8_lossy(&vm.stderr);
    assert!(!vm.status.success(), "the VM must refuse: {message}");
    assert!(
        message.contains("symbol_address requires native compilation"),
        "the refusal must name the builtin: {message}"
    );

    // Compiled, it answers.
    let exe = dir.join("fnptr");
    let compile = Command::new(env!("CARGO_BIN_EXE_lk"))
        .args(["compile"])
        .arg(&source)
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .output()
        .expect("compile");
    assert!(
        compile.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&exe).output().expect("run native");
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "42");
    let _ = fs::remove_dir_all(&dir);
}

/// `<<` and `>>`, which lower to the range-checked `lkrt` helpers rather than
/// to a machine shift. Both halves matter: the values have to agree, and so
/// does the *failure* — a shift amount out of range raises on both sides, and
/// masking it (what the hardware would do) would show up here as a native run
/// that succeeded where the VM refused.
#[test]
fn shift_differential() {
    run_differential(
        "shift",
        &[
            new("shl_const", "return 3 << 8;\n"),
            new("shr_const", "return 1024 >> 5;\n"),
            // Arithmetic, not logical: the sign bit is replicated.
            new("shr_negative", "return (0 - 16) >> 2;\n"),
            // Variable amounts: the value is not a constant the lowering can fold.
            new("shl_variable", "let n = 5;\nreturn 1 << n;\n"),
            new("shr_variable", "let n = 3;\nreturn 4096 >> n;\n"),
            // Precedence: tighter than comparison, looser than `+` (Rust's).
            new("precedence_add", "return 1 << 2 + 3;\n"),
            new("precedence_cmp", "if (8 >> 1 == 4) { return 1; }\nreturn 0;\n"),
            // Mixed with the other bitwise operators, which lower to machine
            // instructions — so this is the two paths meeting.
            new("with_mask", "let v = 0xdeadbeef;\nreturn (1 << 12) - 1 & v;\n"),
            // The edges of the accepted range.
            new("shl_zero", "return 7 << 0;\n"),
            new("shl_63", "return 1 << 63;\n"),
            // Out of range: both sides must refuse, not mask.
            new("shl_out_of_range", "let n = 64;\nreturn 1 << n;\n"),
            new("shr_negative_amount", "let n = 0 - 1;\nreturn 1 >> n;\n"),
        ],
        NativePath::PureCranelift,
    );
}

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
/// access.
///
/// The failure it guards is not hypothetical and not historical: removing the
/// `sequence_point` that `Inst::VolatileLoad` emits still compiles this to one
/// `mov` and a `lea` doubling it. Cranelift has no volatile flag; what keeps
/// both accesses is that its alias analysis keys every access by the last store
/// before it, and a sequence point — which assembles to nothing — moves that
/// key. So the assertion counts *instructions*, which is the thing at risk. It
/// used to count calls into `lkrt`, back when a device read was a call.
#[test]
fn volatile_reads_are_not_collapsed() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("volatile_twice.lk");
    std::fs::write(
        &source,
        "fn read_twice(addr: usize) -> Int {\n\
         \x20   let reg = addr as *mut u32;\n\
         \x20   let a = unsafe { volatile_read_u32(reg) } as Int;\n\
         \x20   let b = unsafe { volatile_read_u32(reg) } as Int;\n\
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
    let accesses = count_loads(&body);
    assert_eq!(accesses, 2, "expected two volatile reads, got {accesses}:\n{body}");
}

/// Volatile writes to *different* addresses keep their order.
///
/// The two tests around this one guard against accesses being *removed*. This
/// one guards the property `drivers/e1000.lk` calls "the entire transmit
/// protocol": a descriptor is filled in, and only then is the card's tail
/// register bumped to tell it to look. Swap those and the card transmits a
/// descriptor that was not finished being written — on real hardware, and quite
/// possibly not in QEMU, which is the worst way for a bug to be shaped.
///
/// x86-64 does not reorder stores in hardware, so what is being pinned here is
/// the *compiler* half: nothing in the MIR passes or in Cranelift's scheduling
/// may move one volatile store past another. It holds today; nothing else
/// notices if it stops.
#[test]
fn volatile_writes_keep_their_order() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("volatile_order.lk");
    // Distinct immediates, so the order is readable off the disassembly without
    // having to decode which address each store names.
    std::fs::write(
        &source,
        "#[export]\n\
         fn tx(desc: usize, tail: usize) -> Int {\n\
         \x20   unsafe { volatile_write_u64(desc as *mut u64, 0x1111 as u64); };\n\
         \x20   unsafe { volatile_write_u32((desc + 8) as *mut u32, 0x2222 as u32); };\n\
         \x20   unsafe { volatile_write_u32(tail as *mut u32, 0x3333 as u32); };\n\
         \x20   return 0;\n\
         }\n\
         println(0);\n",
    )
    .expect("write source");

    let exe = dir.path().join("volatile_order");
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
        .args(["compile", source.to_str().expect("utf-8 path")])
        .arg("--output")
        .arg(exe.to_str().expect("utf-8 path"))
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .status()
        .expect("run lk compile");
    assert!(status.success(), "volatile must lower natively");

    let Ok(disassembly) = std::process::Command::new("objdump")
        .args(["-d", exe.to_str().expect("utf-8 path")])
        .output()
    else {
        // objdump is not everywhere; the compile above is still meaningful.
        return;
    };
    let text = String::from_utf8_lossy(&disassembly.stdout);
    // `#[export]` so the symbol is the source's own name rather than a numbered
    // one that renumbers whenever a function is added above it.
    let body: String = text
        .lines()
        .skip_while(|line| !line.contains("<tx>:"))
        .skip(1)
        .take_while(|line| !line.trim().is_empty() && !line.contains(">:"))
        .collect::<Vec<_>>()
        .join("\n");

    let positions: Vec<Option<usize>> = ["0x1111", "0x2222", "0x3333"]
        .iter()
        .map(|needle| body.find(needle))
        .collect();
    for (needle, position) in ["0x1111", "0x2222", "0x3333"].iter().zip(&positions) {
        assert!(position.is_some(), "{needle} was not written at all:\n{body}");
    }
    let positions: Vec<usize> = positions.into_iter().flatten().collect();
    assert!(
        positions[0] < positions[1] && positions[1] < positions[2],
        "the three volatile writes were reordered:\n{body}"
    );
}

/// Two identical writes to one address must stay two writes.
///
/// The mirror image of `volatile_reads_are_not_collapsed`, and a distinct
/// mechanism: what eliminates this one is the alias pass's *idempotent store*
/// rule, which drops a store of a value the location is already known to hold.
/// It compares SSA values, not runtime ones, so two `write(port, 7)` lines in a
/// row are exactly its target — and a command register that counts writes is
/// exactly the device for which one write is not two. Measured: without the
/// sequence point, `movb $0x7,(%rdi)` is emitted once for a source that says it
/// twice.
#[test]
fn identical_volatile_writes_are_not_collapsed() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("volatile_write_twice.lk");
    std::fs::write(
        &source,
        "fn kick_twice(addr: usize) {\n\
         \x20   let reg = addr as *mut u8;\n\
         \x20   unsafe { volatile_write_u8(reg, 7 as u8); };\n\
         \x20   unsafe { volatile_write_u8(reg, 7 as u8); };\n\
         }\n\
         kick_twice(0x1000);\n\
         return 0;\n",
    )
    .expect("write source");

    let exe = dir.path().join("volatile_write_twice");
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
        .args(["compile", source.to_str().expect("utf-8 path")])
        .arg("--output")
        .arg(exe.to_str().expect("utf-8 path"))
        .env("LK_AOT_NO_FALLBACK", "1")
        .env("LK_AOT_HYBRID", "0")
        .status()
        .expect("run lk compile");
    assert!(status.success(), "volatile writes must lower natively");

    let Some(body) = native_body(&exe, "lk_fn_1") else {
        return; // objdump is not everywhere; the compile above still ran.
    };
    let writes = count_stores(&body);
    assert_eq!(writes, 2, "expected two volatile writes, got {writes}:\n{body}");
}

/// The disassembly of one function of a compiled executable, or `None` when
/// there is no `objdump` to ask.
fn native_body(exe: &std::path::Path, symbol: &str) -> Option<String> {
    let output = std::process::Command::new("objdump")
        .args(["-d", exe.to_str().expect("utf-8 path")])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let marker = format!("<{symbol}>:");
    Some(
        text.lines()
            .skip_while(|line| !line.contains(&marker))
            .take_while(|line| !line.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// Counts loads through a bare register-indirect address — `mov (%rdi),%esi`
/// and its width variants.
///
/// Deliberately narrow. A device access is emitted as exactly this shape, while
/// the prologue and epilogue move between registers (`mov %rsp,%rbp`) and a
/// spill would carry a frame-pointer offset. Counting every `mov` would pass
/// for the wrong reason.
fn count_loads(body: &str) -> usize {
    body.lines()
        .filter_map(mov_operands)
        .filter(|operands| operands.starts_with("(%r") && operands.contains("),%"))
        .count()
}

/// The operand text of one `objdump -d` line, when its mnemonic is a `mov`.
///
/// The line is `address:\tbytes\tmnemonic operands`, so the instruction is the
/// last tab-separated field and the operands are what follows its first run of
/// whitespace. Splitting on the *first* tab instead lands in the middle of the
/// raw bytes, which is a silent zero rather than an error.
///
/// The `mov` restriction is not decoration: `lea (%r8,%rdi,1),%rax` has operands
/// shaped exactly like a load's and touches no memory at all. Matching on the
/// operand shape alone counted the address arithmetic that *follows* two device
/// reads as a third read.
fn mov_operands(line: &str) -> Option<&str> {
    let instruction = line.rsplit('\t').next()?.trim();
    let (mnemonic, rest) = instruction.split_once(char::is_whitespace)?;
    mnemonic.starts_with("mov").then(|| rest.trim())
}

/// Counts stores to a bare register-indirect address — `movb $0x7,(%rdi)`.
fn count_stores(body: &str) -> usize {
    body.lines()
        .filter_map(mov_operands)
        .filter(|operands| operands.ends_with(')') && operands.contains(",(%r"))
        .count()
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
         \x20   let v = unsafe { volatile_read_u32(reg) } as Int;\n\
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

    // Two of the five are instructions rather than calls, which is the whole
    // change: a device access no longer goes through `lkrt`. What is being
    // checked is unchanged — that the mask, the write, the barrier, the read
    // and the restore appear in the order the source puts them.
    enum Step {
        Call(&'static str),
        Load,
        Store,
    }
    let expected = [
        Step::Call("lkrt_cpu_irq_save"),
        Step::Store,
        Step::Call("lkrt_cpu_barrier"),
        Step::Load,
        Step::Call("lkrt_cpu_irq_restore"),
    ];
    let mut remaining = expected.iter();
    let mut wanted = remaining.next();
    for line in &body {
        let matched = match wanted {
            Some(Step::Call(name)) => line.contains(name),
            Some(Step::Load) => count_loads(line) == 1,
            Some(Step::Store) => count_stores(line) == 1,
            None => false,
        };
        if matched {
            wanted = remaining.next();
        }
    }
    let wanted = wanted.map(|step| match step {
        Step::Call(name) => name,
        Step::Load => "a load through a register-indirect address",
        Step::Store => "a store through a register-indirect address",
    });
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

/// `#[extern]` names a function implemented outside the program.
///
/// The mirror of `#[export]`. A native build calls the symbol and never emits
/// the body; the interpreter, which cannot reach outside, runs the body. That
/// asymmetry is the point and also the cost: this is the one construct whose
/// two back ends are not checked against each other, because the thing being
/// called is not in the program.
#[test]
fn an_extern_function_calls_the_named_symbol() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("ext.lk");
    std::fs::write(
        &source,
        "#[extern(\"kernel_double\")]\n\
         fn kernel_double(value: Int) -> Int { return value * 2; }\n\
         println(kernel_double(21));\n\
         return 0;\n",
    )
    .expect("write source");

    // The interpreter runs the body.
    let vm = Command::new(bin_path())
        .current_dir(dir.path())
        .arg("ext.lk")
        .output()
        .expect("spawn vm run");
    assert!(
        vm.status.success(),
        "vm run failed: {}",
        String::from_utf8_lossy(&vm.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&vm.stdout), "42\n0\n");

    // The object refers to the symbol and leaves it to the linker.
    let object = dir.path().join("ext.o");
    let compile = Command::new(bin_path())
        .current_dir(dir.path())
        .args(["compile", "object:x86_64-unknown-none", "ext.lk"])
        .arg("--output")
        .arg(&object)
        .output()
        .expect("spawn object compile");
    assert!(
        compile.status.success(),
        "an extern call must lower: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let bytes = std::fs::read(&object).expect("read object");
    let needle = b"kernel_double";
    assert!(
        bytes.windows(needle.len()).any(|window| window == needle),
        "the object should name the symbol it calls"
    );
}

/// `join` writes every element the way the language writes it.
///
/// It used to raise "ListJoin list must contain only strings" for any carrier
/// but `Str` — so `[1, 2].join(",")` type-checked and failed at run time, while
/// `"${[1, 2]}"` had been printing `[1,2]` all along. Worse, the AOT lowering
/// declined `join` on the numeric carriers *citing that rule*, which is how one
/// arbitrary restriction becomes two.
///
/// The values below are the ones where two renderers drift apart if there are
/// two: a float that is integral (`2.0` → `2`), a negative zero, an exponent,
/// and a NaN. Both ends go through the renderer their own display path uses, so
/// this is the test that says they are the same renderer.
#[test]
fn join_covers_every_carrier_and_agrees_on_how_a_value_looks() {
    run_clif_differential(
        "list_join",
        &[
            new(
                "join_each_carrier",
                "println([1, 2, 3].join(\"-\"));\nprintln([1.5, 2.0].join(\",\"));\n\
                 println([\"a\", \"b\"].join(\", \"));\nprintln([1, \"a\", nil, true].join(\"|\"));\n\
                 return 0;\n",
            ),
            new(
                "join_of_an_empty_list_is_an_empty_string",
                "println([].join(\",\"));\nprintln([1].join(\",\"));\n\
                 println(([1, 2].join(\"\")).len());\nreturn 0;\n",
            ),
            // Where a second renderer would show: integral floats, signed zero,
            // exponents, NaN and infinity.
            new(
                "join_writes_floats_the_way_display_does",
                "println([2.0, -0.0, 1e20, 1e-7].join(\" \"));\n\
                 println([0.0 / 0.0, 1.0 / 0.0, -1.0 / 0.0].join(\" \"));\nreturn 0;\n",
            ),
            // The separator is not a delimiter the elements may contain.
            new(
                "a_separator_that_occurs_in_the_elements",
                "println([\"a,b\", \"c\"].join(\",\"));\nprintln([11, 1].join(\"1\"));\nreturn 0;\n",
            ),
        ],
    );
}

/// The carrier a list happens to have does not decide which methods stay native.
///
/// A sweep of every list method against every carrier found three holes, each of
/// a different kind:
///
/// * `contains` on `Str` — `str_contains` was declared in the ABI and reached
///   only from the `in` operator, so `"a" in xs` lowered and `xs.contains("a")`
///   did not. The comment beside those arms states the invariant it broke: a
///   carrier whose `index_of` lowers and whose `contains` does not makes
///   `xs.contains(v)` and `xs.index_of(v) != nil` disagree about which programs
///   stay native.
/// * two-argument `slice` — only `Int` had it, and the rule it needs (negative
///   counts from the tail, everything clamps) now lives in one `slice_bounds`
///   that all four carriers share, rather than being written out four times.
/// * `flatten` — only the boxed carrier. A typed list cannot nest, so flatten
///   there is a copy, which `slice_from(0)` already is.
#[test]
fn every_carrier_answers_contains_slice_and_flatten() {
    run_clif_differential(
        "list_carrier_parity",
        &[
            new(
                "contains_on_every_carrier",
                "let n = 3;\nlet i = [1, 2, n];\nlet f = [1.5, 2.5, 3.5];\nlet s = [\"a\", \"b\"];\n\
                 println(i.contains(2));\nprintln(f.contains(9.5));\nprintln(s.contains(\"b\"));\n\
                 println(s.contains(\"z\"));\nreturn 0;\n",
            ),
            // The spelling that already lowered, so the two agree.
            new(
                "contains_agrees_with_the_in_operator",
                "let s = [\"a\", \"b\"];\nprintln(s.contains(\"a\") == (\"a\" in s));\n\
                 println(s.contains(\"z\") == (\"z\" in s));\nreturn 0;\n",
            ),
            // Every branch of the shared bounds rule: negative, clamped, inverted.
            new(
                "two_argument_slice_on_every_carrier",
                "let n = 4;\nlet i = [1, 2, 3, n];\nlet f = [1.5, 2.5, 3.5];\nlet s = [\"a\", \"b\", \"c\"];\n\
                 println(i.slice(1, 3));\nprintln(f.slice(0, 2));\nprintln(s.slice(1, 3));\n\
                 println(s.slice(-2, 3));\nprintln(f.slice(0, 99));\nprintln(i.slice(3, 1));\n\
                 println(s.slice(-99, 99));\nreturn 0;\n",
            ),
            // A typed list has nothing to flatten, and the result is a copy: the
            // receiver must not move when the answer is pushed to.
            new(
                "flatten_of_a_typed_list_copies_it",
                "let n = 3;\nlet xs = [1, 2, n];\nlet ys = xs.flatten();\nys.push(9);\n\
                 println(xs);\nprintln(ys);\nprintln([\"a\"].flatten());\n\
                 println([1.5].flatten());\nreturn 0;\n",
            ),
        ],
    );
}

/// `m.clear()` lowers, like the list's and the set's.
///
/// It was the one container method the map lacked natively, so a function using
/// it dropped to the VM for a reason no program can see. `Map<str, bool>` rides
/// the `str_i64` carrier, so five helpers cover the six map types the MIR
/// distinguishes — and every one of them is exercised here, because a carrier
/// wired to the wrong helper would still compile.
#[test]
fn clear_lowers_on_every_map_carrier() {
    run_clif_differential(
        "map_clear",
        &[
            new(
                "clear_each_carrier",
                "let n = 2;\nlet si = {\"a\": 1, \"b\": n};\nlet sf = {\"a\": 1.5, \"b\": 2.5};\n\
                 let sb = {\"a\": true, \"b\": false};\nlet ii = {1: 10, 2: 20};\nlet if_ = {1: 1.5, 2: 2.5};\n\
                 si.clear();\nsf.clear();\nsb.clear();\nii.clear();\nif_.clear();\n\
                 println(si.len());\nprintln(sf.len());\nprintln(sb.len());\n\
                 println(ii.len());\nprintln(if_.len());\nreturn 0;\n",
            ),
            // Clearing is in place: the receiver is empty afterwards, and still
            // usable.
            new(
                "a_cleared_map_is_empty_and_still_a_map",
                "let n = 2;\nlet m = {\"a\": 1, \"b\": n};\nprintln(m.len());\nm.clear();\n\
                 println(m.len());\nprintln(m.has(\"a\"));\nm[\"c\"] = 7;\n\
                 println(m.len());\nprintln(m[\"c\"]);\nreturn 0;\n",
            ),
        ],
    );
}

/// Which spelling you use, and which carrier the list happens to have, do not
/// decide whether a program stays native.
///
/// A full sweep of the 29 declared list methods against the four carriers left
/// two holes after the earlier round:
///
/// * `concat` — the same operation as `chain` under a second name, with its own
///   two narrower arms. `xs.chain(ys)` lowered on all four carriers and
///   `xs.concat(ys)` on two, so the choice of word decided the outcome. Both
///   narrow arms were subsumed by the general one; deleting them is the fix.
/// * `xs[i] = v` — `Int` and `Float` had arms, `Str` and the boxed carrier did
///   not, and `xs.set(i, v)` is the same opcode, so both spellings fell together.
///
/// The out-of-bounds store is included because it is the one place these can
/// disagree loudly: the VM halts, and every carrier's helper has to halt with
/// the same words.
#[test]
fn concat_and_index_assignment_do_not_depend_on_the_carrier() {
    run_clif_differential(
        "list_carrier_parity_2",
        &[
            new(
                "concat_agrees_with_chain_on_every_carrier",
                "let n = 3;\nlet i = [1, n];\nlet f = [1.5, 2.5];\nlet s = [\"a\", \"b\"];\n\
                 println(i.concat(i) == i.chain(i));\nprintln(f.concat(f) == f.chain(f));\n\
                 println(s.concat(s) == s.chain(s));\nprintln(s.concat(s));\n\
                 println(f.concat(f));\nreturn 0;\n",
            ),
            new(
                "index_assignment_on_every_carrier",
                "let n = 3;\nlet i = [1, 2, n];\nlet f = [1.5, 2.5];\nlet s = [\"a\", \"b\"];\n\
                 i[0] = 9;\nf[1] = 9.5;\ns[0] = \"z\";\ns.set(1, \"y\");\n\
                 println(i);\nprintln(f);\nprintln(s);\n\
                 s[-1] = \"tail\";\nprintln(s);\nreturn 0;\n",
            ),
            // A store past the end halts on both ends, with the same words.
            new(
                "a_store_out_of_bounds_halts_the_same_way",
                "let s = [\"a\"];\nprintln(try { s[5] = \"x\"; \"no\" } catch e { \"caught: ${e}\" });\n\
                 println(try { s[-9] = \"x\"; \"no\" } catch e { \"caught: ${e}\" });\n\
                 println(s);\nreturn 0;\n",
            ),
        ],
    );
}

/// `Bytes` is a receiver kind, not four methods and a carrier.
///
/// It had `len`, `is_empty`, `get` and `slice`; the other ten of its fourteen
/// declared methods dropped the whole module to the VM. A receiver that is
/// *almost* native is the shape a coverage percentage cannot show — the corpus
/// compiles, the number stays 60/60, and every program touching bytes is slow.
///
/// `first`/`last` reuse `get` (a negative position already counts from the end).
/// `take`/`skip` do *not* reuse `slice`: a count is not a position, so a negative
/// one is the loud error the VM gives rather than something measured from the
/// tail — which is exactly what the last two cases here pin. `index_of` answers
/// nil on a miss, never -1, because -1 is a legal position and
/// `b[b.index_of(v)]` would quietly read the last byte instead of failing.
#[test]
fn bytes_answers_its_whole_method_surface_natively() {
    run_clif_differential(
        "bytes_methods",
        &[
            new(
                "reads_and_windows",
                "let b = \"abcde\".bytes();\nprintln(b.len());\nprintln(b.first());\n\
                 println(b.last());\nprintln(b.take(2));\nprintln(b.skip(2));\n\
                 println(b.take(99));\nprintln(b.skip(99));\nprintln(b.to_list());\n\
                 println(b.slice(1, 3));\nreturn 0;\n",
            ),
            new(
                "membership_answers_nil_on_a_miss",
                "let b = \"abc\".bytes();\nprintln(b.contains(97));\nprintln(b.contains(122));\n\
                 println(b.index_of(98));\nprintln(b.index_of(122));\n\
                 println(b.index_of(-1));\nprintln(b.index_of(300));\n\
                 println(b.contains(300));\nreturn 0;\n",
            ),
            new(
                "an_empty_bytes_reads_as_nil",
                "let b = \"\".bytes();\nprintln(b.len());\nprintln(b.is_empty());\n\
                 println(b.first());\nprintln(b.last());\nprintln(b.take(3));\n\
                 println(b.index_of(97));\nreturn 0;\n",
            ),
            // `map`/`filter`/`reduce` reach the callback channel by *becoming* an
            // `Int` list first — byte values lose nothing in the conversion. The
            // shapes differ on the way back and that asymmetry is the VM's:
            // `map` may produce anything so it answers a list, `filter` only
            // removes so it answers `Bytes`, `reduce` answers a scalar.
            new(
                "closures_over_bytes_keep_the_vm_result_shapes",
                "let b = \"abc\".bytes();\nprintln(b.map(|v| v + 1));\nprintln(b.filter(|v| v > 97));\n\
                 println(b.reduce(0, |a, x| a + x));\nprintln(b.filter(|v| false));\n\
                 println(b.map(|v| v * 2).len());\nprintln(\"\".bytes().map(|v| v));\n\
                 println(\"\".bytes().reduce(7, |a, x| a + x));\nreturn 0;\n",
            ),
            // A count is not a position: negative raises, on both ends, with the
            // same words.
            new(
                "a_negative_count_is_the_same_loud_error",
                "let b = \"abc\".bytes();\n\
                 println(try { b.take(-1); \"no\" } catch e { \"caught: ${e}\" });\n\
                 println(try { b.skip(-2); \"no\" } catch e { \"caught: ${e}\" });\nreturn 0;\n",
            ),
        ],
    );
}

/// `s.values()` is the members in iteration order, and it lowers.
///
/// It was the one Set method with no arm, so a function calling it dropped to
/// the VM while a `for` loop over the same set stayed native — two ways of
/// asking for the same sequence, one of them native. `set.iter` already builds
/// exactly that list; the order is a hash order, so this rides the same mirror
/// discipline that makes set iteration lowerable at all.
#[test]
fn set_values_is_the_iteration_order_and_lowers() {
    run_clif_differential(
        "set_values",
        &[
            new(
                "values_agrees_with_iteration",
                "let s = Set([5, 1, 9, 3, 7, 2]);\nlet out = [];\nfor x in s { out.push(x); }\n\
                 println(s.values() == out);\nprintln(s.values().len());\n\
                 println(Set([]).values());\nreturn 0;\n",
            ),
            // Mixed kinds, because the order spans them.
            new(
                "values_over_mixed_members",
                "let s = Set([1, \"a\", true, nil]);\nprintln(s.values().len());\n\
                 println(s.values().contains(\"a\"));\nprintln(s.values().contains(1));\nreturn 0;\n",
            ),
        ],
    );
}

/// `math` answers the same values *and* the same errors on both ends.
///
/// Nine of the module's twenty functions did not lower: `tan` while `sin` and
/// `cos` did, the whole inverse and logarithm families, and `clamp`. The split
/// was not a rule — it was where someone stopped.
///
/// The domain guards matter more than the values. `math.sqrt(-1.0)` used to
/// print its real reason to *stderr* and raise `"runtime error"`, so
/// `try { math.sqrt(-1.0) } catch e { e }` was `"sqrt() argument must be
/// non-negative"` interpreted and `"runtime error"` compiled — and a caught
/// error's text is the program's output, not a diagnostic. Every guard added
/// here raises the stdlib module's own sentence, and this test is what pins
/// them word for word.
#[test]
fn math_agrees_on_values_and_on_domain_errors() {
    run_clif_differential(
        "math_surface",
        &[
            new(
                "the_whole_module_lowers",
                "use math;\nlet n = 2.0;\nprintln(math.tan(0.0));\nprintln(math.asin(0.5));\n\
                 println(math.acos(0.5));\nprintln(math.atan(1.0));\nprintln(math.atan2(1.0, n));\n\
                 println(math.log(1.0));\nprintln(math.log10(100.0));\nprintln(math.log2(8.0));\n\
                 println(math.clamp(5, 1, 3));\nprintln(math.clamp(0, 1, 3));\n\
                 println(math.clamp(2, 1, 3));\nreturn 0;\n",
            ),
            // The words, not just the fact that it raised.
            new(
                "a_domain_error_carries_the_modules_own_words",
                "use math;\n\
                 println(try { math.sqrt(-1.0); \"no\" } catch e { \"${e}\" });\n\
                 println(try { math.asin(2.0); \"no\" } catch e { \"${e}\" });\n\
                 println(try { math.acos(-2.0); \"no\" } catch e { \"${e}\" });\n\
                 println(try { math.log(0.0); \"no\" } catch e { \"${e}\" });\n\
                 println(try { math.log10(-1.0); \"no\" } catch e { \"${e}\" });\n\
                 println(try { math.log2(0.0); \"no\" } catch e { \"${e}\" });\n\
                 println(try { math.clamp(5, 3, 1); \"no\" } catch e { \"${e}\" });\nreturn 0;\n",
            ),
            // Edges the two ends could disagree on quietly.
            new(
                "edges_of_the_domains",
                "use math;\nprintln(math.asin(1.0));\nprintln(math.acos(-1.0));\n\
                 println(math.log(1.0));\nprintln(math.atan2(0.0, 0.0));\n\
                 println(math.sqrt(0.0));\nprintln(math.clamp(1, 1, 1));\nreturn 0;\n",
            ),
        ],
    );
}

/// `string.f(s, …)` and `s.f(…)` are the same call, so they lower the same way.
///
/// The forwarder that makes a module spelling reach the method arm was gated on
/// `matches!(module, "iter" | "stream")` — a list of two, not a rule. So every
/// one of the `string` module's functions fell back to the VM while its method
/// spelling lowered, and which spelling a program happened to use decided
/// whether it stayed native. The VM routes both through the same
/// `core_methods`; all thirteen pairs below were checked to be equal there
/// before the gate was widened.
///
/// `split` needed one thing more: it is an *intrinsic* in the bytecode compiler,
/// so the method spelling becomes `Opcode::StringSplit` and never reaches the
/// method table at all. The module spelling does, so the arm it forwards to had
/// to exist — pointed at the same helper the opcode uses, so the two cannot
/// drift.
#[test]
fn the_string_module_spelling_lowers_like_the_method() {
    run_clif_differential(
        "string_module_spelling",
        &[
            new(
                "each_pair_agrees",
                "use string;\nlet z = \"z\";\nlet s = \"  Hello World  \" + z;\n\
                 println(string.trim(s) == s.trim());\nprintln(string.upper(s) == s.upper());\n\
                 println(string.lower(s) == s.lower());\nprintln(string.len(s) == s.len());\n\
                 println(string.reverse(s) == s.reverse());\n\
                 println(string.contains(s, \"Hello\") == s.contains(\"Hello\"));\n\
                 println(string.index_of(s, \"World\") == s.index_of(\"World\"));\n\
                 println(string.starts_with(s, \" \") == s.starts_with(\" \"));\n\
                 println(string.ends_with(s, \"z\") == s.ends_with(\"z\"));\n\
                 println(string.slice(s, 0, 4) == s.slice(0, 4));\n\
                 println(string.repeat(\"ab\", 2) == \"ab\".repeat(2));\nreturn 0;\n",
            ),
            // The intrinsic pair, and the values themselves rather than only the
            // equality — a bug that made both sides equally wrong would pass the
            // comparisons above.
            new(
                "split_and_replace_by_value",
                "use string;\nlet z = \",\";\nprintln(string.split(\"a,b,c\", z));\n\
                 println(string.replace(\"aaa\", \"a\", \"b\"));\nprintln(string.trim(\"  x  \"));\n\
                 println(string.upper(\"aBc\"));\nprintln(string.index_of(\"abc\", \"zz\"));\n\
                 println(string.slice(\"abcde\", 1, 3));\nreturn 0;\n",
            ),
            // Multibyte, because every string position in this language is a
            // character position and the module spelling must not forget.
            new(
                "module_spelling_counts_characters_too",
                "use string;\nlet s = \"中文abc\";\nprintln(string.len(s));\n\
                 println(string.slice(s, 1, 3));\nprintln(string.index_of(s, \"a\"));\n\
                 println(string.reverse(s));\nreturn 0;\n",
            ),
        ],
    );
}

/// The `path` module's fixed-arity members answer natively, and answer the same.
///
/// The module is `std::path` on both ends — the same discipline that keeps the
/// base64/hex text and the datetime formatting byte-identical: share the crate
/// underneath, do not write the rule twice.
///
/// The `String?` members are the sharp edge. `path.parent("c.txt")` is the empty
/// string while `path.parent("/")` is nil, and `path.extension("a")` and
/// `path.extension(".bashrc")` are both nil for different reasons — a boxed
/// result that got the empty-vs-nil distinction wrong would look right on the
/// common cases.
#[test]
fn path_members_answer_the_same_on_both_ends() {
    run_clif_differential(
        "path_members",
        &[
            new(
                "the_optional_parts",
                "use path;\nlet z = \"\";\nprintln(path.parent(\"a/b/c.txt\" + z));\n\
                 println(path.parent(\"c.txt\"));\nprintln(path.parent(\"/\"));\n\
                 println(path.file_name(\"a/b/c.txt\"));\nprintln(path.file_name(\"a/b/\"));\n\
                 println(path.file_stem(\"a/b/c.tar.gz\"));\nprintln(path.extension(\"a/b/c.tar.gz\"));\n\
                 println(path.extension(\"a\"));\nprintln(path.extension(\".bashrc\"));\nreturn 0;\n",
            ),
            new(
                "the_total_parts",
                "use path;\nlet z = \"\";\nprintln(path.with_extension(\"a/b.txt\" + z, \"md\"));\n\
                 println(path.with_extension(\"a\", \"txt\"));\nprintln(path.is_absolute(\"/a\"));\n\
                 println(path.is_absolute(\"a\"));\nprintln(path.components(\"a/b/c\"));\n\
                 println(path.components(\"/a/b\"));\nprintln(path.components(\"\"));\n\
                 println(path.sep());\nprintln(path.delimiter());\nreturn 0;\n",
            ),
        ],
    );
}

/// `hash` answers the same digest natively, on both carriers.
///
/// `sha256`/`sha1`/`crc32` come from the same crates the stdlib module uses, so
/// this mostly pins the *hex rendering* and the string→UTF-8-bytes rule. The
/// case that carries real weight is `fnv64`: no crate in either graph provides
/// FNV-1a, so its loop and its two constants exist twice, and a transcription
/// slip in either one is invisible until something compares the numbers.
#[test]
fn hash_members_answer_the_same_on_both_ends() {
    run_clif_differential(
        "hash_members",
        &[
            new(
                "string_carrier",
                "use hash;\nlet z = \"\";\nprintln(hash.sha256(\"hello\" + z));\n\
                 println(hash.sha1(\"hello\"));\nprintln(hash.crc32(\"hello\"));\n\
                 println(hash.fnv64(\"hello\"));\nprintln(hash.fnv64(\"\"));\n\
                 println(hash.crc32(\"\"));\nprintln(hash.fnv64(\"abc\"));\n\
                 println(hash.sha256(\"\"));\nprintln(hash.fnv64(\"中文\"));\nreturn 0;\n",
            ),
            new(
                "bytes_carrier",
                "use hash;\nuse bytes;\nuse encoding;\nlet b = bytes.from_string(\"hello\");\n\
                 println(hash.sha256(b));\nprintln(hash.sha1(b));\nprintln(hash.crc32(b));\n\
                 println(hash.fnv64(b));\nprintln(encoding.base64.encode(b));\n\
                 println(encoding.hex.encode(b));\nprintln(encoding.base64.encode(\"hello\"));\n\
                 return 0;\n",
            ),
        ],
    );
}

/// `uuid` parses, validates and raises identically on both ends.
///
/// The last one is the reason the native side shares the `uuid` crate rather
/// than re-implementing the parse: `uuid.parse("nope")` raises `invalid UUID:
/// invalid character: found `n` at 0`, where everything after the colon is the
/// crate's own `Display` — and a caught error's message is program output.
///
/// `v4` cannot be compared directly (that is the point of it), so what the
/// second case compares is everything about it that *is* fixed: the length, the
/// canonical shape as `is_valid` judges it, and that two calls differ — which
/// also pins the ABI classification, since a `Pure` `v4` would be CSE'd into
/// one call and print `false`.
#[test]
fn uuid_members_answer_the_same_on_both_ends() {
    run_clif_differential(
        "uuid_members",
        &[
            new(
                "parse_and_validate",
                "use uuid;\nlet z = \"\";\n\
                 println(uuid.parse(\"550E8400-E29B-41D4-A716-446655440000\" + z));\n\
                 println(uuid.parse(\"550e8400e29b41d4a716446655440000\"));\n\
                 println(uuid.is_valid(\"550e8400-e29b-41d4-a716-446655440000\"));\n\
                 println(uuid.is_valid(\"nope\"));\nprintln(uuid.is_valid(\"\"));\n\
                 let r = try { uuid.parse(\"nope\") } catch e { e };\nprintln(r);\nreturn 0;\n",
            ),
            new(
                "v4_shape",
                "use uuid;\nlet a = uuid.v4();\nlet b = uuid.v4();\n\
                 println(a.len());\nprintln(uuid.is_valid(a));\nprintln(a != b);\n\
                 println(uuid.parse(a) == a);\nreturn 0;\n",
            ),
        ],
    );
}

/// The `fs` surface: same values, same booleans, and the same error text.
///
/// Half of these had an lkrt implementation and an ABI row already but no
/// lowering row, so nothing could call them — and three of the messages
/// (`read_to_string`, `write`, `canonicalize`) had drifted from the stdlib
/// module's wording in the meantime. That is what an unreachable code path is:
/// unverified, not spare.
///
/// The cases pin the things that are easy to get subtly wrong: `remove_*`
/// answers `false` for an absent path instead of raising, `copy` answers a byte
/// count rather than a bool, `read_dir` sorts, and a raise names the path with
/// the stdlib's exact sentence.
#[test]
fn fs_members_answer_the_same_on_both_ends() {
    run_clif_differential(
        "fs_members",
        &[
            new(
                "files",
                "use fs;\nuse bytes;\nlet d = fs.temp_dir() + \"/lk_diff_fs_files\";\n\
                 fs.remove_dir_all(d);\nprintln(fs.create_dir_all(d + \"/sub\"));\n\
                 println(fs.write(d + \"/b.txt\", \"bbb\"));\n\
                 println(fs.write(d + \"/a.txt\", bytes.from_string(\"aaa\")));\n\
                 println(fs.append(d + \"/a.txt\", \"!\"));\n\
                 println(fs.read_to_string(d + \"/a.txt\"));\nprintln(fs.read_dir(d));\n\
                 println(fs.is_file(d + \"/a.txt\"));\nprintln(fs.is_dir(d));\n\
                 println(fs.is_file(d + \"/nope\"));\n\
                 println(fs.copy(d + \"/a.txt\", d + \"/c.txt\"));\n\
                 println(fs.rename(d + \"/c.txt\", d + \"/e.txt\"));\n\
                 println(fs.remove_file(d + \"/e.txt\"));\n\
                 println(fs.remove_file(d + \"/e.txt\"));\nprintln(fs.read_dir(d));\n\
                 println(fs.remove_dir_all(d));\nreturn 0;\n",
            ),
            new(
                "errors_and_env",
                "use fs;\nuse env;\nlet d = fs.temp_dir() + \"/lk_diff_fs_missing\";\n\
                 let a = try { fs.read_to_string(d) } catch e { e };\nprintln(a);\n\
                 let b = try { fs.canonicalize(d) } catch e { e };\nprintln(b);\n\
                 let c = try { fs.write(d + \"/x/y\", \"a\") } catch e { e };\nprintln(c);\n\
                 let f = try { fs.rename(d, d + \"2\") } catch e { e };\nprintln(f);\n\
                 println(fs.exists(d));\nprintln(env.has(\"PATH\"));\n\
                 println(env.has(\"LK_NO_SUCH_VAR_XYZ\"));\nreturn 0;\n",
            ),
        ],
    );
}
