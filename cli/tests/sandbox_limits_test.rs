//! Integration coverage for the opt-in sandbox memory limits (plan M2.6):
//! `LK_MAX_HEAP_OBJECTS` caps live heap *objects* and `LK_MAX_HEAP_BYTES` caps
//! live *bytes* (default 70% of RAM). Exceeding either aborts the run — a hard
//! sandbox stop, like fuel — instead of growing unbounded. `0`/unset means
//! unlimited.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn bin_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lk"))
}

fn write_source(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("lk_sandbox_{}_{}.lk", name, std::process::id()));
    fs::write(&path, source).expect("write source");
    path
}

fn run_with(source: &Path, key: &str, value: &str) -> std::process::Output {
    Command::new(bin_path())
        .arg(source)
        .env(key, value)
        .output()
        .expect("spawn lk")
}

// Accumulates ~5000 *live* inner lists in `xs`, so the live-object count grows
// past a tight cap (GC cannot reclaim them — they stay reachable).
const ALLOC_HEAVY: &str = "let xs = [];\nfor i in 0..5000 { xs = xs + [[i, i]]; }\nreturn xs.len();\n";

#[test]
fn heap_object_limit_aborts_runaway_allocation() {
    let source = write_source("heap", ALLOC_HEAVY);

    // `0` = unlimited: the allocation-heavy program runs to completion.
    let unlimited = run_with(&source, "LK_MAX_HEAP_OBJECTS", "0");
    assert!(
        unlimited.status.success(),
        "unlimited run should succeed: {}",
        String::from_utf8_lossy(&unlimited.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&unlimited.stdout).trim(), "5000");

    // Tight cap: aborts with a heap-object-limit error rather than growing on.
    let capped = run_with(&source, "LK_MAX_HEAP_OBJECTS", "500");
    assert!(!capped.status.success(), "capped run should fail");
    let stderr = String::from_utf8_lossy(&capped.stderr);
    assert!(
        stderr.contains("heap object limit exceeded"),
        "unexpected error: {stderr}"
    );

    let _ = fs::remove_file(&source);
}

// Allocates heavily (5000 short-lived lists) but keeps almost nothing reachable
// (`acc` is an int). The cap bounds the *live* set, so a collect-then-recheck
// must reclaim the churn instead of tripping the limit on transient garbage.
const CHURN: &str = "let acc = 0;\nfor i in 0..5000 { let tmp = [i, i, i]; acc = acc + i; }\nreturn acc;\n";

#[test]
fn heap_object_limit_ignores_transient_garbage() {
    let source = write_source("churn", CHURN);
    // A cap far below the allocation volume still succeeds — the live set stays
    // tiny after collection.
    let out = run_with(&source, "LK_MAX_HEAP_OBJECTS", "200");
    assert!(
        out.status.success(),
        "churn must not trip a live-object cap: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "12497500");
    let _ = fs::remove_file(&source);
}

// Doubles a string to 2^24 = 16 MiB in a handful of iterations — a fast way to
// blow past a byte budget.
const STRING_DOUBLING: &str = "let s = \"x\";\nfor i in 0..24 { s = s + s; }\nreturn s.len();\n";

#[test]
fn heap_byte_limit_aborts_runaway_allocation() {
    let source = write_source("bytes", STRING_DOUBLING);

    // `0` = unlimited: the ~16 MiB string builds to completion.
    let unlimited = run_with(&source, "LK_MAX_HEAP_BYTES", "0");
    assert!(
        unlimited.status.success(),
        "unlimited run should succeed: {}",
        String::from_utf8_lossy(&unlimited.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&unlimited.stdout).trim(), "16777216");

    // 8 MiB budget: aborts before the string reaches 16 MiB.
    let capped = run_with(&source, "LK_MAX_HEAP_BYTES", &(8 * 1024 * 1024).to_string());
    assert!(!capped.status.success(), "capped run should fail");
    assert!(
        String::from_utf8_lossy(&capped.stderr).contains("memory limit exceeded"),
        "unexpected error: {}",
        String::from_utf8_lossy(&capped.stderr)
    );

    let _ = fs::remove_file(&source);
}
