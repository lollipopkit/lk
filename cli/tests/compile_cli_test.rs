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
fn test_macro_expand_prints_expanded_source_and_trace() {
    let dir = unique_tmp_dir("macro_expand");
    ensure_clean_dir(&dir);

    write_file(
        &dir,
        "macros.lk",
        r#"
macro_rules! id {
    ($value:expr) => { $value };
}
return id!(7);
"#,
    );

    let output = run_cli(&dir, ["macro", "expand", "macros.lk", "--trace"])
        .output()
        .expect("spawn macro expand");
    assert!(
        output.status.success(),
        "macro expand failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("# macro id at"), "expected trace line, got: {stdout}");
    assert!(stdout.contains("return 7;"), "expected expanded return, got: {stdout}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_macro_expand_uses_macros_from_file_import() {
    let dir = unique_tmp_dir("macro_expand_import");
    ensure_clean_dir(&dir);

    write_file(
        &dir,
        "macros.lk",
        r#"
export macro_rules! answer {
    () => { 42 };
}
"#,
    );
    write_file(
        &dir,
        "main.lk",
        r#"
use { answer } from "macros";
return answer!();
"#,
    );

    let output = run_cli(&dir, ["macro", "expand", "main.lk"])
        .output()
        .expect("spawn macro expand");
    assert!(
        output.status.success(),
        "macro expand failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(
        stdout.contains("return 42;"),
        "expected imported macro expansion, got: {stdout}"
    );

    let run = run_cli(&dir, ["main.lk"]).output().expect("spawn source run");
    assert!(
        run.status.success(),
        "source run failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8(run.stdout).expect("utf8 stdout").trim(), "42");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_macro_expand_prints_ast_derive_expansion() {
    let dir = unique_tmp_dir("macro_expand_derive");
    ensure_clean_dir(&dir);

    write_file(
        &dir,
        "main.lk",
        r#"
#[derive(Debug)]
struct User {
    id: Int,
}
"#,
    );

    let output = run_cli(&dir, ["macro", "expand", "main.lk"])
        .output()
        .expect("spawn macro expand");
    assert!(
        output.status.success(),
        "macro expand failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(
        stdout.contains("# ast macro expansion"),
        "expected AST expansion marker, got: {stdout}"
    );
    assert!(
        stdout.contains("impl __LKShow for User"),
        "expected generated show impl, got: {stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_macro_expand_honors_cfg_feature_flag() {
    let dir = unique_tmp_dir("macro_expand_cfg_feature");
    ensure_clean_dir(&dir);

    write_file(
        &dir,
        "main.lk",
        r#"
#[cfg(feature = "debug")]
fn value() {
    return 7;
}

#[cfg(not(feature = "debug"))]
fn value() {
    return 1;
}
"#,
    );

    let output = run_cli(&dir, ["macro", "expand", "main.lk", "--feature", "debug"])
        .output()
        .expect("spawn macro expand");
    assert!(
        output.status.success(),
        "macro expand failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let ast_output = stdout
        .split("# ast macro expansion")
        .nth(1)
        .expect("expected AST expansion output");
    assert!(
        ast_output.matches("fn value()").count() == 1,
        "expected exactly one selected value function, got: {stdout}"
    );
    assert!(
        !ast_output.contains("#[cfg"),
        "expected cfg attribute to be consumed by AST expansion, got: {stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_macro_expand_uses_manifest_proc_macro_provider() {
    if !Path::new("/bin/sh").exists() {
        return;
    }
    let dir = unique_tmp_dir("macro_expand_manifest_proc_provider");
    ensure_clean_dir(&dir);
    create_dir_all(dir.join("src")).expect("create app src");

    write_file(
        &dir,
        "derive.sh",
        r#"cat >/dev/null
printf '%s' '{"protocol_version":1,"output_tokens":[{"kind":"Fn","lexeme":"fn","span":null},{"kind":"Id","lexeme":"generated","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Return","lexeme":"return","span":null},{"kind":"Int","lexeme":"99","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"RBrace","lexeme":"}","span":null}],"diagnostics":[],"dependencies":[{"path":"derive.dep","digest":"sha256:derive"}]}'
"#,
    );
    write_file(
        &dir,
        "attr.sh",
        r#"cat >/dev/null
printf '%s' '{"protocol_version":1,"output_tokens":[{"kind":"Fn","lexeme":"fn","span":null},{"kind":"Id","lexeme":"decorated","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Return","lexeme":"return","span":null},{"kind":"Int","lexeme":"7","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"RBrace","lexeme":"}","span":null}],"diagnostics":[],"dependencies":[{"path":"attribute.dep","digest":"sha256:attribute"}]}'
"#,
    );
    write_file(
        &dir,
        "function.sh",
        r#"cat >/dev/null
printf '%s' '{"protocol_version":1,"output_tokens":[{"kind":"Int","lexeme":"5","span":null}],"diagnostics":[],"dependencies":[{"path":"function.dep","digest":null}]}'
"#,
    );
    write_file(
        &dir,
        "method.sh",
        r#"cat >/dev/null
printf '%s' '{"protocol_version":1,"output_tokens":[{"kind":"Fn","lexeme":"fn","span":null},{"kind":"Id","lexeme":"value","span":null},{"kind":"LParen","lexeme":"(","span":null},{"kind":"Id","lexeme":"self","span":null},{"kind":"Colon","lexeme":":","span":null},{"kind":"Id","lexeme":"User","span":null},{"kind":"RParen","lexeme":")","span":null},{"kind":"FnArrow","lexeme":"->","span":null},{"kind":"Id","lexeme":"Int","span":null},{"kind":"LBrace","lexeme":"{","span":null},{"kind":"Return","lexeme":"return","span":null},{"kind":"Int","lexeme":"11","span":null},{"kind":"Semicolon","lexeme":";","span":null},{"kind":"RBrace","lexeme":"}","span":null}],"diagnostics":[],"dependencies":[{"path":"method.dep","digest":"sha256:method"}]}'
"#,
    );
    write_file(
        &dir,
        "Lk.toml",
        r#"
[package]
name = "app"

[macros.derive.MakeAnswer]
command = "/bin/sh"
args = ["derive.sh"]

[macros.attribute.Replace]
command = "/bin/sh"
args = ["attr.sh"]

[macros.function_like.proc_value]
command = "/bin/sh"
args = ["function.sh"]

[macros.attribute.ReplaceMethod]
command = "/bin/sh"
args = ["method.sh"]
"#,
    );
    write_file(
        &dir.join("src"),
        "main.lk",
        r#"
#[derive(MakeAnswer)]
struct User { id: Int }

#[Replace]
fn old() {
    return 1;
}

trait Value {
    fn value(self: User) -> Int;
}

impl Value for User {
    #[ReplaceMethod]
    fn value(self: User) -> Int {
        return 1;
    }
}

let user = User { id: 1 };
return generated() + decorated() + proc_value!() + user.value();
"#,
    );

    let output = run_cli(&dir, ["macro", "expand", "src/main.lk", "--deps"])
        .output()
        .expect("spawn macro expand");
    assert!(
        output.status.success(),
        "macro expand failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(
        stdout.contains("fn generated()"),
        "expected manifest provider output in AST expansion, got: {stdout}"
    );
    assert!(
        stdout.contains("fn decorated()"),
        "expected manifest attribute provider output in AST expansion, got: {stdout}"
    );
    assert!(
        stdout.contains("+ 5"),
        "expected manifest function-like provider output in token expansion, got: {stdout}"
    );
    assert!(
        stdout.contains("# proc macro dependencies")
            && stdout.contains("\"path\": \"derive.dep\"")
            && stdout.contains("\"digest\": \"sha256:derive\"")
            && stdout.contains("\"path\": \"attribute.dep\"")
            && stdout.contains("\"path\": \"function.dep\"")
            && stdout.contains("\"path\": \"method.dep\""),
        "expected dependency metadata from all provider kinds, got: {stdout}"
    );

    let run = run_cli(&dir, ["src/main.lk"]).output().expect("spawn source run");
    assert!(
        run.status.success(),
        "source run failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8(run.stdout).expect("utf8 stdout").trim(), "122");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_builtin_assertion_macros_execute_with_stdlib_globals() {
    let dir = unique_tmp_dir("builtin_assertion_macros");
    ensure_clean_dir(&dir);

    write_file(
        &dir,
        "main.lk",
        r#"
use { assert_eq, assert_ne } from macros;
assert_eq!(1, 1.0);
assert_eq!(["a", 2], ["a", 2.0], "numeric equality should coerce");
assert_ne!(1, 2);
return 42;
"#,
    );

    let run = run_cli(&dir, ["main.lk"]).output().expect("spawn source run");
    assert!(
        run.status.success(),
        "source run failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8(run.stdout).expect("utf8 stdout").trim(), "42");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_macro_expand_uses_macros_from_package_import() {
    let dir = unique_tmp_dir("macro_expand_package_import");
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
    write_file(
        &dir.join("deps/util/src"),
        "mod.lk",
        r#"
export macro_rules! answer {
    () => { 42 };
}
"#,
    );
    write_file(
        &dir.join("src"),
        "main.lk",
        r#"
use { answer } from util;
return answer!();
"#,
    );

    let output = run_cli(&dir, ["macro", "expand", "src/main.lk"])
        .output()
        .expect("spawn macro expand");
    assert!(
        output.status.success(),
        "macro expand failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(
        stdout.contains("return 42;"),
        "expected package macro expansion, got: {stdout}"
    );

    let run = run_cli(&dir, ["src/main.lk"]).output().expect("spawn source run");
    assert!(
        run.status.success(),
        "source run failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8(run.stdout).expect("utf8 stdout").trim(), "42");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_compile_writes_module_artifact_output() {
    let dir = unique_tmp_dir("module_output");
    ensure_clean_dir(&dir);

    write_file(&dir, "a.lk", "return 123;\n");

    let output = run_cli(&dir, ["compile", "bytecode", "a.lk"])
        .output()
        .expect("spawn bytecode compile");
    assert!(
        output.status.success(),
        "bytecode compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("a.lkm"),
        "compile should print output path, got: {stdout}"
    );
    let module = fs::read_to_string(dir.join("a.lkm")).expect("read module output");
    assert!(
        module.contains("\"format\":\"lk.module\"") && module.contains("\"code\""),
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
    write_file(&dir, "main.lk", "use \"fib\";\nreturn fib.iterative(10);\n");

    let output = run_cli(&dir, ["compile", "bytecode", "main.lk"])
        .output()
        .expect("spawn bytecode compile");
    assert!(
        output.status.success(),
        "bytecode compile failed: {}",
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

    let cache_entries = fs::read_dir(&cache_dir)
        .expect("read native cache")
        .map(|entry| entry.expect("cache entry").path())
        .collect::<Vec<_>>();
    let metadata_entries = cache_entries
        .iter()
        .filter(|path| {
            path.file_name()
                .and_then(|file| file.to_str())
                .is_some_and(|file| file.ends_with(".proc-macro-deps.json"))
        })
        .count();
    // Explicitly filter for executable (non-metadata) files instead of
    // subtracting metadata_entries, to avoid miscounting if extra files
    // appear in the cache directory.
    let executable_entries = cache_entries
        .iter()
        .filter(|path| {
            path.is_file()
                && !path
                    .file_name()
                    .and_then(|f| f.to_str())
                    .is_some_and(|f| f.ends_with(".proc-macro-deps.json"))
        })
        .count();
    assert_eq!(executable_entries, 1, "expected one cached native executable");
    assert_eq!(metadata_entries, 1, "expected one native cache dependency sidecar");

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
    write_file(&dir, "src/main.lk", "use util;\nreturn util.answer();\n");

    let run_out = run_cli(&dir, ["src/main.lk"]).output().expect("spawn run");
    assert!(
        run_out.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&run_out.stderr)
    );
    assert_eq!(String::from_utf8(run_out.stdout).expect("utf8 stdout").trim(), "42");

    let compile = run_cli(&dir, ["compile", "bytecode", "src/main.lk"])
        .output()
        .expect("spawn compile");
    assert!(
        compile.status.success(),
        "bytecode compile failed: {}",
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

    let output = run_cli(&dir, ["compile", "bytecode", "mod.lk"])
        .output()
        .expect("spawn bytecode compile");
    assert!(
        output.status.success(),
        "bytecode compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let module = fs::read_to_string(dir.join("mod.lkm")).expect("read module output");
    assert!(
        module.contains("\"format\":\"lk.module\""),
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
