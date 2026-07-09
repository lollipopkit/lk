use std::{
    ffi::OsStr,
    fs::{self, File, create_dir_all},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
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

/// Run a CLI subprocess with a timeout so tests fail fast instead of hanging.
fn run_cli_with_timeout<I, S>(dir: &Path, args: I) -> std::process::Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut child = run_cli(dir, args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn CLI process");
    let timeout = Duration::from_secs(60);
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child.wait_with_output().expect("wait for CLI output");
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("CLI process timed out after {timeout:?}");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("failed to wait for CLI process: {e}"),
        }
    }
}

fn write_file(dir: &Path, name: &str, contents: &str) {
    let path = dir.join(name);
    let mut file = File::create(&path).expect("create file");
    file.write_all(contents.as_bytes()).expect("write file");
}

/// RAII guard that cleans up a temporary directory on drop, even on panic.
struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(path: PathBuf) -> Self {
        ensure_clean_dir(&path);
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn ensure_clean_dir(dir: &Path) {
    if let Err(err) = fs::remove_dir_all(dir)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("warning: failed to remove temp dir {}: {err}", dir.display());
    }
    create_dir_all(dir).expect("create tmp dir");
}

#[test]
fn macro_expand_origins_prints_nested_origin_json() {
    let dir = TempDirGuard::new(unique_tmp_dir("macro_origins"));

    write_file(
        dir.path(),
        "main.lk",
        r#"
macro_rules! inner {
    () => { return 42; };
}
macro_rules! outer {
    () => { inner!() };
}
outer!();
"#,
    );

    let output = run_cli_with_timeout(dir.path(), ["macro", "expand", "main.lk", "--origins"]);
    assert!(
        output.status.success(),
        "macro expand failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let json = stdout
        .split_once("# macro token origins\n")
        .map(|(_, json)| json)
        .and_then(|json| json.split_once("# ast macro origins\n").map(|(json, _)| json))
        .expect("origins marker should be present");
    let origins: serde_json::Value = serde_json::from_str(json).expect("valid origins json");
    let answer = origins
        .as_array()
        .expect("origins should be an array")
        .iter()
        .find(|origin| origin["lexeme"] == "42")
        .expect("generated integer origin should be present");
    let frames = answer["frames"].as_array().expect("origin frames should be an array");

    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["macro_name"], "outer");
    assert_eq!(frames[0]["kind"], "definition");
    assert_eq!(frames[1]["macro_name"], "inner");
    assert_eq!(frames[1]["kind"], "definition");
}

#[test]
fn macro_expand_origins_prints_ast_macro_origin_json() {
    let dir = TempDirGuard::new(unique_tmp_dir("ast_macro_origins"));

    write_file(
        dir.path(),
        "main.lk",
        r#"
#[derive(Debug)]
struct User { id: Int }
"#,
    );

    let output = run_cli_with_timeout(dir.path(), ["macro", "expand", "main.lk", "--origins"]);
    assert!(
        output.status.success(),
        "macro expand failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let json = stdout
        .split_once("# ast macro origins\n")
        .map(|(_, json)| json)
        .expect("AST origins marker should be present");
    let origins: serde_json::Value = serde_json::from_str(json).expect("valid AST origins json");
    let derive_origin = origins
        .as_array()
        .expect("AST origins should be an array")
        .iter()
        .find(|origin| origin["macro_name"] == "Debug")
        .expect("Debug derive origin should be present");

    assert_eq!(derive_origin["kind"], "builtin_derive");
    assert_eq!(derive_origin["generated_items"], 2);
    assert!(
        derive_origin["generated_item_labels"]
            .as_array()
            .expect("labels should be an array")
            .iter()
            .any(|label| label == "impl __LKShow for User")
    );
    let generated_item_origins = derive_origin["generated_item_origins"]
        .as_array()
        .expect("generated item origins should be an array");
    let generated_impl = generated_item_origins
        .iter()
        .find(|item| item["label"] == "impl __LKShow for User")
        .expect("generated impl origin");
    assert!(
        generated_impl["span"].is_object(),
        "generated item origin should carry a source-map span"
    );
    let generated_members = generated_impl["generated_member_origins"]
        .as_array()
        .expect("generated member origins should be an array");
    assert!(
        generated_members
            .iter()
            .any(|member| member["label"] == "fn show" && member["span"].is_object()),
        "generated show method origin should carry a source-map span"
    );
    assert!(
        generated_members
            .iter()
            .any(|member| member["label"] == "expr self.id" && member["span"].is_object()),
        "generated field expression origin should carry a source-map span"
    );
}
