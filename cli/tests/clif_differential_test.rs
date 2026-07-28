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
    let src = "fn add_u8(a: u8) -> u8 { return a + 1; }\n\
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
               println(neg_u32(0xFFFFFF80 as u32));\n";
    let expected = concat!(
        "0\n144\n255\n-128\n",
        "4294934528\ntrue\n9223372036854775807\n128\n",
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
        ("wide_u32.lk", "let y: u32 = 0xFFFFFFFFFFFFFFFF;\n", "out of range for u32"),
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
            new("shift_right", "let a: u16 = 0x1234;\nlet b: u16 = 8;\nreturn ((a >> b) & 0xff) as Int;\n"),
            new("shift_left", "let a: u8 = 0x0f;\nlet b: u8 = 1;\nreturn (a << b) as Int;\n"),
            // The width decides, not the arithmetic.
            new("u8_wraps", "let a: u8 = 255;\nlet b: u8 = 1;\nreturn (a + b) as Int;\n"),
            new("u16_wraps", "let a: u16 = 65535;\nlet b: u16 = 1;\nreturn (a + b) as Int;\n"),
            new(
                "u32_wraps",
                "let a: u32 = 4294967295;\nlet b: u32 = 2;\nreturn (a + b) as Int;\n",
            ),
            // Subtraction under zero wraps the same way, which is how a driver
            // computing a ring index one short of the base finds out.
            new("u8_wraps_down", "let a: u8 = 0;\nlet b: u8 = 1;\nreturn (a - b) as Int;\n"),
            // Multiplication past the width, which is where a promotion to
            // `Int` would be least visible: the low bits are still right.
            new("u8_multiplies", "let a: u8 = 200;\nlet b: u8 = 3;\nreturn (a * b) as Int;\n"),
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
            new("u32_compares_a_literal", "let a: u32 = 7;\nif (a > 3) { return 1; }\nreturn 0;\n"),
            new("u8_compares_a_literal", "let a: u8 = 0;\nif (a > 0) { return 1; }\nreturn 0;\n"),
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
            new("complement_wraps_to_the_width", "let a: u8 = 0x0f;\nreturn (~a) as Int;\n"),
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
            new("complement_wraps_to_the_width", "let a: u8 = 0x0f;\nreturn (~a) as Int;\n"),
            new("complement_u32", "let a: u32 = 0xff;\nreturn (~a) as Int;\n"),
            new(
                "complement_clears_a_bit",
                "let flags: u32 = 0xff;\nlet bit: u32 = 0x80;\nreturn (flags & ~bit) as Int;\n",
            ),
            new("complement_u64", "let one: u64 = 1;\nreturn ((~one) >> 32) as Int;\n"),
            // A signed comparison is still signed, which is the property the
            // change must not have taken away.
            new("i64_compares_signed", "let a = 0 - 1;\nif (a < 1) { return 1; }\nreturn 0;\n"),
            // And a signed shift is still arithmetic, which is the property the
            // change must not have taken away.
            new("i8_shifts_arithmetically", "let a: i8 = 0 - 128;\nlet s: i8 = 7;\nreturn (a >> s) as Int;\n"),
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
