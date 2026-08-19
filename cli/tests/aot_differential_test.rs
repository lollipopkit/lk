//! Differential harness (docs/aot/aot-redesign.md §6): every case is compiled
//! natively through the MIR pipeline (the only backend) and executed, then run
//! under the bytecode VM, and the observable behaviour (stdout + success/failure)
//! must match exactly, and the emitted IR must come from the
//! `lk-aot-lower` → `lk-aot-codegen` path.
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
        // Leak detection stays off for sanitized native runs: a raise
        // longjmps over Rust frames whose plain temporaries (Vecs, boxes)
        // then never drop — leaked *by design* (lkrt's arena model), and
        // LSan's exit-time report both fails the run and swallows buffered
        // stdout. ASan's use-after-free/overflow checks remain fully on.
        let native = Command::new(dir.join(case.name))
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .output()
            .expect("spawn compiled executable");
        let native_stdout = String::from_utf8_lossy(&native.stdout).into_owned();

        assert_eq!(
            vm_stdout,
            native_stdout,
            "[{area}/{}] stdout diverged (vm vs native): vm={:?} native={:?} stderr(vm)={} stderr(native)={}",
            case.name,
            vm.status,
            native.status,
            String::from_utf8_lossy(&vm.stderr),
            String::from_utf8_lossy(&native.stderr)
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

/// The shapes `docs/semantics.md` used to exclude from this corpus.
///
/// They were excluded because the two backends genuinely disagreed:
/// `unique()` had a hand-written equality on each side, and lkrt's still
/// described the VM *of the time* — numerics by `to_bits`, strings "never
/// equal" past seven bytes, lists by handle. Once the VM's equality became
/// heap-aware the two drifted, and being outside the corpus is why nothing
/// said so. One equality now, so these belong here.
#[test]
fn differential_equality_and_unique() {
    run_differential(
        "equality",
        &[
            new("unique_zeros", "let xs = [0.0, -0.0];\nreturn xs.unique();\n"),
            new("unique_floats", "let xs = [1.0, 2.0, 1.0];\nreturn xs.unique();\n"),
            new(
                "unique_long_strings",
                "let s = \"abcdefghij\";\nlet xs = [s, s, \"ab\"];\nreturn xs.unique();\n",
            ),
            new("unique_nested", "let xs = [[1], [1], [2]];\nreturn xs.unique();\n"),
            new("eq_across_int_float", "let a = 1;\nlet b = 1.0;\nreturn a == b;\n"),
            new(
                "in_across_int_float",
                "let a = 1;\nlet ys = [1.0, 2.0];\nreturn a in ys;\n",
            ),
            new(
                "in_across_float_int",
                "let a = 1.0;\nlet ys = [1, 2];\nreturn a in ys;\n",
            ),
            new("in_misses", "let ys = [1, 2];\nreturn 1.5 in ys;\n"),
            // A miss is nil on every sequence, not -1: -1 is a valid index (the
            // last element), so `xs[xs.index_of(v)]` used to answer that
            // instead of failing.
            // `try` is an expression, so its value has to survive the region on
            // both backends — natively that means a cell, and a register seeded
            // with nil used to have no way back out of one.
            // Int overflow wraps rather than raising, and both backends have to
            // wrap the same way.
            new(
                "int_overflow_wraps",
                "let a = 9223372036854775807;\nlet b = -9223372036854775807 - 1;\nreturn [a + 1, a * 2, b - 1];\n",
            ),
            new(
                "try_expression_value",
                "fn d(a: Int, b: Int) -> Float {\n  if (b == 0) { error(\"zero\"); }\n  return a / b;\n}\nlet ok = try { d(10, 2) } catch e { -1.0 };\nlet bad = try { d(1, 0) } catch e { -1.0 };\nreturn [ok, bad];\n",
            ),
            new(
                "try_expression_nil_branch",
                "let r = try { 1 % 0 } catch e { let unused = 1; };\nreturn r;\n",
            ),
            new(
                "index_of_miss_is_nil",
                "let xs = [1, 2, 3];\nreturn [xs.index_of(9), xs.index_of(2), \"abc\".index_of(\"z\")];\n",
            ),
            // Strings order lexicographically on both backends. The type
            // checker used to refuse `<` on them outright, so `sort()` was the
            // only way to ask — and the native lowering, told the VM did not
            // support it either, rejected the whole function.
            new(
                "str_lt_long",
                "let a = \"aaaaaaaaa\" + \"a\";\nlet z = \"zzzzzzzzz\" + \"z\";\nreturn a < z;\n",
            ),
            new(
                "str_ge_long",
                "let a = \"aaaaaaaaa\" + \"a\";\nlet z = \"zzzzzzzzz\" + \"z\";\nreturn z >= a;\n",
            ),
            new("str_le_equal", "let a = \"mm\";\nreturn a <= \"mm\";\n"),
            new("str_gt_prefix", "let a = \"abc\";\nreturn a > \"ab\";\n"),
            // The String read surface, on text with multi-byte characters in
            // it. Only `substring`/`find` used to lower, both to byte-indexed
            // helpers, so this is exactly where the two backends disagreed —
            // and nothing compared them, because the corpus was ASCII.
            new(
                "str_slice_multibyte",
                "let s = \"héllo wörld\";\nreturn s.slice(1, 4);\n",
            ),
            new(
                "str_slice_open_multibyte",
                "let s = \"héllo wörld\";\nreturn s.slice(6);\n",
            ),
            new(
                "str_take_skip_multibyte",
                "let s = \"héllo wörld\";\nreturn s.take(3) + s.skip(9);\n",
            ),
            new(
                "str_index_of_multibyte",
                "let s = \"héllo wörld\";\nreturn s.index_of(\"wörld\");\n",
            ),
            new("str_index_of_miss", "let s = \"héllo\";\nreturn s.index_of(\"zz\");\n"),
            new(
                "str_negative_index_multibyte",
                "let s = \"中文abc\";\nreturn s[-1] + s[-5];\n",
            ),
            new(
                "str_first_last_multibyte",
                "let s = \"中文abc\";\nreturn [s.first(), s.last(), \"\".first()];\n",
            ),
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
            // Every arm returns, so nothing follows the match — the function's
            // last block has no terminator, and the catch-all arm is entered
            // with no test. Lowering saw a phantom edge off the end and either
            // rejected the function or built a `ret void` in an `-> i64` one.
            new(
                "match_arms_return",
                "fn g(n: Int) -> Int {\n    match n {\n        0 => { return 7; }\n        1 => { return 8; }\n        _ => { return 9; }\n    }\n}\nprintln(g(0));\nprintln(g(1));\nprintln(g(2));\nreturn 0;\n",
            ),
            // Unreachable code: with no predecessors it has a definition for no
            // register, and that emptiness used to flow into the blocks it
            // falls into, rejecting the function over its own parameter.
            new(
                "code_after_a_total_if",
                "fn h(n: Int) -> Int {\n    if n > 0 { return 1; } else { return 2; }\n    let z = n + 1;\n    return z;\n}\nprintln(h(5));\nprintln(h(-5));\nreturn 0;\n",
            ),
            // A `return` in one branch of a conditional expression ends that
            // branch, not the lowering of what follows the conditional.
            new(
                "conditional_branch_returns",
                "fn f(n: Int) -> Int {\n    let a = if n > 0 { return 1; } else { 2 };\n    return a + 10;\n}\nprintln(f(5));\nprintln(f(-5));\nreturn 0;\n",
            ),
            // A binding arm catches every value, nil included — the same rule
            // the wildcard follows.
            new(
                "binding_arm_catches_nil",
                "fn f(v: Int?) -> Int {\n    return match v { x => 1 };\n}\nprintln(f(nil));\nprintln(f(3));\nreturn 0;\n",
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
            // The same rule across a call: the callee widens a *parameter*,
            // and only the caller can build the list that way. Two shapes,
            // because they are discovered differently — a parameter two call
            // sites disagree about is erased to Dyn and the caller has to be
            // pessimistic, while a parameter a single call site pins keeps its
            // typed carrier and the callee is the one that reports the push.
            new(
                "a_callee_widens_a_shared_parameter",
                "fn widen(xs: Any) -> Int {\n  xs.push(\"z\");\n  return xs.len();\n}\nlet a = [1, 2];\nlet b = [1.5, 2.5];\nprintln(widen(a));\nprintln(widen(b));\nprintln(a);\nprintln(b);\nreturn 0;\n",
            ),
            new(
                "a_callee_widens_its_only_caller_s_list",
                "fn widen(xs: Any) -> Int {\n  xs.push(\"z\");\n  return xs.len();\n}\nlet a = [1, 2];\nprintln(widen(a));\nprintln(a);\nreturn 0;\n",
            ),
            // A list literal whose element type a later push contradicts is
            // built as a Dyn list from the start — the same fixpoint retry an
            // empty `[]` already used. The VM widens the carrier in place;
            // native cannot, so this used to fall back.
            new(
                "widened_after_a_typed_literal",
                "let a: List<Any> = [1, 2];\na.push(\"x\");\nprintln(a);\nlet b: List<Any> = [1.5, 2.5];\nb.push(\"y\");\nprintln(b);\nlet c: List<Any> = [\"p\", \"q\"];\nc.push(7);\nprintln(c);\nreturn 0;\n",
            ),
            new(
                "widened_from_a_register_window",
                "let n = 3;\nlet d: List<Any> = [n, n + 1];\nd.push(\"z\");\nprintln(d);\nlet f: List<Any> = [1, 2];\nfor i in 0..2 { f.push(\"s\"); }\nprintln(f);\nreturn 0;\n",
            ),
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
            // `index_of` on an int list. The VM has it on every sequence; the
            // lowering had it only on `Str`, so this dropped its module to the
            // VM — same answer, only slower, which no gate can see.
            new(
                "list_index_of",
                "let xs = [10, 20, 30];\nprintln(xs.index_of(20) ?? -1);\nprintln(xs.index_of(99) ?? -1);\nprintln([1].index_of(1) ?? -1);\nreturn 0;\n",
            ),
            new(
                "nil_branch_oob",
                "let xs = [1];\nif xs[9] == nil { return 1; }\nreturn 0;\n",
            ),
            // Writing at a negative index means what reading at one means. It
            // used to raise in both backends while `xs[-1]` read the last
            // element — the same expression, one direction.
            new(
                "negative_store",
                "let xs = [1, 2, 3];\nxs[-1] = 9;\nxs.set(-2, 8);\nprintln(xs);\nreturn 0;\n",
            ),
            // A window's negative bounds count from the end, like `xs[-1]`.
            // The VM raised on them and the native slice raised too, while the
            // *string* slice on each side did something different again.
            new(
                "slice_negative",
                "let xs = [1, 2, 3, 4, 5];\nprintln(xs.slice(-2, 5).len());\nprintln(xs.slice(1, -1).len());\nprintln(xs.slice(-99, 99).len());\nprintln(xs.slice(-1, -3).len());\nreturn 0;\n",
            ),
        ],
    );
}

#[test]
fn differential_maps() {
    run_differential(
        "maps",
        &[
            // The same rule across a call, for the other container: the
            // callee stores a value the parameter's carrier cannot hold, so
            // the caller's literal is built with a Dyn carrier. Both shapes,
            // as for lists — the erased one stores through `dyn.index_set`.
            new(
                "a_callee_widens_a_shared_map_parameter",
                "fn widen(m: Any) -> Int {\n  m[\"k\"] = \"z\";\n  return m.len();\n}\nlet a = {\"x\": 1};\nlet b = {\"y\": 1.5};\nprintln(widen(a));\nprintln(widen(b));\nprintln(a);\nprintln(b);\nreturn 0;\n",
            ),
            // An index store through a boxed receiver, which is the other
            // spelling `dyn.index_set` carries: an integer key is a position
            // on a list and a key on a map, and the negative-from-end and
            // out-of-range rules are the unboxed ones.
            new(
                "a_boxed_receiver_stores_by_index",
                "fn setit(xs: Any) -> Int {\n  xs[0] = 9;\n  xs[-1] = 8;\n  return xs.len();\n}\nlet a = [1, 2];\nlet b = [1.5, 2.5];\nprintln(setit(a));\nprintln(setit(b));\nprintln(a);\nprintln(b);\nreturn 0;\n",
            ),
            new(
                "a_boxed_receiver_store_is_bounds_checked",
                "fn setit(xs: Any) -> Int {\n  xs[5] = 9;\n  return xs.len();\n}\nlet a = [1, 2];\nlet b = [1.5, 2.5];\nprintln(setit(a));\nprintln(setit(b));\nreturn 0;\n",
            ),
            new(
                "a_callee_widens_its_only_caller_s_map",
                "fn widen(m: Any) -> Int {\n  m[\"k\"] = \"z\";\n  return m.len();\n}\nlet a = {\"x\": 1};\nprintln(widen(a));\nprintln(a);\nreturn 0;\n",
            ),
            // A map literal whose value type a later store contradicts is
            // built with a Dyn carrier — the same fixpoint retry the list
            // literals use. The VM widens the carrier in place; native cannot,
            // so both of these used to fall back.
            new(
                "widened_after_a_typed_literal",
                "let m: Map<String, Any> = {\"a\": 1};\nm[\"b\"] = \"x\";\nprintln(m);\nreturn 0;\n",
            ),
            new(
                "widened_from_an_empty_literal",
                "let n: Map<String, Any> = {};\nn[\"a\"] = 1;\nn[\"b\"] = \"y\";\nprintln(n);\nreturn 0;\n",
            ),
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
            // `needle in text` is `text.contains(needle)`. The method
            // spelling lowered and the operator sent the whole program back to
            // the VM, which is a 3x slowdown with no message.
            new(
                "in_operator_on_a_string",
                "let s = \"abc\";\nprintln(\"b\" in s);\nprintln(\"z\" in s);\nfn f(t: String) -> Bool {\n  return \"c\" in t;\n}\nprintln(f(s));\nreturn 0;\n",
            ),
            // An erased container is a container: `in` refused an `Any`
            // operand while indexing, `len`, iteration, method dispatch and
            // `push` all took one. Three carriers behind one `Dyn`, so the
            // runtime is what picks.
            new(
                "in_operator_on_an_erased_container",
                "fn has(h: Any, n: Any) -> Bool {\n  return n in h;\n}\nprintln(has(\"abc\", \"b\"));\nprintln(has(\"abc\", \"z\"));\nprintln(has([1, 2], 2));\nprintln(has([1, 2], 9));\nprintln(has({\"k\": 1}, \"k\"));\nreturn 0;\n",
            ),
            // The other two container operators followed it too: list
            // removal and map merge. Four siblings, one rule.
            new(
                "remove_and_merge_with_an_erased_operand",
                "fn rm(xs: Any) -> Int {\n  println(xs - [1]);\n  return 0;\n}\nfn mg(m: Any) -> Int {\n  println(m + {\"b\": 2});\n  return 0;\n}\nrm([1, 2]);\nmg({\"a\": 1});\nreturn 0;\n",
            ),
            // Concatenation followed the same rule as `in`, and refused the
            // same erased operand.
            new(
                "concat_with_an_erased_operand",
                "fn app(xs: Any) -> Int {\n  println(xs + [7]);\n  return 0;\n}\napp([1, 2]);\napp([1.5, 2.5]);\nreturn 0;\n",
            ),
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
            // Text → number: the whole point is that unparseable text answers
            // nil rather than guessing, so the two engines must agree on which
            // spellings are numbers. `lkrt_str_to_int` is a second
            // implementation of `lk_stdlib_string::to_int`'s String arm; this
            // is what keeps them the same one.
            new(
                "to_int_ok",
                "use string;\nprintln(string.to_int(\"42\") ?? -1);\nreturn 0;\n",
            ),
            new(
                "to_int_trims",
                "use string;\nprintln(string.to_int(\"  -7\\n\") ?? -1);\nreturn 0;\n",
            ),
            new(
                "to_int_refuses",
                "use string;\nprintln(string.to_int(\"42abc\") ?? -1);\nprintln(string.to_int(\"\") ?? -1);\nprintln(string.to_int(\"42.0\") ?? -1);\nprintln(string.to_int(\"9223372036854775808\") ?? -1);\nreturn 0;\n",
            ),
            new(
                "to_int_base",
                "use string;\nprintln(string.to_int(\"ff\", 16) ?? -1);\nprintln(string.to_int(\"-101\", 2) ?? -1);\nprintln(string.to_int(\"9\", 8) ?? -1);\nreturn 0;\n",
            ),
            // A negative `slice` bound counts from the end, like `[-1]`. The
            // four implementations had three answers for it, and the two
            // *backends* disagreed: `"abcde".slice(1, -1)` was `""` in the VM
            // and `"bcd"` compiled.
            new(
                "slice_negative",
                "println(\"abcde\".slice(-2, 5));\nprintln(\"abcde\".slice(1, -1));\nprintln(\"abcde\".slice(-99, 99));\nprintln(\"abcde\".slice(-1, -3));\nreturn 0;\n",
            ),
            new(
                "to_float_ok",
                "use string;\nprintln(string.to_float(\"3.5\") ?? -1.0);\nprintln(string.to_float(\" -2e3 \") ?? -1.0);\nprintln(string.to_float(\"nope\") ?? -1.0);\nreturn 0;\n",
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
            // Re-`let` of a promoted name in a loop body: earlier-emitted
            // reads (re-executed on the back edge) keep loading the outer
            // cell; block-scope promotions of outer locals survive restore.
            new(
                "closure_re_let_promoted_in_loop",
                "let x = 100;\nlet g = |q| q + x;\nlet i = 0;\nwhile (i < 2) {\n  println(x);\n  let x = 5;\n  println(x);\n  i = i + 1;\n}\nprintln(g(0));\nreturn 0;\n",
            ),
            new(
                "closure_block_capture_survives",
                "let y = 1;\n{\n  let f = |q| q + y;\n  println(f(0));\n}\nprintln(y);\ny = 9;\nprintln(y);\nreturn 0;\n",
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

#[test]
fn differential_global_containers() {
    run_differential(
        "global_containers",
        &[
            // Module globals holding containers cross functions as boxed Dyn
            // (typed zero-init would diverge from the VM's nil); methods on
            // them dispatch as methods (the compiler routing fix), natively
            // through the Dyn arms.
            new(
                "global_list_methods",
                "let nums = [1, 2, 3];\nfn count() { return nums.len(); }\nfn first() { return nums[0]; }\nprintln(count());\nprintln(first());\nreturn 0;\n",
            ),
            new(
                "global_str_list",
                "let names = [\"ada\", \"bob\"];\nfn count() { return names.len(); }\nfn pick() { return names[1]; }\nprintln(count());\nprintln(pick());\nreturn 0;\n",
            ),
            new(
                "global_map_field",
                "let cfg = {\"host\": \"prod.io\", \"port\": 8080};\nfn host() { return cfg.host; }\nfn port() { return cfg.port; }\nprintln(host());\nprintln(port());\nreturn 0;\n",
            ),
        ],
    );
}

#[test]
fn differential_dyn_cross_function() {
    run_differential(
        "dyn_cross_fn",
        &[
            // Disagreeing call-site types join the parameter to Dyn (each
            // site boxes); the body consumes through the Dyn arms.
            new(
                "param_join_int_str",
                "fn id(x) { return x; }\nprintln(id(1));\nprintln(id(\"s\"));\nprintln(id(2.5));\nprintln(id(true));\nreturn 0;\n",
            ),
            // A nullable (Maybe) argument crosses the call boxed: the callee
            // receives nil as nil — VM call semantics, no unwrap abort.
            new(
                "param_maybe_passes_nil",
                "fn show(x) { return x ?? -1; }\nlet m = {};\nm.set(\"a\", 1);\nprintln(show(m.get(\"a\")));\nprintln(show(m.get(\"zz\")));\nreturn 0;\n",
            ),
            // An explicit nil argument at one site, a typed value at another.
            new(
                "param_nil_vs_int",
                "fn f(x) { if (x == nil) { return 0; } return 1; }\nprintln(f(nil));\nprintln(f(3));\nreturn 0;\n",
            ),
            // VM truthiness: only nil and false are falsy — 0, 0.0 and \"\"
            // are truthy; a Dyn condition tests its tag at runtime.
            new(
                "truthiness_zero_and_nil",
                "let z = 0;\nif (z) { println(\"zero truthy\"); }\nlet n = nil;\nif (n) { println(\"unreachable\"); } else { println(\"nil falsy\"); }\nfn pick(x) { if (x) { return \"t\"; } return \"f\"; }\nprintln(pick(0));\nprintln(pick(nil));\nprintln(pick(false));\nprintln(pick(\"\"));\nreturn 0;\n",
            ),
            // A Bool-typed self-recursive return chain must not look
            // heterogeneous against the stale I64 ret default.
            new(
                "bool_recursive_ret",
                "fn has(xs, t) {\n  if (xs.len() == 0) { return false; }\n  if (xs[0] == t) { return true; }\n  return has(xs.skip(1), t);\n}\nprintln(has([1, 3, 5], 5));\nprintln(has([1, 3, 5], 4));\nreturn 0;\n",
            ),
            // Genuinely mixed return types box every return point: the
            // function returns Dyn, nil crosses as nil.
            new(
                "mixed_ret_types",
                "fn pick(n) {\n  if (n == 0) { return 0; }\n  if (n == 1) { return \"one\"; }\n  if (n == 2) { return 2.5; }\n  return nil;\n}\nprintln(pick(0));\nprintln(pick(1));\nprintln(pick(2));\nprintln(pick(9) == nil);\nreturn 0;\n",
            ),
            // A Maybe-returning function boxes: absent arrives as nil.
            new(
                "maybe_ret_boxes",
                "fn lookup(k) {\n  let m = {};\n  m.set(\"a\", 7);\n  return m.get(k);\n}\nprintln(lookup(\"a\"));\nprintln(lookup(\"zz\") == nil);\nreturn 0;\n",
            ),
            // A boxed *receiver* reaches a list method. `chain` accepted a Dyn
            // argument and not a Dyn receiver, so a list that is reset on one
            // path, extended on another, and handed to a Dyn parameter — which
            // is what a line buffer is — refused to lower. The `bare-metal-x86`
            // kernel is written exactly this way and stopped compiling for it.
            new(
                "dyn_receiver_chain",
                "fn emit(base, line) { return base + line.len(); }\nfn build(n) {\n  let line = [];\n  let out = 0;\n  let i = 0;\n  while (i < n) {\n    if (i % 4 == 0) { out = emit(out, line); line = []; }\n    else { line = line.chain([i]); }\n    i = i + 1;\n  }\n  return emit(out, line);\n}\nprintln(build(11));\nprintln(build(0));\nreturn 0;\n",
            ),
            // The rest of the boxed-receiver names whose arms already accepted
            // `ListDyn` and whose method-table row was missing, so the receiver
            // never reached them. `index_of` is here for its absent answer too:
            // a miss is nil, and nil has to survive the unboxed path.
            new(
                "dyn_receiver_element_methods",
                "fn probe(xs) { return \"\" + xs.first() + xs.last() + xs.index_of(1); }\nprintln(probe([3, 1, 2]));\nprintln(probe([3.5, 1.5]));\nprintln(probe([\"a\", \"b\"]));\nreturn 0;\n",
            ),
            // An all-nil branch join must not build a Nil-typed phi: it widens
            // to Dyn (boxed nil) and compares by tag.
            new(
                "nil_phi_join",
                "let user = { \"name\": \"Alice\", \"address\": nil };\nlet city = nil;\nif (user.address != nil) {\n  city = user.address.city;\n} else {\n  city = nil;\n}\nprintln(city == nil);\nreturn 0;\n",
            ),
        ],
    );
}

#[test]
fn differential_trait_dispatch_contract() {
    // Locks the trait-registration lowering contract (plan J1): the AOT
    // prescan pattern-matches the exact instruction sequence the VM compiler
    // emits for `trait`/`impl` blocks (`GetGlobal __lk_register_trait_impl` →
    // Load*/Move/NewList builders → `Call`). `run_differential` hard-requires
    // the MIR lowering to succeed, so a compiler change to that emission
    // shape turns the otherwise *silent* coverage loss (prescan stops
    // matching → whole module falls back to Tier 0, examples-differential
    // merely skips) into a red test here.
    run_differential(
        "trait_contract",
        &[
            // `self` inside an impl method is that type, so a method built on
            // the type's *other* methods devirtualizes. Without that
            // provenance the receiver was an untyped parameter and the whole
            // shape — which is what a trait default body always is — fell out
            // of the native subset. `run_differential` requires the lowering,
            // so this stays honest.
            new(
                "trait_method_calls_sibling",
                "trait Sz {\n  fn base(self) -> Int;\n  fn doubled(self) -> Int { return self.base() * 2; }\n  fn quad(self) -> Int { return self.doubled() * 2; }\n}\nstruct A { v: Int }\nimpl Sz for A { fn base(self) -> Int { return self.v; } }\nprintln(A { v: 5 }.base());\nprintln(A { v: 5 }.doubled());\nprintln(A { v: 5 }.quad());\nreturn 0;\n",
            ),
            // The same identity guarantee for every other carrier and every
            // other way a container reaches a mutator. None of these had
            // coverage, which is how the typed-list copy above survived: a
            // container's writes being the caller's writes is the single most
            // load-bearing thing about a reference type, and only the list
            // carrier was ever wrong.
            new(
                "every_container_carrier_keeps_its_identity",
                "struct Box { xs: List<Int> }\n\
                 trait Sink { fn take(self, n: Int) -> Int; }\n\
                 struct S { xs: List<Int> }\n\
                 impl Sink for S { fn take(self, n: Int) -> Int { self.xs.push(n); return self.xs.len(); } }\n\
                 fn put(m: Map<String, Int>, k: String, v: Int) -> Int { m.set(k, v); return m.len(); }\n\
                 fn addset(s: Set<Int>, n: Int) -> Int { s.add(n); return s.len(); }\n\
                 fn bump(p: Box, n: Int) -> Int { p.xs.push(n); return p.xs.len(); }\n\
                 fn relay(xs: List<Int>, n: Int) -> Int { return inner(xs, n); }\n\
                 fn inner(xs: List<Int>, n: Int) -> Int { xs.push(n); return xs.len(); }\n\
                 let m: Map<String, Int> = {};\nprintln(put(m, \"a\", 1));\nprintln(m.len());\n\
                 let st = Set([1]);\nprintln(addset(st, 2));\nprintln(st.len());\n\
                 let b = Box { xs: [1] };\nprintln(bump(b, 2));\nprintln(\"${b.xs}\");\n\
                 let s = S { xs: [] };\nprintln(s.take(1));\nprintln(s.take(2));\nprintln(\"${s.xs}\");\n\
                 let r: List<Int> = [];\nprintln(relay(r, 5));\nprintln(\"${r}\");\n\
                 let c: List<Int> = [1];\nlet g = |n: Int| -> Int { c.push(n); return c.len(); };\n\
                 println(g(2));\nprintln(\"${c}\");\nreturn 0;\n",
            ),
            // A container passed to a function keeps its identity — the
            // callee's writes are the caller's.
            //
            // It did not. The first fixpoint pass observes call arguments while
            // every callee's return type is still its `I64` default, and the
            // parameter lattice *joins* observations: pass 1's `I64` and pass
            // 2's real `list<i64>` disagreed, so the parameter became `Dyn` and
            // every call site boxed. A typed list boxes by **rebuilding**
            // (`list_h.i64_to_dyn`), so the callee held a copy and its `push`
            // was lost — a wrong answer that still printed a plausible length.
            // `ret_known` already existed for this hazard on the HOF re-route
            // path; the parameter lattice never got it.
            new(
                "a_container_argument_keeps_its_identity",
                "fn mk() -> List<Int> { return [1]; }\n\
                 fn add(xs: List<Int>, n: Int) -> Int { xs.push(n); return xs.len(); }\n\
                 let xs = mk();\nprintln(add(xs, 2));\nprintln(xs.len());\nprintln(\"${xs}\");\n\
                 let ys: List<Int> = [];\n\
                 println(try { \"${add(ys, 7)}\" } catch e { \"c\" });\nprintln(ys.len());\n\
                 println(\"${ys}\");\nreturn 0;\n",
            ),
            // `typeof` names the struct, and both engines agree about which
            // carriers it can decide statically. A struct instance and a plain
            // map share `MapStrDyn`, so the static table's `Map` was a wrong
            // answer for structs: `typeof(p)` read `Map` compiled and `Object`
            // interpreted — two engines, two wrong answers, neither of them the
            // struct's name.
            new(
                "typeof_names_the_struct",
                "struct S { a: Int }\nfn name_of(x: Any) -> String { return typeof(x); }\n\
                 let m = {\"a\": 1, \"b\": \"x\"};\nlet p = S { a: 1 };\n\
                 println(typeof(p));\nprintln(typeof(m));\nprintln(name_of(p));\nprintln(name_of(m));\n\
                 println(name_of(1));\nprintln(name_of(\"s\"));\nprintln(name_of([1]));\n\
                 println(typeof(1));\nprintln(typeof(1.5));\nprintln(typeof(true));\nprintln(typeof(nil));\n\
                 return 0;\n",
            ),
            // The receiver whose type the lowering *cannot* name — two call
            // sites passing different structs into one parameter, or a mixed
            // list — dispatches at run time off the arena type mark instead of
            // taking the module to the VM. It knows its type then; only the
            // already-boxed `Dyn` shape used to reach that path, and only with
            // zero arguments.
            new(
                "a_receiver_of_unknown_struct_type_dispatches_at_run_time",
                "struct A { v: Int }\nstruct B { v: Int }\n\
                 trait N { fn name(self) -> String; fn scaled(self, k: Int) -> Int;\n\
                 fn label(self, p: String, q: String) -> String; }\n\
                 impl N for A { fn name(self) -> String { return \"A\"; }\n\
                 fn scaled(self, k: Int) -> Int { return self.v * k; }\n\
                 fn label(self, p: String, q: String) -> String { return p + \"A\" + q; } }\n\
                 impl N for B { fn name(self) -> String { return \"B\"; }\n\
                 fn scaled(self, k: Int) -> Int { return self.v + k; }\n\
                 fn label(self, p: String, q: String) -> String { return p + \"B\" + q; } }\n\
                 fn describe(x: Any, k: Int) -> String { return x.name() + \":${x.scaled(k)}\" + x.label(\"<\", \">\"); }\n\
                 println(describe(A { v: 3 }, 4));\nprintln(describe(B { v: 3 }, 4));\n\
                 let xs = [A { v: 1 }, B { v: 2 }];\nfor x in xs { println(x.scaled(10)); }\nreturn 0;\n",
            ),
            // A struct that arrives as an *argument* is that type too. The
            // provenance came only from a `NewObject` the lowering saw, so it
            // survived a `return` (`ret_structs`) but not a parameter:
            // `fn area(q: P) { return q.w * q.h; }` lowered (fields need no
            // name) while `fn area(q: P) { return q.norm(); }` could not
            // devirtualize and took the whole module to the VM. Covered here
            // for a plain function, a lambda, a second argument, and a callee
            // that passes its own parameter on.
            new(
                "a_struct_argument_keeps_its_type",
                "struct P { w: Int, h: Int }\ntrait Sz { fn area(self) -> Int; }\nimpl Sz for P { fn area(self) -> Int { return self.w * self.h; } }\n\
                 fn area_of(q: P) -> Int { return q.area(); }\nfn relay(q: P) -> Int { return area_of(q); }\n\
                 fn tagged(tag: String, q: P) -> String { return tag + \"=\" + \"${q.area()}\"; }\n\
                 let f = |q: P| -> Int { return q.area() + 1; };\nlet p = P { w: 2, h: 3 };\n\
                 println(area_of(p));\nprintln(relay(p));\nprintln(tagged(\"a\", p));\nprintln(f(p));\n\
                 println(area_of(P { w: 4, h: 5 }));\nreturn 0;\n",
            ),
            // An impl method nobody calls is no longer a lowering root — and
            // `show` is the one method reached *without* a call naming it
            // (a display site does). Dropping it from the roots leaves a
            // dangling callee and the module fails MIR validation, so this
            // pins both halves at once: an uncalled `unused` alongside a
            // `show` that only `"${…}"` reaches.
            // A container in a template renders. `docs/semantics.md` used to
            // rule this a loud failure — the VM stopped doing that, and the
            // lowering kept mirroring the retired rule, so every template
            // holding a list or a struct list dropped its module to the VM.
            new(
                "container_in_template",
                "struct P { v: Int }\nlet xs = [1, 2, 3];\nlet ps = [P { v: 1 }, P { v: 2 }];\nprintln(\"${xs}\");\nprintln(\"a${xs}b\");\nprintln(\"${ps}\");\nprintln(\"n=${xs}, p=${ps}\");\nreturn 0;\n",
            ),
            // A struct with no `show` renders like the VM's default:
            // `Name{f:v,…}`, declaration order, nested values quoted.
            //
            // **Nesting is the point.** An earlier attempt spelled the
            // rendering out at the display site and printed a nested struct as
            // a hash-ordered map — a field holding a struct is a bare map by
            // then, and the display site cannot tell. The type description now
            // lives at runtime, where the mark is, so nesting recurses.
            new(
                "struct_default_display",
                "struct P { name: String, n: Int, ok: Bool, f: Float }\nstruct Outer { inner: P, tag: String }\nstruct WithList { p: P, xs: List<Int>, s: String }\nstruct E {}\nlet p = P { name: \"a, b\", n: -3, ok: true, f: 1.5 };\nlet o = Outer { inner: p, tag: \"x\" };\nlet w = WithList { p: p, xs: [1, 2], s: \"z\" };\nlet e = E {};\nprintln(\"${p}\");\nprintln(\"${o}\");\nprintln(\"${w}\");\nprintln(\"${e}\");\nprintln(p);\nreturn 0;\n",
            ),
            // A function that returns a struct carries the type name out to
            // its callers, so a method on the result devirtualizes. The name
            // used to stop at the function boundary — `make(3, 4).norm()` had
            // an untyped receiver, in one module as much as across two.
            new(
                "struct_returning_function",
                "struct Pt { x: Int, y: Int }\ntrait Norm { fn norm(self) -> Int; }\nimpl Norm for Pt { fn norm(self) -> Int { return self.x + self.y; } }\nfn make(a: Int, b: Int) -> Pt { return Pt { x: a, y: b }; }\nfn pick(c: Bool) -> Pt { if c { return make(1, 2); } return make(3, 4); }\nprintln(make(3, 4).norm());\nprintln(pick(true).norm());\nprintln(pick(false).norm());\nreturn 0;\n",
            ),
            // A named call devirtualizes like a positional one, plus the
            // argument *order*: every name is a constant, so the permutation
            // into the callee's frame order is a compile-time fact. The whole
            // opcode had no lowering, which mattered once `module.Type { … }`
            // started desugaring to one.
            new(
                "named_call_permutes_arguments",
                "fn mk({x: Int, y: Int}) -> Int { return x * 10 + y; }\nfn pos(a: Int, {b: Int}) -> Int { return a * 100 + b; }\nprintln(mk(y: 2, x: 3));\nprintln(mk(x: 1, y: 9));\nprintln(pos(7, b: 4));\nreturn 0;\n",
            ),
            new(
                "trait_show_hook_and_uncalled",
                "trait Show { fn show(self) -> String; }\nstruct R { w: Int }\nimpl Show for R { fn show(self) -> String { return \"R!\"; } }\ntrait Extra { fn unused(self, s: String) -> Int; }\nimpl Extra for R { fn unused(self, s: String) -> Int { return s.len(); } }\nlet r = R { w: 3 };\nprintln(\"${r}\");\nreturn 0;\n",
            ),
            // Two implementors, one of them never calling a method it defines.
            // Every impl method is a lowering root, so an *uncalled* one used to
            // be lowered with the `I64` parameter default and fail reading a
            // field — killing the module from a method nobody calls.
            new(
                "trait_uncalled_impl_method",
                "trait Sz {\n  fn base(self) -> Int;\n  fn doubled(self) -> Int;\n  fn quad(self) -> Int;\n}\nstruct A { v: Int }\nimpl Sz for A {\n  fn base(self) -> Int { return self.v; }\n  fn doubled(self) -> Int { return self.base() * 2; }\n  fn quad(self) -> Int { return self.doubled() * 2; }\n}\nstruct B { v: Int }\nimpl Sz for B {\n  fn base(self) -> Int { return self.v; }\n  fn doubled(self) -> Int { return self.v * 3; }\n  fn quad(self) -> Int { return self.doubled() * 2; }\n}\nprintln(A { v: 5 }.quad());\nprintln(B { v: 5 }.quad());\nreturn 0;\n",
            ),
            new(
                "trait_static_dynamic_show",
                "struct Rect { w: Int, h: Int }\nstruct Circle { r: Int }\ntrait Area { fn area(self) -> Int; }\nimpl Area for Rect { fn area(self) -> Int { return self.w * self.h; } }\nimpl Area for Circle { fn area(self) -> Int { return 3 * self.r * self.r; } }\ntrait Show { fn show(self) -> String; }\nimpl Show for Rect { fn show(self) -> String { return \"Rect(${self.w}x${self.h})\"; } }\nlet r = Rect { w: 3, h: 4 };\nprintln(r.area());\nprintln(\"${r}\");\nlet shapes = [Rect { w: 1, h: 2 }, Circle { r: 2 }];\nprintln(shapes.map(|s| s.area()));\nreturn 0;\n",
            ),
        ],
    );
}

#[test]
fn differential_concurrency_edges() {
    run_differential(
        "concurrency_edges",
        &[
            // The module spelling needs the import — on both ends. `chan` is
            // the one name that is a module *and* a bare global (the channel
            // constructor), and `chan.new(1)` compiles to the same bytecode
            // either way: the import is what replaces the global with the
            // module object at run time. Native used to resolve it regardless,
            // so an unimported program ran natively and failed under the VM.
            new(
                "the module spelling after its import",
                "use chan;\nlet c = chan.new(1);\nchan.send(c, 7);\nprintln(chan.recv(c));\nreturn 0;\n",
            ),
            // The two try/catch cases that used to live here moved to
            // `try_catch_differential` in clif_differential_test.rs: this corpus
            // runs under `LK_AOT_NO_FALLBACK=1` in CI, and a protected region has
            // no native lowering yet (todos.md). Their behaviour is still
            // covered, just without the pure-native requirement.
        ],
    );
}
