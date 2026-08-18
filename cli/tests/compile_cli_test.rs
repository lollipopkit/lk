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

#[cfg(feature = "aot")]
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

/// `..` in a path argument is a path, not an attack.
///
/// This asserted the opposite: a `sanitize_path` refused every `..`, while
/// letting an **absolute** path through — so it stopped nothing (anything `..`
/// reaches, `/…` reaches) and refused `lk compile ../x.lk` from a
/// subdirectory. Now the only failure left is the honest one: the file is not
/// there.
#[test]
fn compile_takes_a_parent_directory_argument_as_a_path() {
    let dir = unique_tmp_dir("compile_parent");
    ensure_clean_dir(&dir);
    let nested = dir.join("nested");
    create_dir_all(&nested).expect("nested dir");
    write_file(&dir, "escape.lk", "return 7;\n");

    let out = run_cli(&nested, ["compile", "bytecode", "../escape.lk"])
        .output()
        .expect("spawn compile with parent dir");
    assert!(
        out.status.success(),
        "compiling `../escape.lk` failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // And a `..` that really is not there fails for that reason, not for its
    // shape.
    let out = run_cli(&nested, ["compile", "bytecode", "../nope.lk"])
        .output()
        .expect("spawn compile with a missing parent-dir file");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Failed to read file"), "{stderr}");
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

/// `lk FILE`, `lk check FILE` and `lk compile FILE` must agree on whether a
/// program is valid.
///
/// They did not: `Program::execute_with_ctx` type-checks before running, but
/// the compile path went straight to codegen. A program the VM rejected at run
/// time therefore compiled to a native binary that *ran successfully* and
/// printed the ill-typed value — a silent divergence no differential test
/// caught, because neither the corpus nor the fuzzer emits an annotation that
/// contradicts its initializer.
///
/// `aot`-gated: without the native backend `lk compile` fails with "native
/// backend disabled at build time", which is not the type error this asserts.
#[test]
#[cfg(feature = "aot")]
fn test_compile_rejects_what_run_and_check_reject() {
    let dir = unique_tmp_dir("type_check_parity");
    ensure_clean_dir(&dir);
    write_file(&dir, "bad.lk", "let x: Int = \"s\";\nprintln(x);\n");

    let run = run_cli(&dir, ["bad.lk"]).output().expect("spawn run");
    let check = run_cli(&dir, ["check", "bad.lk"]).output().expect("spawn check");
    let compile = run_cli(&dir, ["compile", "bad.lk"]).output().expect("spawn compile");

    for (name, out) in [("run", &run), ("check", &check), ("compile", &compile)] {
        assert!(!out.status.success(), "`lk {name}` accepted an ill-typed program");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("Type mismatch"),
            "`lk {name}` should report the type error, got: {stderr}"
        );
    }
    // And nothing was produced for the rejected program.
    assert!(!dir.join("bad").exists(), "a rejected program must not leave a binary");

    let _ = fs::remove_dir_all(&dir);
}

/// What `lk check FILE` accepts, `lk FILE` must run.
///
/// It did not. The run path type-checks the program *twice*: the CLI does it with
/// the imports seeded, and `execute_with_ctx` then does it again with a fresh
/// checker and `None` for the directory — so the second one cannot open the files
/// the program imports and rejects every name that crosses a module boundary.
/// `lk check` passed this file and `lk` answered `Unknown type 'P' in parameter
/// 'p'`, which makes the pre-flight command a liar about the one thing it is for.
///
/// The CLI's own check stays: it is the only one the sandboxed (`LK_FUEL`) and
/// bytecode-cache branches get. That this path now checks twice is a startup
/// cost, not a correctness one.
#[test]
fn what_check_accepts_the_run_path_accepts() {
    let dir = unique_tmp_dir("check_and_run_agree");
    ensure_clean_dir(&dir);
    write_file(
        &dir,
        "lib.lk",
        "struct P { x: Int, y: Int }\n\
         impl P { fn sum(self) -> Int { return self.x + self.y; } }\n\
         fn make() -> P { return P { x: 10, y: 20 }; }\n",
    );
    // The parameter annotation names an imported type, and the body calls a
    // method the imported `impl` declares — the two things the unseeded check
    // could not resolve.
    write_file(
        &dir,
        "main.lk",
        "use \"./lib\";\nfn take(p: P) -> Int { return p.sum(); }\nprintln(take(lib.make()));\n",
    );

    let checked = run_cli(&dir, ["check", "main.lk"]).output().expect("spawn check");
    assert!(
        checked.status.success(),
        "`lk check` rejected it: {}",
        String::from_utf8_lossy(&checked.stderr)
    );

    let out = run_cli(&dir, ["main.lk"]).output().expect("spawn run");
    assert!(
        out.status.success(),
        "`lk check` passed but running failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "30");

    let _ = fs::remove_dir_all(&dir);
}

/// An imported `impl`'s method signatures reach the checker.
///
/// A method becomes known to the checker by being *type-checked* — the `Impl`
/// arm sets the self type and each method body's check registers its signature —
/// and that only ever happens for the program's own statements. So a call on an
/// imported type was not merely unchecked, it was *unknown*, and unknown falls
/// through to `Any`: in one file `impl Show for Int` made `a.show(1, 2)` an
/// error, and with the impl one `use` away the same call passed the checker and
/// died at run time.
///
/// The signature is read from the declaration, so this only makes the arity and
/// the annotated types visible — an unannotated parameter stays `Any`, exactly as
/// it is for an imported free function.
#[test]
fn an_imported_impls_signatures_are_checked() {
    let dir = unique_tmp_dir("imported_impl_sigs");
    ensure_clean_dir(&dir);
    write_file(
        &dir,
        "lib.lk",
        "struct P { x: Int }\n\
         impl P { fn scaled(self, k: Int) -> Int { return self.x * k; } }\n\
         fn make() -> P { return P { x: 2 }; }\n",
    );

    for (body, expected) in [
        ("println(lib.make().scaled());", "Method expects 1 arguments"),
        ("println(lib.make().scaled(1, 2));", "Method expects 1 arguments"),
        ("println(lib.make().scaled(\"s\"));", "wrong type"),
    ] {
        write_file(&dir, "main.lk", &format!("use \"./lib\";\n{body}\n"));
        let out = run_cli(&dir, ["check", "main.lk"]).output().expect("spawn check");
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            !out.status.success() && stderr.contains(expected),
            "`{body}` should be refused with {expected:?}, got: {stderr}"
        );
    }

    // And the correct call still passes both.
    write_file(&dir, "main.lk", "use \"./lib\";\nprintln(lib.make().scaled(3));\n");
    let out = run_cli(&dir, ["main.lk"]).output().expect("spawn run");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "6");

    let _ = fs::remove_dir_all(&dir);
}

