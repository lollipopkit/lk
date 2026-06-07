use std::ffi::OsStr;
use std::fs::{self, File, create_dir_all};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

fn bin_path() -> PathBuf {
    // Cargo exposes built binary path for tests via this env var
    PathBuf::from(env!("CARGO_BIN_EXE_lk"))
}

fn unique_tmp_dir(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let pid = std::process::id();
    let uniq = format!("lk_{}_{}", name, pid);
    p.push(uniq);
    p
}

fn run_cli<I, S>(dir: &Path, args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new(bin_path());
    cmd.current_dir(dir).args(args);
    cmd
}

fn write_file(dir: &Path, name: &str, contents: &str) {
    let path = dir.join(name);
    let mut file = File::create(&path).expect("create file");
    file.write_all(contents.as_bytes()).expect("write file");
}

fn ensure_clean_dir(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
    create_dir_all(dir).expect("create tmp dir");
}

#[test]
fn test_compile_writes_module_artifact_output() {
    let dir = unique_tmp_dir("module_output");
    ensure_clean_dir(&dir);

    write_file(&dir, "a.lk", "return 123;\n");

    let output = run_cli(&dir, ["compile", "a.lk"]).output().expect("spawn compile");
    assert!(
        output.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("a.lkm"),
        "compile should print output path, got: {stdout}"
    );
    let module = fs::read_to_string(dir.join("a.lkm")).expect("read module output");
    assert!(
        module.contains("\"format\": \"lk.module\"") && module.contains("\"code\""),
        "expected module artifact, got: {module}"
    );
    let run = run_cli(&dir, ["a.lkm"]).output().expect("spawn module run");
    assert!(
        run.status.success(),
        "module run failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8(run.stdout).expect("utf8 stdout").trim(), "123");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_compile_with_import_writes_module_artifact_output() {
    let dir = unique_tmp_dir("module_import");
    ensure_clean_dir(&dir);

    write_file(
        &dir,
        "fib.lk",
        "fn iterative(n) {\n    if (n <= 1) { return n; }\n    let a = 0;\n    let b = 1;\n    for _ in 2..=n {\n        let t = a + b;\n        a = b;\n        b = t;\n    }\n    return b;\n}\n",
    );
    write_file(&dir, "main.lk", "import \"fib\";\nreturn fib.iterative(10);\n");

    let output = run_cli(&dir, ["compile", "main.lk"]).output().expect("spawn compile");
    assert!(
        output.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let module = fs::read_to_string(dir.join("main.lkm")).expect("read module output");
    assert!(
        module.contains("\"imports\"") && module.contains("\"module\""),
        "expected module artifact with imports, got: {module}"
    );
    let run = run_cli(&dir, ["main.lkm"]).output().expect("spawn module run");
    assert!(
        run.status.success(),
        "module run failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8(run.stdout).expect("utf8 stdout").trim(), "55");

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(feature = "llvm")]
#[test]
fn test_llvm_compile_lowers_simple_i64_return_without_vm_shell() {
    let dir = unique_tmp_dir("llvm_native_i64");
    ensure_clean_dir(&dir);
    write_file(&dir, "a.lk", "return 123;\n");

    let llvm = run_cli(&dir, ["compile", "llvm", "a.lk"])
        .output()
        .expect("spawn llvm compile");
    assert!(
        llvm.status.success(),
        "LLVM IR compile failed: {}",
        String::from_utf8_lossy(&llvm.stderr)
    );
    let stdout = String::from_utf8_lossy(&llvm.stdout);
    assert!(stdout.contains("a.ll"), "expected output path, got: {stdout}");
    let ir = fs::read_to_string(dir.join("a.ll")).expect("read LLVM IR");
    assert!(
        !ir.contains("@lk_module_json"),
        "simple i64 return should not embed artifact shell: {ir}"
    );
    assert!(
        !ir.contains("lk_rt_run_module_json"),
        "simple i64 return should not call artifact runtime: {ir}"
    );
    assert!(
        ir.contains("declare i32 @printf(ptr, ...)"),
        "expected native print lowering: {ir}"
    );

    let exe = run_cli(&dir, ["compile", "exe", "a.lk"])
        .env("RUSTC", dir.join("missing-rustc"))
        .output()
        .expect("spawn exe compile");
    assert!(
        exe.status.success(),
        "native executable compile failed: {}",
        String::from_utf8_lossy(&exe.stderr)
    );
    let stdout = String::from_utf8_lossy(&exe.stdout);
    assert!(stdout.contains("a"), "expected executable output path, got: {stdout}");
    assert!(dir.join("a").exists(), "native executable output should be emitted");
    let run_exe = Command::new(dir.join("a")).output().expect("spawn compiled executable");
    assert!(
        run_exe.status.success(),
        "compiled executable failed: {}",
        String::from_utf8_lossy(&run_exe.stderr)
    );
    assert_eq!(String::from_utf8(run_exe.stdout).expect("utf8 stdout").trim(), "123");

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(feature = "llvm")]
#[test]
fn test_source_run_defaults_to_vm_and_cached_native_is_opt_in() {
    let dir = unique_tmp_dir("native_cache_run");
    ensure_clean_dir(&dir);
    let cache_dir = dir.join("cache");
    write_file(&dir, "a.lk", "let x = 40; return x + 2;\n");

    let default_vm = run_cli(&dir, ["a.lk"])
        .env("LK_NATIVE_CACHE_DIR", &cache_dir)
        .output()
        .expect("spawn default VM source run");
    assert!(
        default_vm.status.success(),
        "default VM source run failed: {}",
        String::from_utf8_lossy(&default_vm.stderr)
    );
    assert_eq!(String::from_utf8(default_vm.stdout).expect("utf8 stdout").trim(), "42");
    assert!(
        !cache_dir.exists(),
        "direct source run should not populate native cache unless LK_NATIVE_RUN=1"
    );

    for _ in 0..2 {
        let output = run_cli(&dir, ["a.lk"])
            .env("LK_NATIVE_RUN", "1")
            .env("LK_NATIVE_CACHE_DIR", &cache_dir)
            .output()
            .expect("spawn native opt-in source run");
        assert!(
            output.status.success(),
            "native opt-in source run failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout).expect("utf8 stdout").trim(), "42");
    }

    let cache_entries = fs::read_dir(&cache_dir).expect("read native cache").count();
    assert_eq!(cache_entries, 1, "expected one cached native executable");

    let vm = run_cli(&dir, ["a.lk"])
        .env("LK_NATIVE_CACHE_DIR", &cache_dir)
        .env("LK_NATIVE_RUN", "1")
        .env("LK_FORCE_VM", "1")
        .output()
        .expect("spawn forced VM source run");
    assert!(
        vm.status.success(),
        "forced VM source run failed: {}",
        String::from_utf8_lossy(&vm.stderr)
    );
    assert_eq!(String::from_utf8(vm.stdout).expect("utf8 stdout").trim(), "42");

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(feature = "llvm")]
#[test]
fn test_llvm_compile_lowers_unused_stdlib_import_metadata() {
    let dir = unique_tmp_dir("llvm_unused_stdlib_import");
    ensure_clean_dir(&dir);
    write_file(&dir, "a.lk", "import math;\nreturn 123;\n");

    let llvm = run_cli(&dir, ["compile", "llvm", "a.lk"])
        .output()
        .expect("spawn llvm compile");
    assert!(
        llvm.status.success(),
        "LLVM IR compile failed: {}",
        String::from_utf8_lossy(&llvm.stderr)
    );
    let ir = fs::read_to_string(dir.join("a.ll")).expect("read LLVM IR");
    assert!(
        !ir.contains("@lk_module_json"),
        "unused import metadata should not force artifact shell: {ir}"
    );
    assert!(ir.contains("@lk_i64_fmt"), "expected native i64 print lowering: {ir}");

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(feature = "llvm")]
#[test]
fn test_llvm_compile_exe_rejects_unsupported_shape_without_host_launcher() {
    let dir = unique_tmp_dir("llvm_exe_unsupported_shape");
    ensure_clean_dir(&dir);
    write_file(&dir, "unsupported.lk", "return !([1, 2, 3]);\n");

    let exe = run_cli(&dir, ["compile", "exe", "unsupported.lk"])
        .env("LK_CLANG", dir.join("missing-clang"))
        .output()
        .expect("spawn exe compile");
    assert!(
        !exe.status.success(),
        "unsupported runtime value should not compile through host launcher"
    );
    let stderr = String::from_utf8_lossy(&exe.stderr);
    assert!(
        stderr.contains("LLVM native lowering does not support"),
        "unexpected stderr: {stderr}"
    );
    assert!(!stderr.contains("spawn clang"), "unexpected stderr: {stderr}");
    assert!(
        !dir.join("unsupported").exists(),
        "host launcher output should not be emitted"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(feature = "llvm")]
#[test]
fn test_llvm_compile_lowers_i64_loop_without_vm_shell() {
    let dir = unique_tmp_dir("llvm_native_i64_loop");
    ensure_clean_dir(&dir);
    write_file(
        &dir,
        "loop.lk",
        "let i = 0;\nlet sum = 0;\nwhile (i < 4) {\n    sum = sum + i;\n    i = i + 1;\n}\nreturn sum;\n",
    );

    let llvm = run_cli(&dir, ["compile", "llvm", "loop.lk"])
        .output()
        .expect("spawn llvm compile");
    assert!(
        llvm.status.success(),
        "LLVM IR compile failed: {}",
        String::from_utf8_lossy(&llvm.stderr)
    );
    let ir = fs::read_to_string(dir.join("loop.ll")).expect("read LLVM IR");
    assert!(
        !ir.contains("@lk_module_json"),
        "i64 loop should not embed artifact shell: {ir}"
    );
    assert!(
        !ir.contains("lk_rt_run_module_json"),
        "i64 loop should not call artifact runtime: {ir}"
    );
    assert!(
        ir.contains("%g0.slot = alloca i64"),
        "expected i64 global slot lowering: {ir}"
    );
    assert!(ir.contains("br label %bb"), "expected native CFG lowering: {ir}");

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(feature = "llvm")]
#[test]
fn test_llvm_compile_lowers_bool_return_without_vm_shell() {
    let dir = unique_tmp_dir("llvm_native_bool");
    ensure_clean_dir(&dir);
    write_file(&dir, "bool.lk", "return true;\n");

    let llvm = run_cli(&dir, ["compile", "llvm", "bool.lk"])
        .output()
        .expect("spawn llvm compile");
    assert!(
        llvm.status.success(),
        "LLVM IR compile failed: {}",
        String::from_utf8_lossy(&llvm.stderr)
    );
    let ir = fs::read_to_string(dir.join("bool.ll")).expect("read LLVM IR");
    assert!(
        !ir.contains("@lk_module_json"),
        "bool return should not embed artifact shell: {ir}"
    );
    assert!(
        !ir.contains("lk_rt_run_module_json"),
        "bool return should not call artifact runtime: {ir}"
    );
    assert!(ir.contains("@lk_bool_true"), "expected bool text lowering: {ir}");
    assert!(ir.contains("@lk_str_fmt"), "expected string print lowering: {ir}");

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(feature = "llvm")]
#[test]
fn test_llvm_compile_lowers_nil_return_without_vm_shell() {
    let dir = unique_tmp_dir("llvm_native_nil");
    ensure_clean_dir(&dir);
    write_file(&dir, "nil.lk", "return nil;\n");

    let llvm = run_cli(&dir, ["compile", "llvm", "nil.lk"])
        .output()
        .expect("spawn llvm compile");
    assert!(
        llvm.status.success(),
        "LLVM IR compile failed: {}",
        String::from_utf8_lossy(&llvm.stderr)
    );
    let ir = fs::read_to_string(dir.join("nil.ll")).expect("read LLVM IR");
    assert!(
        !ir.contains("@lk_module_json"),
        "nil return should not embed artifact shell: {ir}"
    );
    assert!(
        !ir.contains("lk_rt_run_module_json"),
        "nil return should not call artifact runtime: {ir}"
    );
    assert!(ir.contains("@lk_nil_text"), "expected nil text lowering: {ir}");
    assert!(ir.contains("@lk_str_fmt"), "expected string print lowering: {ir}");

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(feature = "llvm")]
#[test]
fn test_llvm_compile_lowers_short_string_return_without_vm_shell() {
    let dir = unique_tmp_dir("llvm_native_short_string");
    ensure_clean_dir(&dir);
    write_file(&dir, "string.lk", "return \"ok\";\n");

    let llvm = run_cli(&dir, ["compile", "llvm", "string.lk"])
        .output()
        .expect("spawn llvm compile");
    assert!(
        llvm.status.success(),
        "LLVM IR compile failed: {}",
        String::from_utf8_lossy(&llvm.stderr)
    );
    let ir = fs::read_to_string(dir.join("string.ll")).expect("read LLVM IR");
    assert!(
        !ir.contains("@lk_module_json"),
        "short string return should not embed artifact shell: {ir}"
    );
    assert!(
        !ir.contains("lk_rt_run_module_json"),
        "short string return should not call artifact runtime: {ir}"
    );
    assert!(ir.contains("@lk_str_fmt"), "expected string print lowering: {ir}");
    assert!(ir.contains("@lk_const_str_0"), "expected string const lowering: {ir}");
    assert!(ir.contains("c\"ok\\00\""), "expected string bytes lowering: {ir}");

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(feature = "llvm")]
#[test]
fn test_llvm_compile_lowers_long_string_return_without_vm_shell() {
    let dir = unique_tmp_dir("llvm_native_long_string");
    ensure_clean_dir(&dir);
    write_file(&dir, "long_string.lk", "return \"longer-than-short\";\n");

    let llvm = run_cli(&dir, ["compile", "llvm", "long_string.lk"])
        .output()
        .expect("spawn llvm compile");
    assert!(
        llvm.status.success(),
        "LLVM IR compile failed: {}",
        String::from_utf8_lossy(&llvm.stderr)
    );
    let ir = fs::read_to_string(dir.join("long_string.ll")).expect("read LLVM IR");
    assert!(
        !ir.contains("@lk_module_json"),
        "long string return should not embed artifact shell: {ir}"
    );
    assert!(
        !ir.contains("lk_rt_run_module_json"),
        "long string return should not call artifact runtime: {ir}"
    );
    assert!(ir.contains("@lk_str_fmt"), "expected string print lowering: {ir}");
    assert!(
        ir.contains("@lk_const_heap_str_0"),
        "expected heap string const lowering: {ir}"
    );
    assert!(
        ir.contains("c\"longer-than-short\\00\""),
        "expected string bytes lowering: {ir}"
    );

    let exe = run_cli(&dir, ["compile", "exe", "long_string.lk"])
        .env("RUSTC", dir.join("missing-rustc"))
        .output()
        .expect("spawn exe compile");
    assert!(
        exe.status.success(),
        "native executable compile failed: {}",
        String::from_utf8_lossy(&exe.stderr)
    );
    let run_exe = Command::new(dir.join("long_string"))
        .output()
        .expect("spawn compiled executable");
    assert!(
        run_exe.status.success(),
        "compiled executable failed: {}",
        String::from_utf8_lossy(&run_exe.stderr)
    );
    assert_eq!(
        String::from_utf8(run_exe.stdout).expect("utf8 stdout").trim(),
        "longer-than-short"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(feature = "llvm")]
#[test]
fn test_llvm_compile_lowers_const_list_return_without_vm_shell() {
    let dir = unique_tmp_dir("llvm_native_const_list");
    ensure_clean_dir(&dir);
    write_file(&dir, "list.lk", "return [1, true, \"longer-than-short\"];\n");

    let llvm = run_cli(&dir, ["compile", "llvm", "list.lk"])
        .output()
        .expect("spawn llvm compile");
    assert!(
        llvm.status.success(),
        "LLVM IR compile failed: {}",
        String::from_utf8_lossy(&llvm.stderr)
    );
    let ir = fs::read_to_string(dir.join("list.ll")).expect("read LLVM IR");
    assert!(
        !ir.contains("@lk_module_json"),
        "const list return should not embed artifact shell: {ir}"
    );
    assert!(
        !ir.contains("lk_rt_run_module_json"),
        "const list return should not call artifact runtime: {ir}"
    );
    assert!(ir.contains("@lk_str_fmt"), "expected string print lowering: {ir}");
    assert!(
        ir.contains("@lk_const_heap_list_0"),
        "expected const list lowering: {ir}"
    );
    assert!(
        ir.contains("c\"[1, true, longer-than-short]\\00\""),
        "expected list display bytes lowering: {ir}"
    );

    let exe = run_cli(&dir, ["compile", "exe", "list.lk"])
        .env("RUSTC", dir.join("missing-rustc"))
        .output()
        .expect("spawn exe compile");
    assert!(
        exe.status.success(),
        "native executable compile failed: {}",
        String::from_utf8_lossy(&exe.stderr)
    );
    let run_exe = Command::new(dir.join("list"))
        .output()
        .expect("spawn compiled executable");
    assert!(
        run_exe.status.success(),
        "compiled executable failed: {}",
        String::from_utf8_lossy(&run_exe.stderr)
    );
    assert_eq!(
        String::from_utf8(run_exe.stdout).expect("utf8 stdout").trim(),
        "[1, true, longer-than-short]"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(feature = "llvm")]
#[test]
fn test_llvm_compile_lowers_const_map_return_without_vm_shell() {
    let dir = unique_tmp_dir("llvm_native_const_map");
    ensure_clean_dir(&dir);
    write_file(&dir, "map.lk", "return {\"a\": 1, \"b\": true};\n");

    let llvm = run_cli(&dir, ["compile", "llvm", "map.lk"])
        .output()
        .expect("spawn llvm compile");
    assert!(
        llvm.status.success(),
        "LLVM IR compile failed: {}",
        String::from_utf8_lossy(&llvm.stderr)
    );
    let ir = fs::read_to_string(dir.join("map.ll")).expect("read LLVM IR");
    assert!(
        !ir.contains("@lk_module_json"),
        "const map return should not embed artifact shell: {ir}"
    );
    assert!(
        !ir.contains("lk_rt_run_module_json"),
        "const map return should not call artifact runtime: {ir}"
    );
    assert!(ir.contains("@lk_str_fmt"), "expected string print lowering: {ir}");
    assert!(ir.contains("@lk_const_heap_map_0"), "expected const map lowering: {ir}");
    assert!(
        ir.contains("c\"{a: 1, b: true}\\00\""),
        "expected map display bytes lowering: {ir}"
    );

    let exe = run_cli(&dir, ["compile", "exe", "map.lk"])
        .env("RUSTC", dir.join("missing-rustc"))
        .output()
        .expect("spawn exe compile");
    assert!(
        exe.status.success(),
        "native executable compile failed: {}",
        String::from_utf8_lossy(&exe.stderr)
    );
    let run_exe = Command::new(dir.join("map"))
        .output()
        .expect("spawn compiled executable");
    assert!(
        run_exe.status.success(),
        "compiled executable failed: {}",
        String::from_utf8_lossy(&run_exe.stderr)
    );
    assert_eq!(
        String::from_utf8(run_exe.stdout).expect("utf8 stdout").trim(),
        "{a: 1, b: true}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(feature = "llvm")]
#[test]
fn test_llvm_compile_lowers_zero_arg_direct_function_call_without_vm_shell() {
    let dir = unique_tmp_dir("llvm_native_direct_call");
    ensure_clean_dir(&dir);
    write_file(&dir, "call.lk", "fn f() { let x = 40; return x + 2; }\nreturn f();\n");

    let llvm = run_cli(&dir, ["compile", "llvm", "call.lk"])
        .output()
        .expect("spawn llvm compile");
    assert!(
        llvm.status.success(),
        "LLVM IR compile failed: {}",
        String::from_utf8_lossy(&llvm.stderr)
    );
    let ir = fs::read_to_string(dir.join("call.ll")).expect("read LLVM IR");
    assert!(
        !ir.contains("@lk_module_json"),
        "direct call return should not embed artifact shell: {ir}"
    );
    assert!(
        !ir.contains("lk_rt_run_module_json"),
        "direct call return should not call artifact runtime: {ir}"
    );
    assert!(ir.contains("@lk_i64_fmt"), "expected i64 print lowering: {ir}");
    assert!(
        ir.contains("add i64"),
        "expected direct call native i64 arithmetic lowering: {ir}"
    );

    let exe = run_cli(&dir, ["compile", "exe", "call.lk"])
        .env("RUSTC", dir.join("missing-rustc"))
        .output()
        .expect("spawn exe compile");
    assert!(
        exe.status.success(),
        "native executable compile failed: {}",
        String::from_utf8_lossy(&exe.stderr)
    );
    let run_exe = Command::new(dir.join("call"))
        .output()
        .expect("spawn compiled executable");
    assert!(
        run_exe.status.success(),
        "compiled executable failed: {}",
        String::from_utf8_lossy(&run_exe.stderr)
    );
    assert_eq!(String::from_utf8(run_exe.stdout).expect("utf8 stdout").trim(), "42");

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(feature = "llvm")]
#[test]
fn test_llvm_compile_lowers_zero_arg_direct_f64_call_without_vm_shell() {
    let dir = unique_tmp_dir("llvm_native_direct_f64_call");
    ensure_clean_dir(&dir);
    write_file(
        &dir,
        "call_float.lk",
        "fn f() { let x = 1.5; return x + 2.25; }\nreturn f();\n",
    );

    let llvm = run_cli(&dir, ["compile", "llvm", "call_float.lk"])
        .output()
        .expect("spawn llvm compile");
    assert!(
        llvm.status.success(),
        "LLVM IR compile failed: {}",
        String::from_utf8_lossy(&llvm.stderr)
    );
    let ir = fs::read_to_string(dir.join("call_float.ll")).expect("read LLVM IR");
    assert!(
        !ir.contains("@lk_module_json"),
        "direct f64 call return should not embed artifact shell: {ir}"
    );
    assert!(
        !ir.contains("lk_rt_run_module_json"),
        "direct f64 call return should not call artifact runtime: {ir}"
    );
    assert!(ir.contains("@lk_f64_fmt"), "expected f64 print lowering: {ir}");
    assert!(
        ir.contains("store double 3.75"),
        "expected direct f64 call constant result lowering: {ir}"
    );

    let exe = run_cli(&dir, ["compile", "exe", "call_float.lk"])
        .env("RUSTC", dir.join("missing-rustc"))
        .output()
        .expect("spawn exe compile");
    assert!(
        exe.status.success(),
        "native executable compile failed: {}",
        String::from_utf8_lossy(&exe.stderr)
    );
    let run_exe = Command::new(dir.join("call_float"))
        .output()
        .expect("spawn compiled executable");
    assert!(
        run_exe.status.success(),
        "compiled executable failed: {}",
        String::from_utf8_lossy(&run_exe.stderr)
    );
    assert_eq!(String::from_utf8(run_exe.stdout).expect("utf8 stdout").trim(), "3.75");

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(feature = "llvm")]
#[test]
fn test_llvm_compile_lowers_zero_arg_direct_compare_call_without_vm_shell() {
    let dir = unique_tmp_dir("llvm_native_direct_compare_call");
    ensure_clean_dir(&dir);
    write_file(
        &dir,
        "call_compare.lk",
        "fn f() { let x = 1.5; return x < 2.25; }\nreturn f();\n",
    );

    let llvm = run_cli(&dir, ["compile", "llvm", "call_compare.lk"])
        .output()
        .expect("spawn llvm compile");
    assert!(
        llvm.status.success(),
        "LLVM IR compile failed: {}",
        String::from_utf8_lossy(&llvm.stderr)
    );
    let ir = fs::read_to_string(dir.join("call_compare.ll")).expect("read LLVM IR");
    assert!(
        !ir.contains("@lk_module_json"),
        "direct compare call return should not embed artifact shell: {ir}"
    );
    assert!(
        !ir.contains("lk_rt_run_module_json"),
        "direct compare call return should not call artifact runtime: {ir}"
    );
    assert!(ir.contains("@lk_bool_true"), "expected bool print lowering: {ir}");

    let exe = run_cli(&dir, ["compile", "exe", "call_compare.lk"])
        .env("RUSTC", dir.join("missing-rustc"))
        .output()
        .expect("spawn exe compile");
    assert!(
        exe.status.success(),
        "native executable compile failed: {}",
        String::from_utf8_lossy(&exe.stderr)
    );
    let run_exe = Command::new(dir.join("call_compare"))
        .output()
        .expect("spawn compiled executable");
    assert!(
        run_exe.status.success(),
        "compiled executable failed: {}",
        String::from_utf8_lossy(&run_exe.stderr)
    );
    assert_eq!(String::from_utf8(run_exe.stdout).expect("utf8 stdout").trim(), "true");

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(feature = "llvm")]
#[test]
fn test_llvm_compile_lowers_positional_direct_call_without_vm_shell() {
    let dir = unique_tmp_dir("llvm_native_direct_arg_call");
    ensure_clean_dir(&dir);
    write_file(&dir, "call_arg.lk", "fn f(x) { return x + 1; }\nreturn f(41);\n");

    let llvm = run_cli(&dir, ["compile", "llvm", "call_arg.lk"])
        .output()
        .expect("spawn llvm compile");
    assert!(
        llvm.status.success(),
        "LLVM IR compile failed: {}",
        String::from_utf8_lossy(&llvm.stderr)
    );
    let ir = fs::read_to_string(dir.join("call_arg.ll")).expect("read LLVM IR");
    assert!(
        !ir.contains("@lk_module_json"),
        "positional direct call should not embed artifact shell: {ir}"
    );
    assert!(
        !ir.contains("lk_rt_run_module_json"),
        "positional direct call should not call artifact runtime: {ir}"
    );
    assert!(ir.contains("@lk_i64_fmt"), "expected i64 print lowering: {ir}");
    assert!(ir.contains("i64 42"), "expected direct arg call constant result: {ir}");

    let exe = run_cli(&dir, ["compile", "exe", "call_arg.lk"])
        .env("RUSTC", dir.join("missing-rustc"))
        .output()
        .expect("spawn exe compile");
    assert!(
        exe.status.success(),
        "native executable compile failed: {}",
        String::from_utf8_lossy(&exe.stderr)
    );
    let run_exe = Command::new(dir.join("call_arg"))
        .output()
        .expect("spawn compiled executable");
    assert!(
        run_exe.status.success(),
        "compiled executable failed: {}",
        String::from_utf8_lossy(&run_exe.stderr)
    );
    assert_eq!(String::from_utf8(run_exe.stdout).expect("utf8 stdout").trim(), "42");

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(feature = "llvm")]
#[test]
fn test_llvm_compile_lowers_f64_positional_direct_call_without_vm_shell() {
    let dir = unique_tmp_dir("llvm_native_direct_f64_arg_call");
    ensure_clean_dir(&dir);
    write_file(
        &dir,
        "call_f64_arg.lk",
        "fn f(x) { return x + 2.25; }\nreturn f(1.5);\n",
    );

    let llvm = run_cli(&dir, ["compile", "llvm", "call_f64_arg.lk"])
        .output()
        .expect("spawn llvm compile");
    assert!(
        llvm.status.success(),
        "LLVM IR compile failed: {}",
        String::from_utf8_lossy(&llvm.stderr)
    );
    let ir = fs::read_to_string(dir.join("call_f64_arg.ll")).expect("read LLVM IR");
    assert!(
        !ir.contains("@lk_module_json"),
        "f64 positional direct call should not embed artifact shell: {ir}"
    );
    assert!(
        !ir.contains("lk_rt_run_module_json"),
        "f64 positional direct call should not call artifact runtime: {ir}"
    );
    assert!(ir.contains("@lk_f64_fmt"), "expected f64 print lowering: {ir}");
    assert!(
        ir.contains("store double 3.75"),
        "expected f64 direct arg call constant result: {ir}"
    );

    let exe = run_cli(&dir, ["compile", "exe", "call_f64_arg.lk"])
        .env("RUSTC", dir.join("missing-rustc"))
        .output()
        .expect("spawn exe compile");
    assert!(
        exe.status.success(),
        "native executable compile failed: {}",
        String::from_utf8_lossy(&exe.stderr)
    );
    let run_exe = Command::new(dir.join("call_f64_arg"))
        .output()
        .expect("spawn compiled executable");
    assert!(
        run_exe.status.success(),
        "compiled executable failed: {}",
        String::from_utf8_lossy(&run_exe.stderr)
    );
    assert_eq!(String::from_utf8(run_exe.stdout).expect("utf8 stdout").trim(), "3.75");

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(feature = "llvm")]
#[test]
fn test_llvm_compile_lowers_f64_return_without_vm_shell() {
    let dir = unique_tmp_dir("llvm_native_f64");
    ensure_clean_dir(&dir);
    write_file(&dir, "float.lk", "let x = 1.5;\nlet y = 2.25;\nreturn x + y;\n");

    let llvm = run_cli(&dir, ["compile", "llvm", "float.lk"])
        .output()
        .expect("spawn llvm compile");
    assert!(
        llvm.status.success(),
        "LLVM IR compile failed: {}",
        String::from_utf8_lossy(&llvm.stderr)
    );
    let ir = fs::read_to_string(dir.join("float.ll")).expect("read LLVM IR");
    assert!(
        !ir.contains("@lk_module_json"),
        "f64 return should not embed artifact shell: {ir}"
    );
    assert!(
        !ir.contains("lk_rt_run_module_json"),
        "f64 return should not call artifact runtime: {ir}"
    );
    assert!(ir.contains("@lk_f64_fmt"), "expected f64 print lowering: {ir}");
    assert!(
        ir.contains("store double 3.75"),
        "expected native f64 arithmetic constant lowering: {ir}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(feature = "llvm")]
#[test]
fn test_llvm_compile_lowers_f64_branch_without_vm_shell() {
    let dir = unique_tmp_dir("llvm_native_f64_branch");
    ensure_clean_dir(&dir);
    write_file(
        &dir,
        "float_branch.lk",
        "let x = 1.5;\nlet y = 2.25;\nif (x < y) { return true; }\nreturn false;\n",
    );

    let llvm = run_cli(&dir, ["compile", "llvm", "float_branch.lk"])
        .output()
        .expect("spawn llvm compile");
    assert!(
        llvm.status.success(),
        "LLVM IR compile failed: {}",
        String::from_utf8_lossy(&llvm.stderr)
    );
    let ir = fs::read_to_string(dir.join("float_branch.ll")).expect("read LLVM IR");
    assert!(
        !ir.contains("@lk_module_json"),
        "f64 branch should not embed artifact shell: {ir}"
    );
    assert!(
        !ir.contains("lk_rt_run_module_json"),
        "f64 branch should not call artifact runtime: {ir}"
    );
    assert!(ir.contains("fcmp olt double"), "expected native f64 comparison: {ir}");
    assert!(ir.contains("@lk_bool_true"), "expected native bool return: {ir}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_package_path_dependency_runs_and_bundles() {
    let dir = unique_tmp_dir("pkg_path_dep");
    ensure_clean_dir(&dir);
    create_dir_all(dir.join("src")).expect("create app src");
    create_dir_all(dir.join("deps/util/src")).expect("create dep src");
    write_file(
        &dir,
        "Lk.toml",
        r#"
[package]
name = "app"

[dependencies]
util = { path = "deps/util" }
"#,
    );
    write_file(
        &dir.join("deps/util"),
        "Lk.toml",
        r#"
[package]
name = "util"
"#,
    );
    write_file(&dir.join("deps/util"), "src/mod.lk", "fn answer() { return 42; }\n");
    write_file(&dir, "src/main.lk", "import util;\nreturn util.answer();\n");

    let run_out = run_cli(&dir, ["src/main.lk"]).output().expect("spawn run");
    assert!(
        run_out.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&run_out.stderr)
    );
    assert_eq!(String::from_utf8(run_out.stdout).expect("utf8 stdout").trim(), "42");

    let compile = run_cli(&dir, ["compile", "src/main.lk"])
        .output()
        .expect("spawn compile");
    assert!(
        compile.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    assert!(dir.join("src/main.lkm").exists(), "compile should emit module artifact");
    let run_module = run_cli(&dir, ["src/main.lkm"]).output().expect("spawn module run");
    assert!(
        run_module.status.success(),
        "module run failed: {}",
        String::from_utf8_lossy(&run_module.stderr)
    );
    assert_eq!(String::from_utf8(run_module.stdout).expect("utf8 stdout").trim(), "42");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_compile_struct_constructs_to_module_artifact() {
    let dir = unique_tmp_dir("compile_vm_guard");
    ensure_clean_dir(&dir);

    write_file(
        &dir,
        "mod.lk",
        "struct Point { x: Int, y: Int }\nreturn Point { x: 1, y: 2 };\n",
    );

    let output = run_cli(&dir, ["compile", "mod.lk"]).output().expect("spawn compile");
    assert!(
        output.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let module = fs::read_to_string(dir.join("mod.lkm")).expect("read module output");
    assert!(
        module.contains("\"format\": \"lk.module\""),
        "expected module artifact, got: {module}"
    );
}

#[test]
fn test_compile_rejects_parent_directory_argument() {
    let dir = unique_tmp_dir("compile_parent");
    ensure_clean_dir(&dir);

    let out = run_cli(&dir, ["compile", "../escape.lk"])
        .output()
        .expect("spawn compile with parent dir");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Parent directory components"),
        "expected sanitize error, got: {stderr}"
    );
}

#[test]
fn test_run_missing_file_reports_error() {
    let dir = unique_tmp_dir("missing_file");
    ensure_clean_dir(&dir);

    let out = run_cli(&dir, ["missing.lk"]).output().expect("spawn run missing file");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Failed to read file"),
        "expected read error, got: {stderr}"
    );
}

#[test]
fn test_run_parse_error_returns_non_zero() {
    let dir = unique_tmp_dir("parse_error");
    ensure_clean_dir(&dir);
    write_file(&dir, "bad.lk", "let x = ;\n");

    let out = run_cli(&dir, ["bad.lk"]).output().expect("spawn run parse error");
    assert!(!out.status.success(), "expected parse failure");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Error:"), "expected parse diagnostics, got: {stderr}");

    let _ = fs::remove_dir_all(&dir);
}
