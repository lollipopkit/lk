use std::{
    ffi::OsStr,
    fs::{self, File, create_dir_all},
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::Duration,
};

use sha2::{Digest, Sha256};

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

fn package_dir_checksum(package_dir: &Path) -> String {
    let mut files = Vec::new();
    collect_checksum_files(package_dir, package_dir, &mut files);
    files.sort();
    let mut hasher = Sha256::new();
    for relative in files {
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update([0]);
        let mut file = File::open(package_dir.join(&relative)).expect("open checksum file");
        let mut buffer = [0; 8192];
        loop {
            let read = file.read(&mut buffer).expect("read checksum file");
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        hasher.update([0]);
    }
    hex_lower(&hasher.finalize())
}

fn collect_checksum_files(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read checksum dir") {
        let entry = entry.expect("checksum dir entry");
        let path = entry.path();
        let file_name = entry.file_name();
        if file_name == ".git" || file_name == "Lk.lock" {
            continue;
        }
        let file_type = entry.file_type().expect("checksum file type");
        if file_type.is_dir() {
            collect_checksum_files(root, &path, files);
        } else if file_type.is_file() {
            files.push(path.strip_prefix(root).expect("relative checksum path").to_path_buf());
        }
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[test]
fn pkg_publish_dry_run_prints_registry_manifest() {
    let dir = unique_tmp_dir("pkg_publish_dry_run");
    ensure_clean_dir(&dir);

    write_file(
        &dir,
        "Lk.toml",
        r#"
[package]
name = "app"
version = "0.2.3"

[dependencies]
helper = { path = "deps/helper" }

[registry]
name = "local"
url = "https://registry.lk.example"
include = ["Lk.toml", "src/**"]

[macros]
trusted_dependencies = ["helper"]

[macros.derive.MakeAnswer]
command = "lk-derive-answer"

[macros.attribute.route]
command = "lk-route-macro"

[macros.function_like.sql]
command = "lk-sql-macro"
"#,
    );
    write_file(&dir, "src/mod.lk", "fn main() { return 1; }\n");
    write_file(
        &dir,
        "deps/helper/Lk.toml",
        r#"
[package]
name = "helper"
version = "0.1.0"

[macros.function_like.help]
command = "lk-help-macro"
"#,
    );
    write_file(&dir, "deps/helper/src/mod.lk", "fn helper() { return 1; }\n");

    let output = run_cli(&dir, ["pkg", "publish", "--dry-run"])
        .output()
        .expect("spawn pkg publish dry-run");
    assert!(
        output.status.success(),
        "pkg publish dry-run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest: serde_json::Value = serde_json::from_slice(&output.stdout).expect("publish manifest stdout is JSON");

    assert_eq!(manifest["package"], "app");
    assert_eq!(manifest["version"], "0.2.3");
    assert_eq!(manifest["registry"], "local");
    assert_eq!(manifest["registry_url"], "https://registry.lk.example");
    assert_eq!(manifest["include"][0], "Lk.toml");
    assert_eq!(manifest["include"][1], "src/**");
    assert_eq!(manifest["dependencies"][0]["name"], "helper");
    assert_eq!(manifest["dependencies"][0]["version_or_rev"], "0.1.0");
    assert_eq!(manifest["macro_providers"]["derive"][0], "MakeAnswer");
    assert_eq!(manifest["macro_providers"]["attribute"][0], "route");
    assert_eq!(manifest["macro_providers"]["function_like"][0], "sql");
    assert_eq!(manifest["macro_providers"]["trusted_dependencies"][0], "helper");
    assert_eq!(manifest["integrity"]["algorithm"], "sha256");
    assert_eq!(manifest["integrity"]["digest"].as_str().unwrap().len(), 64);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn pkg_publish_dry_run_rejects_unresolved_dependencies() {
    let dir = unique_tmp_dir("pkg_publish_missing_dependency");
    ensure_clean_dir(&dir);

    write_file(
        &dir,
        "Lk.toml",
        r#"
[package]
name = "app"
version = "0.2.3"

[dependencies]
remote = { github = "owner/remote", tag = "v1.2.0" }

[registry]
url = "https://registry.lk.example"
"#,
    );
    write_file(&dir, "src/mod.lk", "fn main() { return 1; }\n");

    let output = run_cli(&dir, ["pkg", "publish", "--dry-run"])
        .output()
        .expect("spawn pkg publish dry-run");
    assert!(!output.status.success(), "unresolved dependency should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("registry publish requires all dependencies to resolve; missing: remote"),
        "expected unresolved dependency error, got: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn pkg_publish_uploads_registry_manifest() {
    let dir = unique_tmp_dir("pkg_publish_upload");
    ensure_clean_dir(&dir);
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock registry");
    let registry_url = format!("http://{}", listener.local_addr().expect("mock registry addr"));
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept publish request");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("set read timeout");
        let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
        let mut request_line = String::new();
        reader.read_line(&mut request_line).expect("read request line");
        let mut authorization = String::new();
        let mut scope = String::new();
        let mut content_length = 0usize;
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).expect("read header");
            if header == "\r\n" {
                break;
            }
            if let Some((name, value)) = header.trim_end().split_once(':') {
                match name.to_ascii_lowercase().as_str() {
                    "authorization" => authorization = value.trim().to_string(),
                    "x-lk-registry-scope" => scope = value.trim().to_string(),
                    "content-length" => content_length = value.trim().parse().expect("content length"),
                    _ => {}
                }
            }
        }
        let mut body = vec![0; content_length];
        reader.read_exact(&mut body).expect("read request body");
        stream
            .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\nok")
            .expect("write mock response");
        (
            request_line,
            authorization,
            scope,
            String::from_utf8(body).expect("request body utf8"),
        )
    });

    write_file(
        &dir,
        "Lk.toml",
        &format!(
            r#"
[package]
name = "app"
version = "0.2.3"

[registry]
name = "local"
url = "{registry_url}"

[macros.function_like.sql]
command = "lk-sql-macro"
"#
        ),
    );
    write_file(&dir, "src/mod.lk", "fn main() {{ return 1; }}\n");

    let output = run_cli(&dir, ["pkg", "publish"])
        .env("LK_REGISTRY_TOKEN", "secret-token")
        .output()
        .expect("spawn pkg publish");
    assert!(
        output.status.success(),
        "pkg publish failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Published app 0.2.3 to local"), "{stderr}");
    let (request_line, authorization, scope, body) = server.join().expect("mock registry thread");
    assert_eq!(request_line, "POST /api/v1/packages HTTP/1.1\r\n");
    assert_eq!(authorization, "Bearer secret-token");
    assert_eq!(scope, "publish");
    let body: serde_json::Value = serde_json::from_str(&body).expect("request body json");
    assert_eq!(body["package"], "app");
    assert_eq!(body["version"], "0.2.3");
    assert_eq!(body["macro_providers"]["function_like"][0], "sql");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn pkg_yank_and_unyank_send_registry_requests() {
    let dir = unique_tmp_dir("pkg_yank_upload");
    ensure_clean_dir(&dir);
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock registry");
    let registry_url = format!("http://{}", listener.local_addr().expect("mock registry addr"));
    let server = thread::spawn(move || {
        let mut requests = Vec::new();
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().expect("accept yank request");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("set read timeout");
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut request_line = String::new();
            reader.read_line(&mut request_line).expect("read request line");
            let mut authorization = String::new();
            let mut scope = String::new();
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).expect("read header");
                if header == "\r\n" {
                    break;
                }
                if let Some((name, value)) = header.trim_end().split_once(':') {
                    match name.to_ascii_lowercase().as_str() {
                        "authorization" => authorization = value.trim().to_string(),
                        "x-lk-registry-scope" => scope = value.trim().to_string(),
                        _ => {}
                    }
                }
            }
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .expect("write mock response");
            requests.push((request_line, authorization, scope));
        }
        requests
    });

    write_file(
        &dir,
        "Lk.toml",
        &format!(
            r#"
[package]
name = "app"
version = "0.2.3"

[registry]
url = "{registry_url}"
"#
        ),
    );
    write_file(&dir, "src/mod.lk", "fn main() {{ return 1; }}\n");

    let yank = run_cli(&dir, ["pkg", "yank", "helper", "0.2.3"])
        .env("LK_REGISTRY_TOKEN", "fallback-token")
        .env("LK_REGISTRY_YANK_TOKEN", "yank-token")
        .output()
        .expect("spawn pkg yank");
    assert!(
        yank.status.success(),
        "pkg yank failed: {}",
        String::from_utf8_lossy(&yank.stderr)
    );
    assert!(
        String::from_utf8_lossy(&yank.stderr).contains("Yanked helper 0.2.3"),
        "{}",
        String::from_utf8_lossy(&yank.stderr)
    );

    let unyank = run_cli(&dir, ["pkg", "yank", "helper", "0.2.3", "--undo"])
        .env("LK_REGISTRY_TOKEN", "fallback-token")
        .env("LK_REGISTRY_YANK_TOKEN", "yank-token")
        .output()
        .expect("spawn pkg unyank");
    assert!(
        unyank.status.success(),
        "pkg unyank failed: {}",
        String::from_utf8_lossy(&unyank.stderr)
    );
    assert!(
        String::from_utf8_lossy(&unyank.stderr).contains("Un-yanked helper 0.2.3"),
        "{}",
        String::from_utf8_lossy(&unyank.stderr)
    );

    let requests = server.join().expect("mock registry thread");
    assert_eq!(requests[0].0, "POST /api/v1/packages/helper/0.2.3/yank HTTP/1.1\r\n");
    assert_eq!(requests[1].0, "DELETE /api/v1/packages/helper/0.2.3/yank HTTP/1.1\r\n");
    for (_, authorization, scope) in requests {
        assert_eq!(authorization, "Bearer yank-token");
        assert_eq!(scope, "yank");
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn pkg_index_sync_downloads_and_caches_registry_snapshot() {
    let dir = unique_tmp_dir("pkg_index_sync");
    ensure_clean_dir(&dir);
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock registry");
    let registry_url = format!("http://{}", listener.local_addr().expect("mock registry addr"));
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept index request");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("set read timeout");
        let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
        let mut request_line = String::new();
        reader.read_line(&mut request_line).expect("read request line");
        let mut scope = String::new();
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).expect("read header");
            if header == "\r\n" {
                break;
            }
            if let Some((name, value)) = header.trim_end().split_once(':')
                && name.eq_ignore_ascii_case("x-lk-registry-scope")
            {
                scope = value.trim().to_string();
            }
        }
        let body = serde_json::json!({
            "packages": [
                {
                    "name": "helper",
                    "versions": [
                        {
                            "version": "0.2.0",
                            "source": "https://example.invalid/helper.git",
                            "rev": "abc123",
                            "checksum": "sha256:0123",
                            "yanked": false
                        }
                    ],
                    "macro_providers": {
                        "function_like": ["sql"],
                        "derive": ["MakeAnswer"]
                    }
                }
            ]
        })
        .to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("write mock response");
        (request_line, scope)
    });

    write_file(
        &dir,
        "Lk.toml",
        &format!(
            r#"
[package]
name = "app"
version = "0.1.0"

[registry]
name = "local"
url = "{registry_url}"
"#
        ),
    );
    write_file(&dir, "src/mod.lk", "fn main() {{ return 1; }}\n");

    let lk_home = dir.join("lk-home");
    let output = run_cli(&dir, ["pkg", "index", "sync"])
        .env("LK_HOME", &lk_home)
        .output()
        .expect("spawn pkg index sync");
    assert!(
        output.status.success(),
        "pkg index sync failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (request_line, scope) = server.join().expect("mock registry thread");
    assert_eq!(request_line, "GET /api/v1/index HTTP/1.1\r\n");
    assert_eq!(scope, "index");

    let cache = fs::read_to_string(lk_home.join("registry/local/index.json")).expect("read cached registry index");
    let cache: serde_json::Value = serde_json::from_str(&cache).expect("cache JSON");
    assert_eq!(cache["registry"], "local");
    assert_eq!(cache["registry_url"], registry_url);
    assert_eq!(cache["packages"][0]["name"], "helper");
    assert_eq!(cache["packages"][0]["versions"][0]["version"], "0.2.0");
    assert_eq!(cache["packages"][0]["versions"][0]["checksum"], "sha256:0123");
    assert_eq!(cache["packages"][0]["macro_providers"]["function_like"][0], "sql");
    assert_eq!(cache["packages"][0]["macro_providers"]["derive"][0], "MakeAnswer");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn pkg_fetch_resolves_registry_version_to_locked_git_revision() {
    let dir = unique_tmp_dir("pkg_fetch_registry_version");
    ensure_clean_dir(&dir);
    let repo = dir.join("registry-helper.git-src");
    create_dir_all(repo.join("src")).expect("create registry repo");
    write_file(&repo, "src/mod.lk", "fn helper() { return 42; }\n");
    write_file(
        &repo,
        "Lk.toml",
        r#"
[package]
name = "helper"
version = "0.1.0"
"#,
    );
    run_git(&repo, ["init"]);
    run_git(&repo, ["config", "user.email", "lk@example.test"]);
    run_git(&repo, ["config", "user.name", "LK Test"]);
    run_git(&repo, ["add", "."]);
    run_git(&repo, ["commit", "-m", "init helper"]);
    let rev = git_stdout(&repo, ["rev-parse", "HEAD"]);
    let checksum = format!("sha256:{}", package_dir_checksum(&repo));

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock registry");
    let registry_url = format!("http://{}", listener.local_addr().expect("mock registry addr"));
    let source = repo.display().to_string();
    let resolved_rev = rev.clone();
    let resolved_checksum = checksum.clone();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept registry resolution request");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("set read timeout");
        let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
        let mut request_line = String::new();
        reader.read_line(&mut request_line).expect("read request line");
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).expect("read header");
            if header == "\r\n" {
                break;
            }
        }
        let body = serde_json::json!({
            "source": source,
            "rev": resolved_rev,
            "checksum": resolved_checksum,
        })
        .to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("write mock response");
        request_line
    });

    let app = dir.join("app");
    ensure_clean_dir(&app);
    write_file(
        &app,
        "Lk.toml",
        &format!(
            r#"
[package]
name = "app"
version = "0.1.0"

[registry]
url = "{registry_url}"

[dependencies]
helper = {{ version = "0.1.0" }}
"#
        ),
    );
    write_file(&app, "src/mod.lk", "use helper;\nreturn helper.helper();\n");

    let lk_home = dir.join("lk-home");
    let output = run_cli(&app, ["pkg", "fetch"])
        .env("LK_HOME", &lk_home)
        .output()
        .expect("spawn pkg fetch");
    assert!(
        output.status.success(),
        "pkg fetch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request_line = server.join().expect("mock registry thread");
    assert_eq!(request_line, "GET /api/v1/packages/helper/0.1.0 HTTP/1.1\r\n");
    let lock = fs::read_to_string(app.join("Lk.lock")).expect("read Lk.lock");
    assert!(lock.contains("name = \"helper\""), "{lock}");
    assert!(lock.contains(&format!("source = \"{}\"", repo.display())), "{lock}");
    assert!(lock.contains(&format!("rev = \"{rev}\"")), "{lock}");
    assert!(lock.contains(&format!("checksum = \"{checksum}\"")), "{lock}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn pkg_fetch_offline_resolves_registry_range_from_cached_index() {
    let dir = unique_tmp_dir("pkg_fetch_offline_registry_index");
    ensure_clean_dir(&dir);
    let repo = dir.join("offline-helper.git-src");
    create_dir_all(repo.join("src")).expect("create registry repo");
    write_file(&repo, "src/mod.lk", "fn helper() { return 10; }\n");
    write_file(
        &repo,
        "Lk.toml",
        r#"
[package]
name = "helper"
version = "0.1.0"
"#,
    );
    run_git(&repo, ["init"]);
    run_git(&repo, ["config", "user.email", "lk@example.test"]);
    run_git(&repo, ["config", "user.name", "LK Test"]);
    run_git(&repo, ["add", "."]);
    run_git(&repo, ["commit", "-m", "init helper"]);
    let old_rev = git_stdout(&repo, ["rev-parse", "HEAD"]);
    let old_checksum = format!("sha256:{}", package_dir_checksum(&repo));
    write_file(&repo, "src/mod.lk", "fn helper() { return 20; }\n");
    run_git(&repo, ["add", "."]);
    run_git(&repo, ["commit", "-m", "update helper"]);
    let selected_rev = git_stdout(&repo, ["rev-parse", "HEAD"]);
    let selected_checksum = format!("sha256:{}", package_dir_checksum(&repo));

    let app = dir.join("app");
    ensure_clean_dir(&app);
    let registry_url = "http://127.0.0.1:9";
    write_file(
        &app,
        "Lk.toml",
        &format!(
            r#"
[package]
name = "app"
version = "0.1.0"

[registry]
name = "local"
url = "{registry_url}"

[dependencies]
helper = {{ version = ">=0.2.0, <0.4.0" }}
"#
        ),
    );
    write_file(&app, "src/mod.lk", "use helper;\nreturn helper.helper();\n");

    let lk_home = dir.join("lk-home");
    create_dir_all(lk_home.join("registry/local")).expect("create registry cache dir");
    let index = serde_json::json!({
        "registry": "local",
        "registry_url": registry_url,
        "packages": [
            {
                "name": "helper",
                "versions": [
                    {
                        "version": "0.1.5",
                        "source": repo.display().to_string(),
                        "rev": old_rev.clone(),
                        "checksum": old_checksum.clone()
                    },
                    {
                        "version": "0.2.0",
                        "source": repo.display().to_string(),
                        "rev": selected_rev.clone(),
                        "checksum": selected_checksum.clone()
                    },
                    {
                        "version": "0.3.2",
                        "source": repo.display().to_string(),
                        "rev": old_rev.clone(),
                        "checksum": old_checksum.clone(),
                        "yanked": true
                    }
                ],
                "macro_providers": {
                    "function_like": ["sql"]
                }
            }
        ]
    });
    write_file(
        &lk_home,
        "registry/local/index.json",
        &serde_json::to_string_pretty(&index).expect("index JSON"),
    );

    let output = run_cli(&app, ["pkg", "fetch", "--offline"])
        .env("LK_HOME", &lk_home)
        .output()
        .expect("spawn offline pkg fetch");
    assert!(
        output.status.success(),
        "offline pkg fetch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lock = fs::read_to_string(app.join("Lk.lock")).expect("read Lk.lock");
    assert!(lock.contains("name = \"helper\""), "{lock}");
    assert!(lock.contains(&format!("source = \"{}\"", repo.display())), "{lock}");
    assert!(lock.contains(&format!("rev = \"{selected_rev}\"")), "{lock}");
    assert!(lock.contains(&format!("checksum = \"{selected_checksum}\"")), "{lock}");
    assert!(!lock.contains(&format!("rev = \"{old_rev}\"")), "{lock}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn pkg_fetch_resolves_registry_semver_range_to_highest_compatible_revision() {
    let dir = unique_tmp_dir("pkg_fetch_registry_range");
    ensure_clean_dir(&dir);
    let repo = dir.join("registry-helper-range.git-src");
    create_dir_all(repo.join("src")).expect("create registry repo");
    write_file(&repo, "src/mod.lk", "fn helper() { return 42; }\n");
    write_file(
        &repo,
        "Lk.toml",
        r#"
[package]
name = "helper"
version = "0.1.0"
"#,
    );
    run_git(&repo, ["init"]);
    run_git(&repo, ["config", "user.email", "lk@example.test"]);
    run_git(&repo, ["config", "user.name", "LK Test"]);
    run_git(&repo, ["add", "."]);
    run_git(&repo, ["commit", "-m", "init helper"]);
    let old_rev = git_stdout(&repo, ["rev-parse", "HEAD"]);
    let old_checksum = format!("sha256:{}", package_dir_checksum(&repo));
    write_file(&repo, "src/mod.lk", "fn helper() { return 99; }\n");
    run_git(&repo, ["add", "."]);
    run_git(&repo, ["commit", "-m", "update helper"]);
    let selected_rev = git_stdout(&repo, ["rev-parse", "HEAD"]);
    let selected_checksum = format!("sha256:{}", package_dir_checksum(&repo));

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock registry");
    let registry_url = format!("http://{}", listener.local_addr().expect("mock registry addr"));
    let source = repo.display().to_string();
    let selected = selected_rev.clone();
    let selected_package_checksum = selected_checksum.clone();
    let old = old_rev.clone();
    let old_package_checksum = old_checksum.clone();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept registry versions request");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("set read timeout");
        let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
        let mut request_line = String::new();
        reader.read_line(&mut request_line).expect("read request line");
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).expect("read header");
            if header == "\r\n" {
                break;
            }
        }
        let body = serde_json::json!({
            "versions": [
                {
                    "version": "0.1.5",
                    "source": source.clone(),
                    "rev": old.clone(),
                    "checksum": old_package_checksum.clone()
                },
                {
                    "version": "0.2.0",
                    "source": source.clone(),
                    "rev": selected,
                    "checksum": selected_package_checksum
                },
                {
                    "version": "0.3.2",
                    "source": source.clone(),
                    "rev": old.clone(),
                    "checksum": old_package_checksum.clone(),
                    "yanked": true
                },
                {
                    "version": "0.4.0",
                    "source": source,
                    "rev": old,
                    "checksum": old_package_checksum
                }
            ]
        })
        .to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("write mock response");
        request_line
    });

    let app = dir.join("app");
    ensure_clean_dir(&app);
    write_file(
        &app,
        "Lk.toml",
        &format!(
            r#"
[package]
name = "app"
version = "0.1.0"

[registry]
url = "{registry_url}"

[dependencies]
helper = {{ version = ">=0.2.0, <0.4.0" }}
"#
        ),
    );
    write_file(&app, "src/mod.lk", "use helper;\nreturn helper.helper();\n");

    let lk_home = dir.join("lk-home");
    let output = run_cli(&app, ["pkg", "fetch"])
        .env("LK_HOME", &lk_home)
        .output()
        .expect("spawn pkg fetch");
    assert!(
        output.status.success(),
        "pkg fetch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request_line = server.join().expect("mock registry thread");
    assert_eq!(request_line, "GET /api/v1/packages/helper HTTP/1.1\r\n");
    let lock = fs::read_to_string(app.join("Lk.lock")).expect("read Lk.lock");
    assert!(lock.contains("name = \"helper\""), "{lock}");
    assert!(lock.contains(&format!("source = \"{}\"", repo.display())), "{lock}");
    assert!(lock.contains(&format!("rev = \"{selected_rev}\"")), "{lock}");
    assert!(lock.contains(&format!("checksum = \"{selected_checksum}\"")), "{lock}");
    assert!(!lock.contains(&format!("rev = \"{old_rev}\"")), "{lock}");

    let _ = fs::remove_dir_all(&dir);
}