/// A `trait` implemented in an imported file must dispatch in the importer.
///
/// It did not: the importer executes an imported file in a throwaway
/// `VmContext` and keeps only its exported *values*, so the module's `impl`
/// blocks were dropped on the floor and `make(4).area()` failed with
/// "Object has no method 'area'". The method table now travels with the
/// import, bound to the imported module's own function table and heap.
#[test]
fn test_trait_impl_from_imported_file_dispatches() {
    let dir = unique_tmp_dir("cross_module_impl");
    ensure_clean_dir(&dir);
    write_file(
        &dir,
        "shape.lk",
        "trait Area { fn area(self) -> Int; }\n\
         struct Sq { s: Int }\n\
         impl Area for Sq { fn area(self) -> Int { return self.s * self.s; } }\n\
         fn make(n: Int) -> Sq { return Sq { s: n }; }\n",
    );
    write_file(
        &dir,
        "main.lk",
        "use { make } from \"./shape.lk\";\nlet sq = make(4);\nprintln(sq.area());\n",
    );

    let out = run_cli(&dir, ["main.lk"]).output().expect("spawn run");
    assert!(
        out.status.success(),
        "cross-module trait dispatch failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "16");

    let _ = fs::remove_dir_all(&dir);
}

/// A trait used as a **type** must accept an implementor from another file.
///
/// The trait, the struct and the impl's *methods* all crossed the boundary
/// already; the relation "this type implements this trait" did not, because
/// only the importing program's own statements were walked for it. So
/// `render(v: Shape)` in an imported file reported "expected Shape, got Sq"
/// for the very type that file declares an impl for — the feature worked
/// within one file and nowhere else.
#[test]
fn test_trait_as_a_type_accepts_an_imported_implementor() {
    let dir = unique_tmp_dir("cross_module_trait_type");
    ensure_clean_dir(&dir);
    write_file(
        &dir,
        "shape.lk",
        "trait Area { fn area(self) -> Int; }\n\
         struct Sq { s: Int }\n\
         impl Area for Sq { fn area(self) -> Int { return self.s * self.s; } }\n\
         fn make(n: Int) -> Sq { return Sq { s: n }; }\n\
         fn describe(v: Area) -> Int { return v.area(); }\n",
    );
    write_file(
        &dir,
        "main.lk",
        "use { make, describe } from \"./shape.lk\";\nprintln(describe(make(5)));\n",
    );

    let out = run_cli(&dir, ["main.lk"]).output().expect("spawn run");
    assert!(
        out.status.success(),
        "a trait-typed parameter refused an imported implementor: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "25");

    let _ = fs::remove_dir_all(&dir);
}

/// One trait may be implemented for a builtin type only once across the whole
/// program.
///
/// A builtin has no declaring module, so every module's impls for it share one
/// scope and the later registration silently overwrote the earlier: with two
/// modules implementing `Doubler for Int`, `(5).dbl()` answered whichever was
/// imported *last*, so moving a `use` line changed the result.
#[test]
fn test_overlapping_builtin_impl_is_rejected() {
    let dir = unique_tmp_dir("builtin_impl_overlap");
    ensure_clean_dir(&dir);
    for (file, factor) in [("modA.lk", 2), ("modB.lk", 3)] {
        write_file(
            &dir,
            file,
            &format!(
                "trait Doubler {{ fn dbl(self) -> Int; }}\n\
                 impl Doubler for Int {{ fn dbl(self) -> Int {{ return self * {factor}; }} }}\n\
                 fn mk() -> Int {{ return 1; }}\n"
            ),
        );
    }

    // One module implementing it is fine.
    write_file(
        &dir,
        "one.lk",
        "use * as A from \"./modA\";\nprintln(A.mk());\nprintln((5).dbl());\n",
    );
    let out = run_cli(&dir, ["one.lk"]).output().expect("spawn run");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim().lines().collect::<Vec<_>>(),
        ["1", "10"]
    );

    // Two is the overlap, and it must be reported rather than resolved by
    // import order.
    write_file(
        &dir,
        "both.lk",
        "use * as A from \"./modA\";\nuse * as B from \"./modB\";\nprintln((5).dbl());\n",
    );
    let out = run_cli(&dir, ["both.lk"]).output().expect("spawn run");
    assert!(!out.status.success(), "an overlapping builtin impl must not run");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("conflicting `impl Doubler for Int`"),
        "the error must name the trait and the type, got: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// `try`/`catch` is a statement, not a closure — three things that were wrong
/// while it was rewritten in the parser into `try$call(|| { body })`.
///
/// All three are the same cause (the body was a *closure*), and all three were
/// silent or hard failures rather than diagnostics.
#[test]
fn test_try_catch_is_a_statement_not_a_closure() {
    let dir = unique_tmp_dir("try_catch_statement");
    ensure_clean_dir(&dir);

    // 1. `return` inside the body returns from the enclosing function. It used
    //    to return from the closure, so this printed the catch's value.
    write_file(
        &dir,
        "ret.lk",
        "fn f(x: Int) -> Int {\n  try { return 42; } catch e { return 1; }\n}\nprintln(f(1));\n",
    );
    let out = run_cli(&dir, ["ret.lk"]).output().expect("spawn run");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "42");

    // 2. A top-level body writing an outer local. This failed at runtime in the
    //    cell-capture machinery: "StoreCellVal expected UpvalCell object".
    write_file(
        &dir,
        "outer.lk",
        // `% 0` rather than `/ 0`: `/` yields a Float, so dividing by zero is
        // an infinity now and raises nothing. Integer remainder still has no
        // answer at zero, which is what this case needs — it is about try's
        // scoping, not about division.
        "let t = 0;\nfor i in 0..100 {\n  try { t += i % 0; } catch e { t += 1; }\n}\nprintln(t);\n",
    );
    let out = run_cli(&dir, ["outer.lk"]).output().expect("spawn run");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "100");

    // 3. A top-level `let` was not even *visible* inside the body — the closure
    //    put it out of reach and this failed to compile with
    //    "Compiler undefined local/global `acc`".
    write_file(
        &dir,
        "visible.lk",
        "let acc = [];\ntry { for i in 0..3 { acc.push(i); } } catch e {}\nprintln(acc.len());\n",
    );
    let out = run_cli(&dir, ["visible.lk"]).output().expect("spawn run");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "3");

    // 4. A `return` inside the body returns from the enclosing function, so it
    //    has to take part in return-type checking. It did not, so an ill-typed
    //    `return` inside a `try` passed `lk check` silently.
    write_file(
        &dir,
        "rettype.lk",
        "fn f() -> Int {\n  try { return \"not an int\"; } catch e { return 2; }\n}\nprintln(f());\n",
    );
    let out = run_cli(&dir, ["check", "rettype.lk"]).output().expect("spawn check");
    assert!(
        !out.status.success(),
        "an ill-typed return inside `try` must be rejected"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("Return type mismatch"),
        "got: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // 5. A catch name shadowing a local that a closure promoted to a capture
    //    cell: the binding is fresh (so the handler must not read it through
    //    `LoadCellVal`) and the outer mark must come back afterwards (so the
    //    shadowed local must not read as the raw cell).
    write_file(
        &dir,
        "shadow.lk",
        "fn f() {\n  let e = 0;\n  let bump = || { e = e + 1; };\n  bump();\n\
         try { 1 % 0; } catch e { println(\"caught\"); }\n  println(e);\n}\nf();\n",
    );
    let out = run_cli(&dir, ["shadow.lk"]).output().expect("spawn run");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim().lines().collect::<Vec<_>>(),
        ["caught", "1"],
        "the catch binding is fresh, and the shadowed cell local survives the scope"
    );

    // The value a catch binds is unchanged from the `pcall` era: the raised
    // value itself for `error(v)`, the message string for anything else.
    write_file(
        &dir,
        "bind.lk",
        "try { error([1, 2]); } catch e { println(typeof(e)); }\ntry { 1 % 0; } catch e { println(typeof(e)); }\n",
    );
    let out = run_cli(&dir, ["bind.lk"]).output().expect("spawn run");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim().lines().collect::<Vec<_>>(),
        ["List", "String"]
    );

    let _ = fs::remove_dir_all(&dir);
}

