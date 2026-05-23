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
fn test_compile_writes_instr32_module_output() {
    let dir = unique_tmp_dir("lkb_pos");
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
        module.contains("\"format\": \"lk.module32\"") && module.contains("\"code\""),
        "expected Instr32 module artifact, got: {module}"
    );
    let run = run_cli(&dir, ["a.lkm"]).output().expect("spawn module run");
    assert!(
        run.status.success(),
        "module run failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8(run.stdout).expect("utf8 stdout").trim(), "123");
    assert!(!dir.join("a.lkb").exists(), "compile must not emit old LKB bytecode");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_compile_with_import_writes_instr32_module_output() {
    let dir = unique_tmp_dir("lkb_import");
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
    assert!(!dir.join("main.lkb").exists(), "compile must not emit old LKB bytecode");

    let _ = fs::remove_dir_all(&dir);
}

#[cfg(feature = "llvm")]
#[test]
fn test_llvm_compile_targets_are_disabled_during_instr32_migration() {
    let dir = unique_tmp_dir("llvm_disabled");
    ensure_clean_dir(&dir);
    write_file(&dir, "a.lk", "return 123;\n");

    let llvm = run_cli(&dir, ["compile", "llvm", "a.lk"])
        .output()
        .expect("spawn llvm compile");
    assert!(!llvm.status.success(), "LLVM IR output should be disabled");
    let stderr = String::from_utf8_lossy(&llvm.stderr);
    assert!(
        stderr.contains("LLVM IR output is disabled during the Instr32 VM migration"),
        "expected LLVM migration error, got: {stderr}"
    );
    assert!(!dir.join("a.ll").exists(), "disabled LLVM output must not emit IR");

    let exe = run_cli(&dir, ["compile", "exe", "a.lk"])
        .output()
        .expect("spawn exe compile");
    assert!(!exe.status.success(), "native executable output should be disabled");
    let stderr = String::from_utf8_lossy(&exe.stderr);
    assert!(
        stderr.contains("native executable output is disabled during the Instr32 VM migration"),
        "expected native migration error, got: {stderr}"
    );
    assert!(
        !dir.join("a").exists(),
        "disabled native output must not emit executable"
    );

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
    assert!(
        dir.join("src/main.lkm").exists(),
        "compile should emit Instr32 module output"
    );
    let run_module = run_cli(&dir, ["src/main.lkm"]).output().expect("spawn module run");
    assert!(
        run_module.status.success(),
        "module run failed: {}",
        String::from_utf8_lossy(&run_module.stderr)
    );
    assert_eq!(String::from_utf8(run_module.stdout).expect("utf8 stdout").trim(), "42");
    assert!(
        !dir.join("src/main.lkb").exists(),
        "compile must not emit old LKB bytecode"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_compile_struct_constructs_to_instr32_module() {
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
        module.contains("\"format\": \"lk.module32\""),
        "expected module artifact, got: {module}"
    );
}

#[test]
fn test_lkb_negative_corrupted_magic() {
    let dir = unique_tmp_dir("lkb_neg");
    ensure_clean_dir(&dir);

    // Write a fake LKB file with only the magic header and junk
    let bad_path = dir.join("bad.lkb");
    let mut f = File::create(&bad_path).expect("create bad lkb");
    f.write_all(b"LKBjunk").expect("write bad bytes");

    let out = run_cli(&dir, ["bad.lkb"]).output().expect("spawn run");

    assert!(!out.status.success(), "expected failure for removed LKB input");
    let stderr = String::from_utf8(out.stderr).expect("utf8 stderr");
    assert!(
        stderr.contains("LKB execution has been removed"),
        "stderr did not contain removed-LKB error, got: {}",
        stderr
    );

    // Cleanup
    let _ = fs::remove_dir_all(&dir);
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
