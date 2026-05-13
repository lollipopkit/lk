use std::ffi::OsStr;
use std::fs::{self, File, create_dir_all};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use lk_core::{stmt::ModuleResolver, val::Val, vm};

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
fn test_lkb_positive_compile_and_run() {
    let dir = unique_tmp_dir("lkb_pos");
    ensure_clean_dir(&dir);

    // Create a simple source file that returns 123
    write_file(&dir, "a.lk", "return 123;\n");

    // Compile to LKB (output to a.lkb next to source)
    let output = run_cli(&dir, ["compile", "a.lk"]).output().expect("spawn compile");
    assert!(
        output.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let lkb_path = dir.join("a.lkb");
    assert!(lkb_path.exists(), "expected compiled bytecode at {:?}", lkb_path);

    // Run the LKB file; expect it to print the return value
    let run_out = run_cli(&dir, ["a.lkb"]).output().expect("spawn run");
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
fn test_lkb_compile_with_import_bundled() {
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

    let bytes = fs::read(dir.join("main.lkb")).expect("read bytecode");
    let decoded = vm::decode_module(&bytes).expect("decode module");
    assert_eq!(decoded.bundled_modules.len(), 1, "expected bundled module");
    assert!(
        decoded.bundled_modules[0].path.ends_with("fib.lk"),
        "unexpected bundled path: {}",
        decoded.bundled_modules[0].path
    );
    let resolver = ModuleResolver::new();
    resolver.register_embedded_module("fib.lk", decoded.bundled_modules[0].module.clone());
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

    let fib_path = dir.join("fib.lk");
    fs::remove_file(&fib_path).expect("remove fib module");

    let run_out = run_cli(&dir, ["main.lkb"]).output().expect("spawn run");
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
    let decoded =
        vm::decode_module(&fs::read(dir.join("src/main.lkb")).expect("read bytecode")).expect("decode module");
    assert_eq!(decoded.bundled_modules.len(), 1);
    assert!(
        decoded
            .meta
            .as_ref()
            .and_then(|meta| meta.tags.get("package_modules"))
            .is_some(),
        "expected package module metadata"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_compile_rejects_unsupported_constructs_for_vm() {
    let dir = unique_tmp_dir("compile_vm_guard");
    ensure_clean_dir(&dir);

    // Program uses `struct`, which the VM compiler now supports.
    write_file(
        &dir,
        "mod.lk",
        "struct Point { x: Int, y: Int }\nreturn Point { x: 1, y: 2 };\n",
    );

    let output = run_cli(&dir, ["compile", "mod.lk"]).output().expect("spawn compile");
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
fn test_lkb_negative_corrupted_magic() {
    let dir = unique_tmp_dir("lkb_neg");
    ensure_clean_dir(&dir);

    // Write a fake LKB file with only the magic header and junk
    let bad_path = dir.join("bad.lkb");
    let mut f = File::create(&bad_path).expect("create bad lkb");
    f.write_all(b"LKBjunk").expect("write bad bytes");

    let out = run_cli(&dir, ["bad.lkb"]).output().expect("spawn run");

    assert!(!out.status.success(), "expected failure for corrupted LKB");
    let stderr = String::from_utf8(out.stderr).expect("utf8 stderr");
    assert!(
        stderr.contains("Failed to decode LKB from"),
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