/// The reverse direction: an `impl` declared *here*, dispatched inside a
/// function imported from another file.
///
/// The body belongs to this module, the executing frame belongs to the other
/// one, and a function index means nothing outside its own table — so index *N*
/// used to be resolved against the imported module and this exact program
/// recursed into `render` itself until the stack overflowed. It now runs the
/// right body, with the declaring module's globals swapped in for the call
/// (`docs/vm-cross-module-dispatch.md`).
#[test]
fn test_local_trait_impl_dispatches_inside_an_imported_function() {
    let dir = unique_tmp_dir("foreign_frame_impl");
    ensure_clean_dir(&dir);
    write_file(&dir, "render.lk", "fn render(q) { return q.area(); }\n");
    write_file(
        &dir,
        "main.lk",
        "use { render } from \"./render.lk\";\n\
         struct Sq { s: Int }\n\
         trait Area { fn area(self) -> Int; }\n\
         impl Area for Sq { fn area(self) -> Int { return self.s * self.s; } }\n\
         println(render(Sq { s: 4 }));\n",
    );

    let out = run_cli(&dir, ["main.lk"]).output().expect("spawn run");
    assert!(
        out.status.success(),
        "dispatch from a foreign frame failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "16");

    // A method that *writes* a module global cannot cross the boundary: the
    // write would land in the temporary global table the call runs against and
    // be dropped on restore. That is reported, not approximated.
    write_file(
        &dir,
        "main.lk",
        "use { render } from \"./render.lk\";\n\
         let counter = 0;\n\
         struct Sq { s: Int }\n\
         trait Area { fn area(self) -> Int; }\n\
         impl Area for Sq { fn area(self) -> Int { counter = counter + 1; return counter; } }\n\
         println(render(Sq { s: 4 }));\n",
    );
    let out = run_cli(&dir, ["main.lk"]).output().expect("spawn run");
    assert!(!out.status.success(), "a global-writing method must not silently run");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Sq::area") && stderr.contains("writes a module global"),
        "the refusal must name the method and the reason, got: {stderr}"
    );

    // Same method, dispatched from its own module's frame: unaffected.
    write_file(
        &dir,
        "main.lk",
        "let counter = 0;\n\
         struct Sq { s: Int }\n\
         trait Area { fn area(self) -> Int; }\n\
         impl Area for Sq { fn area(self) -> Int { counter = counter + 1; return counter; } }\n\
         let q = Sq { s: 4 };\n\
         println(q.area());\n\
         println(q.area());\n",
    );
    let out = run_cli(&dir, ["main.lk"]).output().expect("spawn run");
    assert!(
        out.status.success(),
        "same-module dispatch must be unaffected: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim().lines().collect::<Vec<_>>(),
        ["1", "2"]
    );

    let _ = fs::remove_dir_all(&dir);
}

