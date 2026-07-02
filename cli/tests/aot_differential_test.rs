//! Differential harness (docs/llvm/aot-redesign.md §6): every case is compiled
//! natively through the MIR pipeline (the only backend) and executed, then run
//! under the bytecode VM, and the observable behaviour (stdout + success/failure)
//! must match exactly, and the emitted IR must come from the
//! `lk-aot-lower` → `lk-aot-codegen` path.
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
    let pid = std::process::id();
    p.push(format!("lk_aot_diff_{name}_{pid}"));
    p
}

fn ensure_clean_dir(dir: &FsPath) {
    let _ = fs::remove_dir_all(dir);
    create_dir_all(dir).expect("create tmp dir");
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

/// Compile `case` natively with the MIR gate enabled, run it, run the same
/// source under the VM, and require identical stdout and identical
/// success/failure (exact failure codes legitimately differ: VM runtime errors
/// exit 1 while native guards abort with SIGABRT).
fn run_differential(area: &str, cases: &[Case]) {
    let dir = unique_tmp_dir(area);
    ensure_clean_dir(&dir);

    for case in cases {
        let file = format!("{}.lk", case.name);
        let path = dir.join(&file);
        let mut f = File::create(&path).expect("create case file");
        f.write_all(case.source.as_bytes()).expect("write case file");

        // VM reference run.
        let vm = run_cli(&dir, [file.as_str()]).output().expect("spawn vm run");
        let vm_stdout = String::from_utf8_lossy(&vm.stdout).into_owned();

        // MIR-path IR.
        let llvm = run_cli(&dir, ["compile", "llvm", &file])
            .output()
            .expect("spawn llvm compile");
        assert!(
            llvm.status.success(),
            "[{area}/{}] IR compile failed: {}",
            case.name,
            String::from_utf8_lossy(&llvm.stderr)
        );
        let ir = fs::read_to_string(dir.join(format!("{}.ll", case.name))).expect("read IR");
        assert!(
            ir.contains("; ModuleID = 'lk_aot'"),
            "[{area}/{}] expected MIR-pipeline IR",
            case.name
        );

        // Native build + run.
        let exe = run_cli(&dir, ["compile", &file])
            .output()
            .expect("spawn native compile");
        assert!(
            exe.status.success(),
            "[{area}/{}] native compile failed: {}",
            case.name,
            String::from_utf8_lossy(&exe.stderr)
        );
        let native = Command::new(dir.join(case.name))
            .output()
            .expect("spawn compiled executable");
        let native_stdout = String::from_utf8_lossy(&native.stdout).into_owned();

        assert_eq!(
            vm_stdout, native_stdout,
            "[{area}/{}] stdout diverged (vm vs native)",
            case.name
        );
        assert_eq!(
            vm.status.success(),
            native.status.success(),
            "[{area}/{}] success/failure diverged: vm={:?} native={:?} stderr(vm)={} stderr(native)={}",
            case.name,
            vm.status,
            native.status,
            String::from_utf8_lossy(&vm.stderr),
            String::from_utf8_lossy(&native.stderr)
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn differential_scalars() {
    run_differential(
        "scalars",
        &[
            new("arith", "return 1 + 2 * 3;\n"),
            new("var_arith", "let x = 10;\nreturn x * 3 + 1;\n"),
            new("float_add", "return 1.5 + 2.5;\n"),
            new("int_div", "return 20 / 4;\n"),
            new("int_mod", "return 7 % 3;\n"),
            new("cmp", "return 3 < 5;\n"),
            new("float_div_display", "return 1.0 / 7.0;\n"),
            new("mixed_float", "return 5 + 7.5;\n"),
            new("not_expr", "return !(3 > 4);\n"),
            new("div_zero", "let x = 2;\nlet y = 0;\nreturn x / y;\n"),
        ],
    );
}

#[test]
fn differential_control_flow() {
    run_differential(
        "control",
        &[
            new("min", "let a = 3;\nlet b = 5;\nif a < b { return a; }\nreturn b;\n"),
            new(
                "sum_loop",
                "let s = 0;\nlet i = 1;\nwhile (i <= 100) { s = s + i; i = i + 1; }\nreturn s;\n",
            ),
            new("countdown", "let i = 10;\nwhile (i > 0) { i = i - 1; }\nreturn i;\n"),
            new(
                "factorial",
                "let f = 1;\nlet i = 2;\nwhile (i <= 6) { f = f * i; i = i + 1; }\nreturn f;\n",
            ),
            new(
                "nested_loops",
                "let c = 0;\nlet i = 0;\nwhile (i < 5) { let j = 0; while (j < 5) { c = c + 1; j = j + 1; } i = i + 1; }\nreturn c;\n",
            ),
            new(
                "break_loop",
                "let i = 0;\nwhile (true) { if i == 10 { break; } i = i + 1; }\nreturn i;\n",
            ),
            new(
                "continue_loop",
                "let s = 0;\nlet i = 0;\nwhile (i < 10) { i = i + 1; if i % 2 == 0 { continue; } s = s + i; }\nreturn s;\n",
            ),
            new(
                "else_if_chain",
                "let x = 7;\nif x < 5 { return 0; } else if x < 10 { return 1; } else { return 2; }\n",
            ),
            new(
                "fizz_count",
                "let c = 0;\nlet i = 1;\nwhile (i <= 15) { if i % 3 == 0 { c = c + 1; } i = i + 1; }\nreturn c;\n",
            ),
            new("not_branch", "let x = 5;\nif !(x > 10) { return 100; }\nreturn 1;\n"),
            new(
                "float_loop",
                "let s = 0.0;\nlet i = 0;\nwhile (i < 5) { s = s + 1.5; i = i + 1; }\nreturn s;\n",
            ),
        ],
    );
}

#[test]
fn differential_functions() {
    run_differential(
        "functions",
        &[
            new("add_fn", "fn add(a, b) { return a + b; }\nreturn add(3, 4);\n"),
            new(
                "fact_rec",
                "fn fact(n) { if n <= 1 { return 1; } return n * fact(n - 1); }\nreturn fact(6);\n",
            ),
            new(
                "fib_rec",
                "fn fib(n) { if n < 2 { return n; } return fib(n - 1) + fib(n - 2); }\nreturn fib(10);\n",
            ),
            new(
                "gcd_rec",
                "fn gcd(a, b) { if b == 0 { return a; } return gcd(b, a % b); }\nreturn gcd(48, 36);\n",
            ),
            new(
                "nested_call",
                "fn inc(x) { return x + 1; }\nfn dbl(x) { return x * 2; }\nreturn dbl(inc(5));\n",
            ),
            new("f64_param", "fn scale(x) { return x * 2.5; }\nreturn scale(4.0);\n"),
            new("f64_ret", "fn half(x) { return x / 2.0; }\nreturn half(10);\n"),
            new(
                "bool_ret",
                "fn ev(x) { return x % 2 == 0; }\nif ev(4) { return 1; }\nreturn 0;\n",
            ),
            new(
                "ret_chain",
                "fn g(x) { return x * 2.0; }\nfn f(x) { return g(x) + 1.0; }\nreturn f(3);\n",
            ),
        ],
    );
}

#[test]
fn differential_lists() {
    run_differential(
        "lists",
        &[
            new("len", "let xs = [1, 2, 3, 4];\nreturn xs.len();\n"),
            new("const_index", "let xs = [10, 20, 30, 40];\nreturn xs[0] + xs[2];\n"),
            new("oob_nil", "let xs = [10];\nreturn xs[9];\n"),
            new("neg_index", "let xs = [10, 20, 30];\nreturn xs[-1];\n"),
            new(
                "push_loop",
                "let xs = [];\nlet i = 0;\nwhile (i < 5) { xs.push(i); i = i + 1; }\nreturn xs.len();\n",
            ),
            new("set_index", "let xs = [1, 2, 3];\nxs[1] = 99;\nreturn xs[1];\n"),
            new(
                "fill_squares",
                "let xs = [0, 0, 0, 0, 0];\nlet i = 0;\nwhile (i < 5) { xs[i] = i * i; i = i + 1; }\nreturn xs[3];\n",
            ),
            new(
                "iterate_sum",
                "let xs = [10, 20, 30];\nlet s = 0;\nfor x in xs { s = s + x; }\nreturn s;\n",
            ),
            new("in_op", "let xs = [1, 2, 3];\nreturn 2 in xs;\n"),
            new(
                "f64_iterate",
                "let xs = [1.5, 2.0, 3.5];\nlet s = 0.0;\nfor x in xs { s = s + x; }\nreturn s;\n",
            ),
            new(
                "index_sum_loop",
                "let xs = [5, 10, 15];\nlet s = 0;\nlet i = 0;\nwhile (i < xs.len()) { s = s + xs[i]; i = i + 1; }\nreturn s;\n",
            ),
            new("join", "let xs = [\"a\", \"b\", \"c\"];\nreturn xs.join(\"-\");\n"),
            new("str_index", "let xs = [\"foo\", \"bar\"];\nreturn xs[1];\n"),
            new(
                "str_dyn_index",
                "let xs = [\"a\", \"b\", \"c\"];\nlet s = \"\";\nlet i = 0;\nwhile (i < xs.len()) { s = s + xs[i]; i = i + 1; }\nreturn s;\n",
            ),
            new("str_oob_nil", "let xs = [\"a\"];\nreturn xs[5];\n"),
            new("str_neg_index", "let xs = [\"a\", \"b\", \"c\"];\nreturn xs[-1];\n"),
            new(
                "str_nil_branch",
                "let xs = [\"a\"];\nif xs[9] == nil { return 1; }\nreturn 0;\n",
            ),
            new(
                "nil_branch_oob",
                "let xs = [1];\nif xs[9] == nil { return 1; }\nreturn 0;\n",
            ),
        ],
    );
}

#[test]
fn differential_maps() {
    run_differential(
        "maps",
        &[
            new("str_get", "let m = {\"a\": 1, \"b\": 2};\nreturn m[\"b\"];\n"),
            new("missing_nil", "let m = {\"a\": 1};\nreturn m[\"z\"];\n"),
            new(
                "build",
                "let m = {};\nm[\"x\"] = 5;\nm[\"y\"] = 9;\nreturn m[\"x\"] + m[\"y\"];\n",
            ),
            new("int_key", "let m = {1: 10, 2: 20};\nreturn m[2];\n"),
            new("len", "let m = {\"a\": 1, \"b\": 2, \"c\": 3};\nreturn m.len();\n"),
            new("str_f64", "let m = {\"a\": 1.5, \"b\": 2.5};\nreturn m[\"b\"];\n"),
            new("int_f64", "let m = {1: 1.5, 2: 2.5};\nreturn m[2];\n"),
            new(
                "empty_int_key",
                "let m = {};\nlet i = 0;\nwhile (i < 3) { m[i] = i * 10; i = i + 1; }\nreturn m[2];\n",
            ),
            new(
                "freq_count",
                "let xs = [1, 2, 2, 3, 3];\nlet freq = {};\nfor x in xs { freq[x] = 1; }\nreturn freq.len();\n",
            ),
            new(
                "nil_branch_missing",
                "let m = {\"a\": 1};\nif m[\"z\"] == nil { return 1; }\nreturn 0;\n",
            ),
            new("missing_arith_halts", "let m = {\"a\": 1};\nreturn m[\"z\"] + 1;\n"),
        ],
    );
}

#[test]
fn differential_strings() {
    run_differential(
        "strings",
        &[
            new("const_ret", "return \"hello\";\n"),
            new("eq", "return \"hi\" == \"hi\";\n"),
            new("ne", "return \"hi\" != \"ho\";\n"),
            new("concat", "let a = \"foo\";\nlet b = \"bar\";\nreturn a + b;\n"),
            new("interp_str", "let a = \"x\";\nlet b = \"y\";\nreturn \"${a}-${b}!\";\n"),
            new("interp_int", "let n = 5;\nreturn \"n=${n}\";\n"),
            new(
                "interp_expr",
                "let a = 3;\nlet b = 4;\nreturn \"${a}+${b}=${a + b}\";\n",
            ),
            new("interp_bool", "let x = 5;\nreturn \"big=${x > 3}\";\n"),
            new("interp_float", "return \"v=${2.0}\";\n"),
            new("interp_neg", "return \"val:${-7}\";\n"),
            new("long_string", "return \"longer-than-short\";\n"),
            new(
                "long_string_var",
                "let s = \"a-fairly-long-string-literal\";\nreturn s + \"!\";\n",
            ),
        ],
    );
}

#[test]
fn differential_modules_and_globals() {
    run_differential(
        "modules",
        &[
            // Module builtins: only determinism-safe assertions go through
            // stdout (clock/epoch values themselves are time-dependent).
            new(
                "os_clock_monotonic",
                "use os;\nlet t0 = os.clock();\nlet t1 = os.clock();\nprintln(\"ok={}\", t1 >= t0);\nreturn 0;\n",
            ),
            new("os_epoch_positive", "use os;\nreturn os.epoch() > 0;\n"),
            new(
                "env_get_or_default",
                "use env;\nlet v = env.get_or(\"LK_DIFF_NOT_SET_XYZ\", \"fallback\");\nprintln(\"{}\", v);\nreturn v == \"fallback\";\n",
            ),
            new(
                "math_floor_float",
                "use math;\nprintln(\"{} {} {}\", math.floor(7.9), math.floor(-7.1), math.floor(4));\nreturn 0;\n",
            ),
            new(
                "mutable_global_scalar",
                "let total = 0;\nlet scale = 2.5;\nfn read_total(x) { return total + x; }\ntotal = 40;\nprintln(\"{} {}\", read_total(2), scale * 2.0);\nreturn read_total(0);\n",
            ),
            new(
                "mutable_global_str",
                "use env;\nlet label = env.get_or(\"LK_DIFF_NOT_SET_XYZ\", \"tag\");\nfn show(n) { return \"${label}-${n}\"; }\nprintln(\"{}\", show(3));\nreturn 0;\n",
            ),
            new(
                "for_range_incl_excl",
                "let s = 0;\nfor i in 1..=10 { s = s + i; }\nlet t = 0;\nfor j in 0..4 { t = t + j; }\nprintln(\"{} {}\", s, t);\nreturn s + t;\n",
            ),
            new(
                "for_range_empty",
                "let s = 0;\nfor i in 5..5 { s = s + 1; }\nreturn s;\n",
            ),
            new(
                "maybe_default_merge",
                "let m = {\"a\": 1};\nlet k = \"a\";\nlet v = m[k + \"\"];\nif v == nil { v = 7; }\nlet w = m[k + \"x\"];\nif w == nil { w = 9; }\nprintln(\"{} {}\", v + 1, w + 1);\nreturn 0;\n",
            ),
            new(
                "dyn_str_key_map",
                "let counts = {};\nlet i = 0;\nwhile (i < 6) { let key = \"k\" + \"${i % 2}\";\n let prev = counts[key];\n if prev == nil { counts[key] = 1; } else { counts[key] = prev + 1; }\n i = i + 1; }\nprintln(\"{} {}\", counts[\"k0\"], counts[\"k1\"]);\nreturn counts.len();\n",
            ),
            new(
                "str_list_push_join",
                "let parts = [];\nlet i = 0;\nwhile (i < 3) { parts.push(\"p${i}\"); i = i + 1; }\nreturn parts.join(\",\");\n",
            ),
            new(
                "str_char_len",
                "let s = \"hello\" + \" world\";\nprintln(\"{}\", s.len());\nreturn 0;\n",
            ),
        ],
    );
}

#[test]
fn differential_builtins() {
    run_differential(
        "builtins",
        &[
            // println/print formatting must match `format_variadic_runtime`
            // exactly: `{}` substitution, leftover `{}` kept literal, extra args
            // appended space-separated, non-string first arg joined with spaces.
            new("println_fmt", "let x = 42;\nprintln(\"{}\", x);\nreturn 0;\n"),
            new(
                "println_multi",
                "let x = 6;\nprintln(\"a={} b={}\", x, x * 7);\nreturn 0;\n",
            ),
            new("println_value", "let x = 42;\nprintln(x);\nreturn 0;\n"),
            new("println_plain", "println(\"plain text\");\nreturn 0;\n"),
            new("println_empty", "println();\nreturn 0;\n"),
            new("println_missing_args", "println(\"x={} y={}\", 1);\nreturn 0;\n"),
            new("println_extra_args", "println(\"v:\", 2, 3);\nreturn 0;\n"),
            new("println_join", "println(1.5, true, \"s\");\nreturn 0;\n"),
            new(
                "println_dynamic_str",
                "let s = \"dyn\" + \"amic\";\nprintln(s);\nreturn 0;\n",
            ),
            new(
                "println_in_loop",
                "let i = 0;\nwhile (i < 3) { println(\"i={}\", i); i = i + 1; }\nreturn i;\n",
            ),
            new("print_no_newline", "print(\"a\");\nprint(\"b\");\nreturn 0;\n"),
            new("assert_true", "let x = 1;\nassert(x == 1);\nreturn 7;\n"),
            // Both sides must fail loudly with identical (already-flushed) stdout.
            new(
                "assert_false_after_output",
                "println(\"before\");\nlet x = 1;\nassert(x == 2);\nreturn 7;\n",
            ),
            new("assert_msg_false", "assert(1 == 2, \"boom\");\nreturn 7;\n"),
            new(
                "div_zero_after_output",
                "println(\"before\");\nlet a = 1;\nlet b = 0;\nreturn a % b;\n",
            ),
            // assert_eq/assert_ne: pass and loud-fail (with pre-flushed stdout),
            // Int/Float coercion, string comparison, extra message argument.
            new(
                "assert_eq_pass",
                "let x = 6;\nassert_eq(x * 7, 42);\nassert_eq(\"a\" + \"b\", \"ab\");\nassert_eq(2, 2.0);\nreturn 5;\n",
            ),
            new(
                "assert_eq_fail_after_output",
                "println(\"before\");\nlet x = 1;\nassert_eq(x, 2);\nreturn 7;\n",
            ),
            new("assert_eq_fail_msg", "assert_eq(1, 2, \"context\");\nreturn 7;\n"),
            new("assert_ne_pass", "assert_ne(1, 2);\nreturn 3;\n"),
            new(
                "assert_ne_fail_after_output",
                "println(\"before\");\nassert_ne(5, 5);\nreturn 7;\n",
            ),
            // panic: always fatal, stdout before it must be preserved.
            new(
                "panic_after_output",
                "println(\"before\");\nlet x = 1;\nif (x == 1) { panic(\"stop\", x); }\nreturn 7;\n",
            ),
            // Multi-identity lambda arguments: each identity gets its own
            // specialized clone of the callee (apply itself may be a lambda).
            new(
                "lambda_multi_identity",
                "fn apply(f, x) { return f(x) + f(x + 1); }\nlet double = |v| v * 2;\nlet square = |v| v * v;\nprintln(apply(double, 5));\nprintln(apply(square, 5));\nprintln(apply(|v| v + 100, 1));\nlet lapply = |g, n| g(n);\nprintln(lapply(double, 7));\nprintln(lapply(square, 5));\nreturn 0;\n",
            ),
            // List display: VM-exact separators/quoting through println
            // (the runtime_display path; template interpolation of containers
            // rejects — that path is scalar-only in the VM).
            new(
                "list_display",
                "let xs = [1, 2, 3];\nprintln(xs);\nprintln(\"{}\", xs);\nlet fs = [1.5, 2.0, 0.25];\nprintln(fs);\nlet ss = [];\nss.push(\"a\");\nss.push(\"b c\");\nss.push(\"he said \\\"hi\\\"\");\nprintln(ss);\nlet empty = [];\nempty.push(0);\nprintln(empty);\nreturn 0;\n",
            ),
            // NaN semantics: `!=` must be true when either side is NaN
            // (fcmp une, not one); ordered comparisons stay false.
            new(
                "math_nan_compare",
                "use math;\nlet n = math.nan;\nprintln(n != n);\nprintln(n == n);\nprintln(n < 1.0);\nprintln(n >= 1.0);\nprintln(1.5 != math.nan);\nreturn 0;\n",
            ),
            // math module: constants, type-directed rounding/abs/min/max,
            // Number→Float promotion, and the sqrt negative-argument guard.
            new(
                "math_consts_and_fns",
                "use math;\nprintln(math.pi > 3.14);\nprintln(math.abs(-42));\nprintln(math.abs(-1.5));\nprintln(math.floor(3.7));\nprintln(math.ceil(3.2));\nprintln(math.round(2.5));\nprintln(math.min(3, 7));\nprintln(math.max(2.5, 1.5));\nprintln(math.pow(2, 10));\nprintln(math.sqrt(144.0));\nprintln(math.exp(0));\nprintln(math.sin(0));\nprintln(math.cos(0));\nreturn 0;\n",
            ),
            new(
                "math_sqrt_negative_after_output",
                "use math;\nprintln(\"before\");\nlet x = 0.0 - 4.0;\nprintln(math.sqrt(x));\nreturn 0;\n",
            ),
            // Zero-capture lambdas: top-level (module-global, single
            // assignment) and function-local, called indirectly — both
            // devirtualize to direct calls.
            new(
                "lambda_toplevel_call",
                "let double = |x| x * 2;\nlet add = |a, b| a + b;\nprintln(double(5));\nprintln(add(3, 7));\nprintln(double(add(1, 2)));\nreturn 0;\n",
            ),
            new(
                "lambda_cross_function",
                "let inc = |x| x + 1;\nfn twice(n) { return inc(inc(n)); }\nprintln(twice(5));\nreturn 0;\n",
            ),
            new(
                "lambda_local_in_fn",
                "fn area(w, h) { let mul = |a, b| a * b; return mul(w, h); }\nprintln(area(6, 7));\nreturn 0;\n",
            ),
            new(
                "lambda_float_mono",
                "let scale = |x| x * 1.5;\nprintln(scale(2));\nprintln(scale(3));\nreturn 0;\n",
            ),
            new(
                "lambda_local_reassign",
                "let f = |x| x + 1;\nprintln(f(1));\nf = |x| x * 10;\nprintln(f(2));\nreturn 0;\n",
            ),
            // Capturing closures: the environment is a shared mutable cell —
            // a mutation *after* closure creation must be visible at the call
            // (the lowering resolves cells at each call site).
            new(
                "closure_capture_basic",
                "let factor = 3;\nlet scale = |x| x * factor;\nprintln(scale(4));\nreturn 0;\n",
            ),
            new(
                "closure_capture_mutation_after",
                "let factor = 3;\nlet scale = |x| x * factor;\nfactor = 5;\nprintln(scale(1));\nreturn 0;\n",
            ),
            new(
                "closure_capture_two_vars_intercall",
                "let a = 2;\nlet b = 30;\nlet f = |x| x * a + b;\nprintln(f(5));\na = 7;\nprintln(f(5));\nreturn 0;\n",
            ),
            new(
                "closure_capture_in_fn",
                "fn area(w) { let unit = 10;\n let mul = |v| v * unit;\n return mul(w); }\nprintln(area(6));\nreturn 0;\n",
            ),
            // (String captures flowing into `+` stay unsupported: the AddInt
            // string-operand prescan cannot see through `LoadCellVal`.)
            new(
                "closure_capture_float",
                "let rate = 1.5;\nlet scale = |x| x * rate;\nprintln(scale(2));\nprintln(scale(3));\nreturn 0;\n",
            ),
            // datetime (chrono-backed, byte-identical formatting) and str
            // contains/len; Bool == Bool comparison.
            new(
                "datetime_fns",
                "use datetime;\nlet ts = 1700000000;\nprintln(datetime.format(ts, \"%Y-%m-%d %H:%M:%S\"));\nprintln(datetime.add(ts, 3600));\nprintln(datetime.sub(ts, 3600));\nprintln(datetime.day_of_week(ts));\nprintln(datetime.day_of_year(ts));\nlet w = datetime.is_weekend(ts);\nprintln(w == true || w == false);\nlet f = datetime.format(ts, \"%Y-%m-%d\");\nprintln(f.contains(\"20\"));\nprintln(f.len());\nreturn 0;\n",
            ),
            // io.std: fixed stdio handles, write/writeln byte counts, flush.
            new(
                "io_std_write",
                "use { std } from io;\nlet out = std.stdout();\nprintln(std.write(out, \"a\"));\nprintln(std.writeln(out, \"b\"));\nprintln(std.flush(out));\nreturn 0;\n",
            ),
            // Zero-capture lambdas as user-function arguments: the parameter
            // is erased and the callee devirtualizes through the static ref
            // (single lambda identity across call sites).
            new(
                "lambda_as_argument",
                "fn apply(f, x) { return f(x) + f(x + 1); }\nlet double = |v| v * 2;\nprintln(apply(double, 5));\nprintln(apply(double, 10));\nfn twice(g, n) { return g(g(n)); }\nprintln(twice(|v| v + 3, 4));\nreturn 0;\n",
            ),
            // Capturing closures as user-function arguments: the environment
            // (resolved to current cell contents at the call site) travels as
            // hidden trailing arguments, so mutation between calls is visible;
            // zero-capture and capturing identities mix at the same helper.
            new(
                "closure_as_argument",
                "fn apply(f, x) { return f(x) + f(x + 1); }\nlet k = 10;\nlet addk = |v| v + k;\nprintln(apply(addk, 1));\nlet m = 3;\nlet mulm = |v| v * m;\nprintln(apply(mulm, 2));\nm = 5;\nprintln(apply(mulm, 2));\nlet double = |v| v * 2;\nprintln(apply(double, 4));\nprintln(apply(|v| v - k, 100));\nlet a = 2;\nlet b = 30;\nlet two = |x| x * a + b;\nprintln(apply(two, 5));\nreturn 0;\n",
            ),
            // A capturing closure forwarded through two helpers: the erased
            // identity and the hidden env arguments propagate transitively.
            new(
                "closure_as_argument_forwarding",
                "fn inner(f, x) { return f(x); }\nfn outer(g, y) { return inner(g, y) + inner(g, y * 2); }\nlet base = 100;\nlet addb = |v| v + base;\nprintln(outer(addb, 5));\nbase = 200;\nprintln(outer(addb, 5));\nreturn 0;\n",
            ),
            // Cross-block cell state (virtual-slot phis): mutation in branch
            // arms visible after the merge, loop-carried cell updates, a
            // capturing closure through a branchy (VM-inlined) helper, and
            // loop-variable snapshot capture (per-iteration cell copy).
            new(
                "closure_cell_across_blocks",
                "let c = 1;\nlet f = |x| x * c;\nlet n = 4;\nif n > 2 { c = 5; } else { c = 7; }\nprintln(f(2));\nc = 9;\nprintln(f(2));\nreturn 0;\n",
            ),
            new(
                "closure_cell_loop_carried",
                "let c = 0;\nlet f = |x| x + c;\nfor i in 0..3 {\n  println(f(0));\n  c = c + 1;\n}\nprintln(f(100));\nreturn 0;\n",
            ),
            new(
                "closure_arg_branchy_helper",
                "fn pick(h, n) { if n > 3 { return h(n); } return h(0); }\nlet off = 7;\nprintln(pick(|q| q + off, 10));\nprintln(pick(|q| q + off, 1));\noff = 20;\nprintln(pick(|q| q + off, 1));\nreturn 0;\n",
            ),
            new(
                "closure_loop_var_snapshot",
                "for i in 0..3 {\n  let f = |x| x + i;\n  println(f(10));\n}\nlet j = 0;\nwhile (j < 3) {\n  let g = |x| x * 10 + j;\n  println(g(j));\n  j = j + 1;\n}\nreturn 0;\n",
            ),
            // A body re-`let` of the loop variable's name is a fresh binding:
            // the counter register stays intact (two iterations), the capture
            // promotes a shared cell (assignment visible), per-iteration.
            new(
                "closure_loop_name_re_let",
                "for i in 0..2 {\n  let i = i * 10;\n  let f = |x| x + i;\n  i = i + 5;\n  println(f(0));\n}\nreturn 0;\n",
            ),
            // A read-only Var argument to an inlined helper must not alias a
            // register a later closure argument boxes (captured locals
            // pre-promote before any argument lowers).
            new(
                "closure_inline_arg_alias",
                "fn use2(a, g) { let t = a + 1; return g(t); }\nlet y = 10;\nprintln(use2(y, |q| q + y));\ny = 20;\nprintln(use2(y, |q| q + y));\nreturn 0;\n",
            ),
            // Returned closures via the static summary path: a function whose
            // single return is a closure with parameter-mapped captures is
            // consumed at the call site (no call emitted, pure body skipped).
            // Covers distinct environments from one factory, a zero-capture
            // return, and factory results feeding closure-as-argument calls.
            new(
                "closure_returned",
                "fn multiplier(n) { return |x| x * n; }\nlet triple = multiplier(3);\nlet quintuple = multiplier(5);\nprintln(triple(4));\nprintln(quintuple(4));\nprintln(triple(7) + quintuple(2));\nfn make_adder() { return |a, b| a + b; }\nlet add = make_adder();\nprintln(add(3, 9));\nreturn 0;\n",
            ),
            new(
                "closure_returned_as_argument",
                "fn apply(f, x) { return f(x) + f(x + 1); }\nfn multiplier(n) { return |x| x * n; }\nprintln(apply(|v| v * 2, 3));\nprintln(apply(multiplier(3), 5));\nlet k = 7;\nprintln(apply(|v| v + k, 1));\nprintln(apply(multiplier(k), 2));\nreturn 0;\n",
            ),
            // List structural equality: length/element mismatches, empty
            // lists, Int/Float coercion ([1] == [1.0] is true), NaN elements
            // breaking equality, str lists, != inversion, and the non-empty
            // cross-typed fold ([1] == ["1"] is false).
            new(
                "list_structural_eq",
                "let a = [1, 2, 3];\nprintln(a == [1, 2, 3]);\nprintln(a == [1, 2]);\nprintln(a == [1, 2, 4]);\nprintln(a != [1, 2, 3]);\nprintln([] == []);\nlet f = [1.5, 2.0];\nprintln(f == [1.5, 2.0]);\nprintln(f == [1.5, 2.1]);\nlet s = [\"x\", \"y\"];\nprintln(s == [\"x\", \"y\"]);\nprintln(s == [\"x\", \"z\"]);\nprintln([1] == [1.0]);\nprintln([1] == [\"1\"]);\nuse math;\nprintln([math.nan] == [math.nan]);\nlet grown = [1];\ngrown.push(2);\nprintln(grown == [1, 2]);\nreturn 0;\n",
            ),
            // List HOF over compiled zero-capture lambdas (fn-pointer ABI):
            // map/filter/reduce over List<i64>, including chained pipelines
            // and an aborting callback (div/0 inside the lambda).
            new(
                "list_hof_map_filter_reduce",
                "let nums = [1, 2, 3, 4, 5, 6];\nlet squares = nums.map(|x| x * x);\nprintln(squares[5]);\nlet evens = nums.filter(|x| x % 2 == 0);\nprintln(evens.len());\nlet total = nums.reduce(0, |acc, x| acc + x);\nprintln(total);\nreturn 0;\n",
            ),
            new(
                "list_hof_chain",
                "let nums = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];\nlet result = nums.filter(|x| x % 2 != 0).map(|x| x * x).reduce(0, |a, b| a + b);\nprintln(result);\nreturn 0;\n",
            ),
            new(
                "list_hof_callback_aborts_after_output",
                "println(\"before\");\nlet nums = [1, 0, 2];\nlet r = nums.map(|x| 10 / x);\nprintln(r[0]);\nreturn 0;\n",
            ),
            // typeof: static scalar names plus the runtime Maybe (missing map
            // key → Nil) selection. One println per call — a *dynamic* Str as
            // the first println argument with extra args is the (rejected)
            // dynamic-format-string shape.
            new(
                "typeof_scalars",
                "let i = 1;\nlet f = 1.5;\nlet b = true;\nlet s = \"x\";\nprintln(typeof(i));\nprintln(typeof(f));\nprintln(typeof(b));\nprintln(typeof(s));\nprintln(typeof(nil));\nreturn 0;\n",
            ),
            new(
                "typeof_map_maybe",
                "let m = {};\nm.set(\"k\", 1);\nprintln(typeof(m.get(\"k\")));\nprintln(typeof(m.get(\"missing\")));\nreturn 0;\n",
            ),
        ],
    );
}
