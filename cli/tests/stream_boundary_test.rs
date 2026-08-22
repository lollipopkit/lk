//! A stream cannot cross a module boundary while its pipeline holds heap values.
//!
//! A stream is an id into a process-global registry *plus* handles: its `roots`
//! are heap references, and the pipeline the registry holds for that id keeps
//! its `map`/`filter` callbacks as heap references too. Both belong to the heap
//! that built them, and a copy between heaps rewrote neither — so the other
//! side got an id whose callbacks pointed into a heap it could not read.
//!
//! What that produced depended on when the collector ran. Plainly, the callback
//! handle was out of range and the program died with `heap object 102 out of
//! bounds`. With a collection in between, the slot had been reused and the
//! filter was silently skipped: `[1,2,3,4,5,6]` came back where `[16,25,36]`
//! was asked for. The second is the one worth a test.
//!
//! Refused rather than repaired: repairing means rewriting the *registry's*
//! pipeline into the destination module, and the registry lives in the stdlib,
//! which `core` must not reach into.

use std::process::{Command, Stdio};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_lk"))
}

fn run_file(dir: &std::path::Path, name: &str) -> (String, String) {
    let out = bin()
        .current_dir(dir)
        .arg(name)
        .env("LK_FORCE_VM", "1")
        .output()
        .expect("run lk");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// Building the pipeline in one module and consuming it in another is refused,
/// and the refusal says what to do instead.
#[test]
fn a_stream_with_a_callback_cannot_cross_a_module_boundary() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("slib.lk"),
        "use stream;\n\
         fn filtered() -> Any {\n\
             let s = stream.from_list([1, 2, 3, 4, 5, 6]);\n\
             return stream.filter(s, fn(x) => x > 3);\n\
         }\n",
    )
    .expect("write slib");
    std::fs::write(
        dir.path().join("main.lk"),
        "use stream;\n\
         use { filtered } from \"slib\";\n\
         println(stream.collect(stream.map(filtered(), fn(x) => x * x)));\n",
    )
    .expect("write main");

    let (stdout, stderr) = run_file(dir.path(), "main.lk");
    assert!(
        !stdout.contains("[1,4,9,16,25,36]"),
        "the filter must not be silently skipped: {stdout}"
    );
    assert!(
        stderr.contains("cannot be") && stderr.contains("stream.collect"),
        "the refusal should name the way out: {stderr}"
    );
}

/// A stream whose pipeline holds no heap value is just an id, and crosses.
///
/// Refusing every stream would have been the easy rule and the wrong one: it
/// would break `stream.range(…)` and a list of scalars, which are sound.
#[test]
fn a_stream_with_no_heap_roots_still_crosses() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("slib.lk"),
        "use stream;\n\
         fn counted() -> Any { return stream.range(0, 5); }\n",
    )
    .expect("write slib");
    std::fs::write(
        dir.path().join("main.lk"),
        "use stream;\n\
         use { counted } from \"slib\";\n\
         println(stream.collect(counted()));\n",
    )
    .expect("write main");

    let (stdout, stderr) = run_file(dir.path(), "main.lk");
    assert_eq!(stdout.trim(), "[0,1,2,3,4]", "stderr: {stderr}");
}

/// The same rule on the REPL's path, which is a different copy implementation:
/// every input is its own module, so the pipeline crosses on the next line.
#[test]
fn the_repl_refuses_the_same_crossing() {
    use std::io::Write;

    let mut child = bin()
        .env("LK_FORCE_VM", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn repl");
    child
        .stdin
        .as_mut()
        .expect("piped stdin")
        .write_all(
            b"use stream;\n\
              let s = stream.from_list([1, 2, 3, 4, 5, 6]);\n\
              let f = stream.filter(s, fn(x) => x > 3);\n\
              stream.collect(f)\n",
        )
        .expect("write session");
    let out = child.wait_with_output().expect("repl output");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    assert!(
        !stdout.contains("[1,2,3,4,5,6]"),
        "the filter must not be silently skipped: {stdout}"
    );
    assert!(
        stderr.contains("cannot be imported"),
        "the refusal should reach the session: {stderr}"
    );
}