/// Two modules may each declare their own `Point`, and each must keep its own
/// methods.
///
/// They did not. Objects carried a bare `"Point"` and the dispatch table was
/// keyed by that name alone, so whichever module registered last owned the name
/// for the whole context: `A.mk(1).tag()` answered `"B"`. A locally declared
/// `Point` hijacked the imported one the same way. Both halves of a declared
/// type's identity — the declaring module and the name — now travel with the
/// value (`lk_core::val::TypeScope`).
#[test]
fn test_same_type_name_in_two_modules_dispatches_separately() {
    let dir = unique_tmp_dir("type_scope_collision");
    ensure_clean_dir(&dir);
    for (file, tag) in [("a.lk", "A"), ("b.lk", "B")] {
        write_file(
            &dir,
            file,
            &format!(
                "struct Point {{ v: Int }}\n\
                 trait Tagged {{ fn tag(self) -> String; }}\n\
                 impl Tagged for Point {{ fn tag(self) -> String {{ return \"{tag}\"; }} }}\n\
                 fn mk(v: Int) -> Point {{ return Point {{ v: v }}; }}\n"
            ),
        );
    }
    write_file(
        &dir,
        "main.lk",
        "use * as A from \"./a\";\n\
         use * as B from \"./b\";\n\
         struct Point { v: Int }\n\
         trait Tagged { fn tag(self) -> String; }\n\
         impl Tagged for Point { fn tag(self) -> String { return \"LOCAL\"; } }\n\
         println(A.mk(1).tag());\n\
         println(B.mk(2).tag());\n\
         println(Point { v: 3 }.tag());\n",
    );

    let out = run_cli(&dir, ["main.lk"]).output().expect("spawn run");
    assert!(
        out.status.success(),
        "same-named types across modules failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim().lines().collect::<Vec<_>>(),
        ["A", "B", "LOCAL"],
        "each `Point` must run its own module's `tag`"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// An impl reaches the importer even when the value comes from *deeper* than
/// the modules it named.
///
/// `main` imports `mid`, `mid` imports `leaf`, and `mid.passthru()` hands back
/// a `leaf` struct. The importer collected impls one level deep, so `leaf`'s
/// module was never registered and the call failed outright with "Object has no
/// method 'depth'". Registration now covers the resolver's transitive closure,
/// which is safe precisely because entries are scoped and cannot collide.
#[test]
fn test_trait_impl_from_a_transitive_import_dispatches() {
    let dir = unique_tmp_dir("transitive_impl");
    ensure_clean_dir(&dir);
    write_file(
        &dir,
        "leaf.lk",
        "struct Deep { v: Int }\n\
         trait Depth { fn depth(self) -> Int; }\n\
         impl Depth for Deep { fn depth(self) -> Int { return self.v * 10; } }\n\
         fn mk(v: Int) -> Deep { return Deep { v: v }; }\n",
    );
    write_file(
        &dir,
        "mid.lk",
        "use * as L from \"./leaf\";\nfn passthru(v: Int) -> Deep { return L.mk(v); }\n",
    );
    write_file(
        &dir,
        "main.lk",
        "use * as M from \"./mid\";\nprintln(M.passthru(3).depth());\n",
    );

    let out = run_cli(&dir, ["main.lk"]).output().expect("spawn run");
    assert!(
        out.status.success(),
        "transitive trait dispatch failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "30");

    let _ = fs::remove_dir_all(&dir);
}

/// `lk compile object:<triple>` emits a relocatable object for that target and
/// stops, without linking.
///
/// This is the bare-metal path: the linker script, entry point and memory map
/// belong to the board, so LK hands over an object the way a C library does and
/// lets the board's existing build place it.
///
/// The architecture is read out of the ELF header rather than by shelling out
/// to a disassembler — the host's `objdump` is usually x86-only and cannot even
/// read an aarch64 object, which is precisely the situation this feature is for.
#[test]
#[cfg(feature = "aot")]
fn compile_emits_an_object_for_a_cross_target() {
    let dir = unique_tmp_dir("compile_object_cross");
    ensure_clean_dir(&dir);
    write_file(
        &dir,
        "add.lk",
        "fn add(a: Int, b: Int) -> Int { return a + b; }\nreturn add(2, 3);\n",
    );

    let output = run_cli(&dir, ["compile", "object:aarch64-unknown-none", "add.lk"])
        .output()
        .expect("run lk compile");
    assert!(
        output.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let object = std::fs::read(dir.join("add.o")).expect("object was written");
    assert_eq!(&object[..4], b"\x7fELF", "not an ELF object");
    // e_machine: 183 is EM_AARCH64.
    assert_eq!(
        u16::from_le_bytes([object[18], object[19]]),
        183,
        "object is not aarch64"
    );

    // The runtime calls stay undefined — they are resolved when the board links
    // this against an `lkrt` built for the same target.
    let symbols = String::from_utf8_lossy(&object);
    assert!(symbols.contains("lkrt_"), "expected unresolved lkrt references");

    let _ = std::fs::remove_dir_all(&dir);
}

/// An unknown triple must fail loudly rather than quietly building for the host.
#[test]
#[cfg(feature = "aot")]
fn compile_object_rejects_an_unknown_triple() {
    let dir = unique_tmp_dir("compile_object_bad_triple");
    ensure_clean_dir(&dir);
    write_file(&dir, "x.lk", "return 1;\n");

    let output = run_cli(&dir, ["compile", "object:not-a-real-triple", "x.lk"])
        .output()
        .expect("run lk compile");
    assert!(!output.status.success(), "an unknown triple must not succeed");

    let _ = std::fs::remove_dir_all(&dir);
}

/// One expression's scratch registers are handed back as it goes.
///
/// A register VM needs *one* temporary for `a + b + c + …`, not one per term:
/// the result is written over the left operand, which is what `x += 1` has
/// always compiled to. Every intermediate kept its own register instead, so a
/// single expression could exhaust the 256 a frame has — and the failure was a
/// refusal to compile a program that is nothing unusual. 300 terms and 40 list
/// elements are both well past where it used to stop (~250 and 27).
///
/// The answers are checked, not just the exit status: reusing an operand's
/// register is only safe because the opcodes read both operands before writing
/// the destination, and a compiler that got that wrong would still compile.
#[test]
fn one_expression_reuses_its_scratch_registers() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("wide_expr.lk");
    let chain = (1..=300).map(|i| format!("({i} * 2)")).collect::<Vec<_>>().join(" + ");
    let elements = (0..40)
        .map(|i| format!("(\"abc\".count(\"a\") + {i})"))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        &path,
        format!(
            "let total = {chain};
let xs = [{elements}];
println(\"${{total}} ${{xs.len()}} ${{xs[39]}}\");
"
        ),
    )
    .expect("write");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
        .arg(&path)
        .output()
        .expect("run lk");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // 2 * (1 + … + 300) = 90300; the last element is 1 + 39.
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "90300 40 40");
}

/// The same, for the two windows an expression can be lowered into: a call's
/// arguments and a template string's parts.
///
/// Both pre-allocate a contiguous window and then lower into it, and both let
/// every part's scratch pile up behind the window. Two programs, because the
/// two halves fail differently and one program does not separate them:
///
/// - 60 interpolations of `${s.count(t) + i}` **refuse to compile** without the
///   template half.
/// - a 60-argument call nested inside a template still compiles without the
///   call half — it just costs 191 registers where 132 are needed, which is why
///   the count is asserted rather than the exit status.
#[test]
fn a_call_window_and_a_template_reuse_their_scratch_too() {
    let dir = tempfile::tempdir().expect("temp dir");
    let params = (0..60).map(|i| format!("a{i}: Int")).collect::<Vec<_>>().join(", ");
    let args = (0..60)
        .map(|i| format!("(\"aaa\".count(\"a\") + \"b\".len() + {i})"))
        .collect::<Vec<_>>()
        .join(", ");
    let template = (0..60)
        .map(|i| format!("${{\"a\".count(\"a\") + {i}}}"))
        .collect::<Vec<_>>()
        .join("-");

    // `a0` is 3 + 1 + 0 and `a59` is 3 + 1 + 59.
    let call = dir.path().join("wide_call.lk");
    std::fs::write(
        &call,
        format!("fn many({params}) -> Int {{ return a0 + a59; }}\nprintln(\"${{many({args})}}\");\n"),
    )
    .expect("write");
    let rendered = dir.path().join("wide_template.lk");
    std::fs::write(&rendered, format!("println(\"{template}\");\n")).expect("write");

    let expected = ["67", &(1..=60).map(|i| i.to_string()).collect::<Vec<_>>().join("-")];
    for (path, expected) in [(&call, expected[0]), (&rendered, expected[1])] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
            .arg(path)
            .output()
            .expect("run lk");
        assert!(
            output.status.success(),
            "{}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
    }

    // The count itself, not just "it compiled": a call window that stops
    // recycling is still under the ceiling at this width, so success alone
    // would not notice. Measured 132 with the reuse and 191 without.
    let counted = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
        .args(["coverage", "--disassemble"])
        .arg(&call)
        .output()
        .expect("run lk coverage");
    let listing = String::from_utf8_lossy(&counted.stdout);
    let registers: usize = listing
        .lines()
        .find_map(|line| line.trim().strip_prefix("registers: "))
        .and_then(|count| count.trim().parse().ok())
        .unwrap_or_else(|| panic!("no register count in the listing: {listing}"));
    assert!(
        registers < 160,
        "the call window stopped reusing its scratch: {registers} registers"
    );
}

/// A struct literal is not capped at a number nobody could reach.
///
/// The guard said "max 127 fields", but `NewObject` reads its fields from a
/// window of *two* registers each plus one for the type name, so 127 fields
/// need 255 window registers and `dst` has nowhere to go. In practice it broke
/// around 84, and what came out was "this function needs more than 256
/// registers" — a message about the enclosing function, for a limit belonging to
/// one literal. Two diagnostics, one real ceiling, neither of them naming it.
///
/// 200 is chosen to sit past every one of those numbers: past 84, past 127, and
/// past the 255-register window the old path needed.
#[test]
fn a_struct_literal_is_not_capped_at_an_unreachable_field_count() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("wide.lk");
    let fields = (0..200).map(|i| format!("f{i}: Int")).collect::<Vec<_>>().join(", ");
    let values = (0..200).map(|i| format!("f{i}: {i}")).collect::<Vec<_>>().join(", ");
    std::fs::write(
        &path,
        format!("struct Wide {{ {fields} }}\nlet w = Wide {{ {values} }};\nprintln(\"${{w.f199}} ${{w.f0}}\");\n"),
    )
    .expect("write");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
        .arg(&path)
        .output()
        .expect("run lk");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "199 0");
}

