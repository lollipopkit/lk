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

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn macro_expand_origins_prints_ast_macro_origin_json() {
    let dir = unique_tmp_dir("ast_macro_origins");
    ensure_clean_dir(&dir);

    write_file(
        &dir,
        "main.lk",
        r#"
#[derive(Debug)]
struct User { id: Int }
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

    let _ = fs::remove_dir_all(&dir);
}
