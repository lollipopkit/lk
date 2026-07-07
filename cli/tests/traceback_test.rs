//! Integration coverage for VM error tracebacks (plan M2.2): an uncaught error
//! prints the chain of named functions it unwound through, and a `pcall`-caught
//! error leaves no stale frames for a later error.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

fn bin_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lk"))
}

fn unique_tmp_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut path = std::env::temp_dir();
    path.push(format!("lk_{}_{}_{}", name, std::process::id(), seq));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("create tmp dir");
    path
}

fn run_stderr(source: &str) -> String {
    let dir = unique_tmp_dir("traceback");
    let file = dir.join("prog.lk");
    fs::write(&file, source).expect("write source");
    let output = Command::new(bin_path()).arg(&file).output().expect("spawn lk");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(!output.status.success(), "program was expected to error: {stderr}");
    let _ = fs::remove_dir_all(&dir);
    stderr
}

#[test]
fn uncaught_error_prints_named_call_stack() {
    // `recurse` is recursive (not inlined), so each frame is a real runtime
    // call that the error unwinds through.
    let stderr = run_stderr(
        "fn recurse(n) {\n    if n <= 0 { return error(\"boom\"); }\n    return recurse(n - 1);\n}\nreturn recurse(3);\n",
    );
    assert!(stderr.contains("Call stack:"), "expected a traceback header: {stderr}");
    assert!(
        stderr.contains("recurse"),
        "expected the failing function name: {stderr}"
    );
}

#[test]
fn caught_error_leaves_no_stale_frames() {
    // `boom` errors but is caught by try/catch; the later uncaught `later`
    // error's traceback must contain only `later`, not the discarded `boom`
    // frame.
    let stderr = run_stderr(
        "fn boom() { return error(\"caught\"); }\n\
         fn later() { return error(\"real\"); }\n\
         try { boom(); } catch e { }\n\
         return later();\n",
    );
    assert!(
        stderr.contains("later"),
        "expected the uncaught function in the trace: {stderr}"
    );
    assert!(
        !stderr.contains("boom"),
        "the caught frame must not leak into a later traceback: {stderr}"
    );
}