/// Too many fields is reported as too many fields.
///
/// A `struct` declaration emits no code of its own, but every one gets a
/// generated constructor taking one *named parameter* per field — and parameters
/// are locals. So a 254-field struct failed with "this function needs more than
/// 256 registers … split the body into smaller functions": a body the program
/// does not contain, and advice that cannot be followed, for a limit that is
/// real and worth stating plainly.
#[test]
fn a_struct_too_wide_to_construct_says_so() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("too_wide.lk");
    let fields = (0..254).map(|i| format!("f{i}: Int")).collect::<Vec<_>>().join(", ");
    std::fs::write(&path, format!("struct TooWide {{ {fields} }}\n")).expect("write");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lk"))
        .arg("check")
        .arg(&path)
        .output()
        .expect("run lk check");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(stderr.contains("struct `TooWide` has 254 fields"), "{stderr}");
    assert!(stderr.contains("253 is the most one can have"), "{stderr}");
    // The register message named the wrong thing entirely.
    assert!(!stderr.contains("split the body into smaller functions"), "{stderr}");
}

/// A package dependency bundles like a file import, in every spelling that
/// names one.
///
/// Before, the bundler queued file imports only, so a call into a dependency
/// fell to the stdlib-only module lowering and the whole program ran on the
/// Tier 0 VM bundle — about 3x slower, with nothing said. The sweep pins the
/// `use dep;` spelling through the workspace example; the other three have no
/// corpus program, and each is a separate arm of the binding table.
#[test]
fn a_package_dependency_lowers_natively_in_every_import_spelling() {
    let dir = unique_tmp_dir("pkg_bundle_spellings");
    ensure_clean_dir(&dir);
    write_file(
        &dir,
        "Lk.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[dependencies]\nmathlib = { path = \"mathlib\" }\n",
    );
    create_dir_all(dir.join("mathlib/src")).expect("create dep dir");
    write_file(
        &dir.join("mathlib"),
        "Lk.toml",
        "[package]\nname = \"mathlib\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
    );
    write_file(
        &dir.join("mathlib/src"),
        "mod.lk",
        "fn double(n: Int) -> Int {\n    return n * 2;\n}\n",
    );
    create_dir_all(dir.join("src")).expect("create src dir");

    for source in [
        "use mathlib;\nprintln(mathlib.double(7));\n",
        "use mathlib as ml;\nprintln(ml.double(7));\n",
        "use { double } from mathlib;\nprintln(double(7));\n",
        "use * as m from mathlib;\nprintln(m.double(7));\n",
    ] {
        write_file(&dir.join("src"), "main.lk", source);
        // Strict: no fallback, no hybrid bridge — "compiles" means "lowered".
        let compiled = run_cli(&dir, ["compile", "src/main.lk"])
            .env("LK_AOT_NO_FALLBACK", "1")
            .env("LK_AOT_HYBRID", "0")
            .output()
            .expect("spawn compile");
        assert!(
            compiled.status.success(),
            "{source} did not lower: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );

        let native = Command::new(dir.join("src/main"))
            .current_dir(&dir)
            .output()
            .expect("run the native binary");
        let vm = run_cli(&dir, ["src/main.lk"])
            .env("LK_FORCE_VM", "1")
            .output()
            .expect("run under the VM");
        assert_eq!(
            String::from_utf8_lossy(&native.stdout),
            String::from_utf8_lossy(&vm.stdout),
            "{source}: the two executors disagree"
        );
        assert_eq!(String::from_utf8_lossy(&native.stdout).trim(), "14", "{source}");
    }
}
