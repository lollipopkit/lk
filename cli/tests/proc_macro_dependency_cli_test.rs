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

fn test_shell() -> Option<PathBuf> {
    let shell = PathBuf::from("/bin/sh");
    shell.exists().then_some(shell)
}
