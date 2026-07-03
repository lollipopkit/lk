//! Integration coverage for the opt-in bytecode compile cache (plan M1.3):
//! `LK_CACHE=1` stores a compiled `.lkm` for an unchanged macro-free source and
//! reuses it on later runs, and is a no-op when unset.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn bin_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lk"))
}

fn unique_tmp_dir(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("lk_{}_{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("create tmp dir");
    path
}

fn run(source_file: &Path, cache_dir: &Path, with_cache: bool) -> String {
    let mut cmd = Command::new(bin_path());
    cmd.arg(source_file).env("LK_BYTECODE_CACHE_DIR", cache_dir);
    if with_cache {
        cmd.env("LK_CACHE", "1");
    } else {
        cmd.env_remove("LK_CACHE");
    }
    let output = cmd.output().expect("spawn lk");
    assert!(
        output.status.success(),
        "lk run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn cached_lkm_files(cache_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(cache_dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("lkm"))
        .collect()
}

#[test]
fn bytecode_cache_stores_then_reuses_compiled_module() {
    let dir = unique_tmp_dir("bc_cache_hit");
    let source = dir.join("prog.lk");
    fs::write(&source, "fn dbl(x) { return x * 2; }\nreturn dbl(21);\n").expect("write source");
    let cache_dir = dir.join("cache");

    // First run compiles and writes exactly one cache entry.
    assert_eq!(run(&source, &cache_dir, true), "42");
    let cached = cached_lkm_files(&cache_dir);
    assert_eq!(cached.len(), 1, "first cached run should write one .lkm: {cached:?}");
    let mtime_after_first = fs::metadata(&cached[0])
        .and_then(|m| m.modified())
        .expect("cache mtime");

    // Second run produces the same output and does NOT rewrite the cache file
    // — the hit path skips compilation/store, so the mtime is unchanged.
    assert_eq!(run(&source, &cache_dir, true), "42");
    let mtime_after_second = fs::metadata(&cached[0])
        .and_then(|m| m.modified())
        .expect("cache mtime");
    assert_eq!(
        mtime_after_first, mtime_after_second,
        "second run should hit the cache without rewriting it"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn bytecode_cache_is_opt_in() {
    let dir = unique_tmp_dir("bc_cache_optin");
    let source = dir.join("prog.lk");
    fs::write(&source, "return 7;\n").expect("write source");
    let cache_dir = dir.join("cache");

    // Without LK_CACHE the run works but writes no cache directory/entry.
    assert_eq!(run(&source, &cache_dir, false), "7");
    assert!(
        cached_lkm_files(&cache_dir).is_empty(),
        "no cache entries should be written when LK_CACHE is unset"
    );

    let _ = fs::remove_dir_all(&dir);
}
