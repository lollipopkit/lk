//! Integration coverage for the decentralized git+lockfile dependency flow
//! (the M5.4 replacement for the removed centralized registry): a git
//! dependency is cloned and pinned to its resolved revision in `Lk.lock`.

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

fn run_git<I, S>(dir: &Path, args: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let status = Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .expect("spawn git");
    assert!(status.success(), "git command failed with {status}");
}

fn git_stdout<I, S>(dir: &Path, args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("spawn git");
    assert!(
        output.status.success(),
        "git command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
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
fn pkg_fetch_locks_git_dependency_to_head_revision() {
    let base = unique_tmp_dir("pkg_git_fetch");
    ensure_clean_dir(&base);

    // A standalone git repository acts as the dependency source (Deno/Go-style
    // decentralized dependency — no registry involved).
    let dep = base.join("dep");
    create_dir_all(&dep).expect("create dep dir");
    run_git(&dep, ["init", "-q", "-b", "main"]);
    run_git(&dep, ["config", "user.email", "test@example.com"]);
    run_git(&dep, ["config", "user.name", "LK Test"]);
    write_file(&dep, "Lk.toml", "[package]\nname = \"dep\"\nversion = \"0.1.0\"\n");
    write_file(&dep, "src/mod.lk", "fn helper() { return 1; }\n");
    run_git(&dep, ["add", "."]);
    run_git(&dep, ["commit", "-q", "-m", "init dep"]);
    let dep_rev = git_stdout(&dep, ["rev-parse", "HEAD"]);

    // The consumer package depends on it by git URL (a local path is a valid
    // git clone source).
    let app = base.join("app");
    create_dir_all(&app).expect("create app dir");
    let dep_url = dep.to_string_lossy().replace('\\', "/");
    write_file(
        &app,
        "Lk.toml",
        &format!("[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\ndep = {{ git = \"{dep_url}\" }}\n"),
    );
    write_file(&app, "src/main.lk", "println(\"hi\");\n");

    // `lk pkg fetch` clones the dependency and records its resolved revision.
    let output = run_cli(&app, ["pkg", "fetch"])
        .env("LK_HOME", base.join("home"))
        .output()
        .expect("spawn pkg fetch");
    assert!(
        output.status.success(),
        "pkg fetch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let lock = fs::read_to_string(app.join("Lk.lock")).expect("read Lk.lock");
    assert!(lock.contains("name = \"dep\""), "Lk.lock should record the dep: {lock}");
    assert!(
        lock.contains(&dep_rev),
        "Lk.lock should pin the dep to its HEAD revision {dep_rev}: {lock}"
    );

    let _ = fs::remove_dir_all(&base);
}
