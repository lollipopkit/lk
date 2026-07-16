//! Integration coverage for the opt-in sandbox resource limits (plan M2.6):
//! `LK_MAX_HEAP_OBJECTS` caps the number of live heap objects, aborting a
//! runaway allocation with a catchable heap-limit error instead of growing
//! unbounded. Unset/`0` means unlimited.

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
