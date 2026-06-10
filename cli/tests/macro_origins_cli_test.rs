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
    let mut file = File::create(&path).expect("create file");
    file.write_all(contents.as_bytes()).expect("write file");
}

fn ensure_clean_dir(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
    create_dir_all(dir).expect("create tmp dir");
}

#[test]
fn macro_expand_origins_prints_nested_origin_json() {
    let dir = unique_tmp_dir("macro_origins");
    ensure_clean_dir(&dir);

    write_file(
        &dir,
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

    let output = run_cli(&dir, ["macro", "expand", "main.lk", "--origins"])
        .output()
        .expect("spawn macro expand");
    assert!(
        output.status.success(),
        "macro expand failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let json = stdout
        .split_once("# macro token origins\n")
        .map(|(_, json)| json)
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

    let _ = fs::remove_dir_all(&dir);
}
