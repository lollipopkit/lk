use std::fs;
use std::path::PathBuf;

use super::{collect_dir, collect_files, run_fmt};

const UNFORMATTED: &str = "fn main() {\nlet x = 1;\n}\n";
const FORMATTED: &str = "fn main() {\n    let x = 1;\n}\n";

fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("target")).unwrap();
    fs::create_dir_all(root.join(".hidden")).unwrap();
    fs::write(root.join("main.lk"), UNFORMATTED).unwrap();
    fs::write(root.join("src/lib.lk"), UNFORMATTED).unwrap();
    fs::write(root.join("src/notes.md"), "# not lk\n").unwrap();
    fs::write(root.join("target/built.lk"), UNFORMATTED).unwrap();
    fs::write(root.join(".hidden/skipped.lk"), UNFORMATTED).unwrap();
    dir
}

fn collected(root: &std::path::Path) -> Vec<String> {
    let mut files = Vec::new();
    collect_dir(root, &mut files).expect("walk");
    files.sort();
    files
        .iter()
        .map(|p| p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/"))
        .collect()
}

#[test]
fn walks_lk_files_and_skips_build_and_hidden_dirs() {
    let dir = tree();
    assert_eq!(collected(dir.path()), vec!["main.lk", "src/lib.lk"]);
}

#[test]
fn formats_every_file_in_a_directory() {
    let dir = tree();
    run_fmt(&[dir.path().to_path_buf()], false).expect("fmt");
    assert_eq!(fs::read_to_string(dir.path().join("main.lk")).unwrap(), FORMATTED);
    assert_eq!(fs::read_to_string(dir.path().join("src/lib.lk")).unwrap(), FORMATTED);
    // Excluded directories are left untouched.
    assert_eq!(
        fs::read_to_string(dir.path().join("target/built.lk")).unwrap(),
        UNFORMATTED
    );
}

#[test]
fn check_mode_reports_without_writing() {
    let dir = tree();
    let err = run_fmt(&[dir.path().to_path_buf()], true).expect_err("check should fail");
    assert!(err.to_string().contains("not formatted"), "{err}");
    assert_eq!(fs::read_to_string(dir.path().join("main.lk")).unwrap(), UNFORMATTED);

    run_fmt(&[dir.path().to_path_buf()], false).expect("fmt");
    run_fmt(&[dir.path().to_path_buf()], true).expect("check should pass after fmt");
}

#[test]
fn unparsable_file_is_left_untouched_and_fails_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let broken = dir.path().join("broken.lk");
    fs::write(&broken, "let s = \"unterminated;\n").unwrap();
    fs::write(dir.path().join("ok.lk"), UNFORMATTED).unwrap();

    let err = run_fmt(&[dir.path().to_path_buf()], false).expect_err("run should fail");
    assert!(err.to_string().contains("could not be formatted"), "{err}");
    assert_eq!(fs::read_to_string(&broken).unwrap(), "let s = \"unterminated;\n");
    // The healthy file in the same run is still formatted.
    assert_eq!(fs::read_to_string(dir.path().join("ok.lk")).unwrap(), FORMATTED);
}

#[test]
fn explicit_file_argument_ignores_the_extension_filter() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("script.lk.txt");
    fs::write(&path, UNFORMATTED).unwrap();
    run_fmt(&[path.clone()], false).expect("fmt");
    assert_eq!(fs::read_to_string(&path).unwrap(), FORMATTED);
}

#[test]
fn missing_path_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let missing: PathBuf = dir.path().join("nope.lk");
    let err = collect_files(&[missing]).expect_err("missing path should fail");
    assert!(err.to_string().contains("path not found"), "{err}");
}
