use std::{
    ffi::OsStr,
    fs::{self, File, create_dir_all},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

fn bin_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lk"))
}

fn unique_tmp_dir(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("lk_{}_{}", name, std::process::id()));
    path
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
    if let Some(parent) = path.parent() {
        create_dir_all(parent).expect("create parent dir");
    }
    let mut file = File::create(&path).expect("create file");
    file.write_all(contents.as_bytes()).expect("write file");
}

fn ensure_clean_dir(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
    create_dir_all(dir).expect("create tmp dir");
}

#[test]
fn trusted_dependency_proc_macro_provider_expands_namespaced_function_like_macro() {
    let Some(shell) = test_shell() else {
        return;
    };
    let dir = unique_tmp_dir("trusted_dependency_proc_macro");
    ensure_clean_dir(&dir);

    write_file(
        &dir,
        "Lk.toml",
        r#"
[package]
name = "app"

[dependencies]
helper = { path = "deps/helper" }

[macros]
trusted_dependencies = ["helper"]
"#,
    );
    write_file(
        &dir,
        "main.lk",
        r#"
return helper::answer!();
"#,
    );
    write_file(
        &dir,
        "deps/helper/Lk.toml",
        &format!(
            r#"
[package]
name = "helper"

[macros.function_like.answer]
command = "{}"
args = ["-c", "cat >/dev/null; printf '%s' '{{\"protocol_version\":1,\"output_tokens\":[{{\"kind\":\"Int\",\"lexeme\":\"42\",\"span\":null}}],\"diagnostics\":[],\"dependencies\":[]}}'"]
"#,
            shell.display()
        ),
    );
    write_file(&dir, "deps/helper/src/mod.lk", "fn value() { return 1; }\n");

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
        "expected trusted provider output: {stdout}"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// An **untrusted** dependency's provider is never spawned.
///
/// `[macros] trusted_dependencies` is the security boundary of the whole macro
/// system: a provider is an external process, run during `lk check` / `lk macro
/// expand`, before any of the program executes. Only the positive half was
/// tested — a listed dependency expands — so a refactor that dropped the
/// `trusted.contains(&module.name)` guard would have left every test green
/// while a dependency gained arbitrary compile-time execution.
///
/// The assertion is the *spawn*, not the output. Discarding a provider's answer
/// after running it would satisfy "the macro did not expand" and still have run
/// the command, so the provider writes a sentinel file and this checks that the
/// file is absent.
#[test]
fn an_untrusted_dependency_provider_is_never_spawned() {
    let Some(shell) = test_shell() else {
        return;
    };
    let dir = unique_tmp_dir("untrusted_dependency_proc_macro");
    ensure_clean_dir(&dir);
    let sentinel = dir.join("provider_ran");

    // Identical to the trusted case below it, minus the `[macros]` table.
    write_file(
        &dir,
        "Lk.toml",
        r#"
[package]
name = "app"

[dependencies]
helper = { path = "deps/helper" }
"#,
    );
    write_file(&dir, "main.lk", "\nreturn helper::answer!();\n");
    write_file(
        &dir,
        "deps/helper/Lk.toml",
        &format!(
            r#"
[package]
name = "helper"

[macros.function_like.answer]
command = "{}"
args = ["-c", "touch '{}'; cat >/dev/null; printf '%s' '{{\"protocol_version\":1,\"output_tokens\":[{{\"kind\":\"Int\",\"lexeme\":\"42\",\"span\":null}}],\"diagnostics\":[],\"dependencies\":[]}}'"]
"#,
            shell.display(),
            sentinel.display()
        ),
    );
    write_file(&dir, "deps/helper/src/mod.lk", "fn value() { return 1; }\n");

    let output = run_cli(&dir, ["macro", "expand", "main.lk"])
        .output()
        .expect("spawn macro expand");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

    assert!(
        !sentinel.exists(),
        "an untrusted dependency's provider was executed — the trust list is the only thing \
         between a dependency and arbitrary compile-time execution"
    );
    assert!(
        !stdout.contains("return 42;"),
        "an untrusted provider's output reached the program: {stdout}"
    );

    // And the same package *with* the dependency trusted runs it — otherwise
    // this test would pass on a build where providers never run at all.
    write_file(
        &dir,
        "Lk.toml",
        r#"
[package]
name = "app"

[dependencies]
helper = { path = "deps/helper" }

[macros]
trusted_dependencies = ["helper"]
"#,
    );
    let output = run_cli(&dir, ["macro", "expand", "main.lk"])
        .output()
        .expect("spawn macro expand");
    assert!(
        output.status.success(),
        "macro expand failed once trusted: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(sentinel.exists(), "the trusted run must actually spawn the provider");

    let _ = fs::remove_dir_all(&dir);
}

fn test_shell() -> Option<PathBuf> {
    let shell = PathBuf::from("/bin/sh");
    shell.exists().then_some(shell)
}
