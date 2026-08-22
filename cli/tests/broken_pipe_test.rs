//! `lk prog.lk | head` must stop, not panic.
//!
//! Rust ignores `SIGPIPE` before `main`, which turns a write to a closed pipe
//! into `EPIPE` and then into a `println!` panic — the CLI printed
//! `thread 'main' panicked at library/std/src/io/stdio.rs …` and exited 101.
//! The AOT-compiled binary has a C `main` and never got that startup, so it
//! already died with signal 13. This pins the interpreter to the same answer.
#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[test]
fn a_reader_that_goes_away_stops_the_program_instead_of_panicking() {
    let dir = std::env::temp_dir().join(format!("lk_pipe_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let script = dir.join("spew.lk");
    // Long enough that the child is certainly still writing when the reader
    // goes away, and unbuffered enough that the first line arrives promptly.
    std::fs::write(&script, "for i in 1..=200000 { println(\"${i}\"); }\n").expect("write script");

    let mut child = Command::new(PathBuf::from(env!("CARGO_BIN_EXE_lk")))
        .arg(&script)
        .env("LK_FORCE_VM", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn lk");

    // Read one line, then drop the pipe — this is what `| head -1` does.
    let stdout = child.stdout.take().expect("piped stdout");
    let mut reader = BufReader::new(stdout);
    let mut first = String::new();
    reader.read_line(&mut first).expect("read first line");
    assert_eq!(first.trim(), "1", "the program should print before the reader leaves");
    drop(reader);

    let output = child.wait_with_output().expect("wait");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panicked"),
        "a closed pipe must not surface as a Rust panic, got: {stderr}"
    );
    assert_eq!(
        output.status.signal(),
        Some(libc::SIGPIPE),
        "expected death by SIGPIPE like any other filter, got {:?} (stderr: {stderr})",
        output.status
    );

    let _ = std::fs::remove_dir_all(&dir);
}
