use std::ffi::OsStr;
use std::fs::{self, File, create_dir_all};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use lkr_core::{stmt::ModuleResolver, val::Val, vm};

fn bin_path() -> PathBuf {
    // Cargo exposes built binary path for tests via this env var
    PathBuf::from(env!("CARGO_BIN_EXE_lkr"))
}

fn unique_tmp_dir(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let pid = std::process::id();
    let uniq = format!("lkr_{}_{}", name, pid);
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
fn test_lkrb_positive_compile_and_run() {
    let dir = unique_tmp_dir("lkrb_pos");
    ensure_clean_dir(&dir);

    // Create a simple source file that returns 123
    write_file(&dir, "a.lkr", "return 123;\n");

    // Compile to LKRB (output to a.lkrb next to source)
    let output = run_cli(&dir, ["compile", "a.lkr"]).output().expect("spawn compile");
    assert!(
        output.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let lkrb_path = dir.join("a.lkrb");
    assert!(lkrb_path.exists(), "expected compiled bytecode at {:?}", lkrb_path);

    // Run the LKRB file; expect it to print the return value
    let run_out = run_cli(&dir, ["a.lkrb"]).output().expect("spawn run");
    assert!(
        run_out.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&run_out.stderr)
    );
    let stdout = String::from_utf8(run_out.stdout).expect("utf8 stdout");
    assert_eq!(stdout.trim(), "123");

    // Best-effort cleanup
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_lkrb_compile_with_import_bundled() {
    let dir = unique_tmp_dir("lkrb_import");
    ensure_clean_dir(&dir);

    write_file(
        &dir,
        "fib.lkr",
        "fn iterative(n) {\n    if (n <= 1) { return n; }\n    let a = 0;\n    let b = 1;\n    for _ in 2..=n {\n        let t = a + b;\n        a = b;\n        b = t;\n    }\n    return b;\n}\n",
    );
    write_file(&dir, "main.lkr", "import \"fib\";\nreturn fib.iterative(10);\n");

    let output = run_cli(&dir, ["compile", "main.lkr"]).output().expect("spawn compile");
    assert!(
        output.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let bytes = fs::read(dir.join("main.lkrb")).expect("read bytecode");
    let decoded = vm::decode_module(&bytes).expect("decode module");
    assert_eq!(decoded.bundled_modules.len(), 1, "expected bundled module");
    assert!(
        decoded.bundled_modules[0].path.ends_with("fib.lkr"),
        "unexpected bundled path: {}",
        decoded.bundled_modules[0].path
    );
    let resolver = ModuleResolver::new();
    resolver.register_embedded_module("fib.lkr", decoded.bundled_modules[0].module.clone());
    let embedded = resolver.resolve_file("fib").expect("resolve embedded");
    match embedded {
        Val::Map(map) => {
            assert!(
                map.contains_key("iterative"),
                "embedded module missing iterative export"
            );
        }
        other => panic!("unexpected embedded module type: {:?}", other.type_name()),
    }

    let fib_path = dir.join("fib.lkr");
    fs::remove_file(&fib_path).expect("remove fib module");

    let run_out = run_cli(&dir, ["main.lkrb"]).output().expect("spawn run");
    assert!(
        run_out.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&run_out.stderr)
    );
    let stdout = String::from_utf8(run_out.stdout).expect("utf8 stdout");
    assert_eq!(stdout.trim(), "55");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_compile_rejects_unsupported_constructs_for_vm() {
    let dir = unique_tmp_dir("compile_vm_guard");
    ensure_clean_dir(&dir);

    // Program uses `struct`, which the VM compiler now supports.
    write_file(
        &dir,
        "mod.lkr",
        "struct Point { x: Int, y: Int }\nreturn Point { x: 1, y: 2 };\n",
    );

    let output = run_cli(&dir, ["compile", "mod.lkr"]).output().expect("spawn compile");
    assert!(
        output.status.success(),
        "compile should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Compiled successfully") || stdout.is_empty(),
        "expected successful compilation, got: {stdout}"
    );
}

#[test]
fn test_lkrb_negative_corrupted_magic() {
    let dir = unique_tmp_dir("lkrb_neg");
    ensure_clean_dir(&dir);

    // Write a fake LKRB file with only the magic header and junk
    let bad_path = dir.join("bad.lkrb");
    let mut f = File::create(&bad_path).expect("create bad lkrb");
    f.write_all(b"LKRBjunk").expect("write bad bytes");

    let out = run_cli(&dir, ["bad.lkrb"]).output().expect("spawn run");

    assert!(!out.status.success(), "expected failure for corrupted LKRB");
    let stderr = String::from_utf8(out.stderr).expect("utf8 stderr");
    assert!(
        stderr.contains("Failed to decode LKRB from"),
        "stderr did not contain decode error, got: {}",
        stderr
    );

    // Cleanup
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_compile_rejects_parent_directory_argument() {
    let dir = unique_tmp_dir("compile_parent");
    ensure_clean_dir(&dir);

    let out = run_cli(&dir, ["compile", "../escape.lkr"])
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

    let out = run_cli(&dir, ["missing.lkr"]).output().expect("spawn run missing file");
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
    write_file(&dir, "bad.lkr", "let x = ;\n");

    let out = run_cli(&dir, ["bad.lkr"]).output().expect("spawn run parse error");
    assert!(!out.status.success(), "expected parse failure");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Error:"), "expected parse diagnostics, got: {stderr}");

    let _ = fs::remove_dir_all(&dir);
}
