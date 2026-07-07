//! Differential over `examples/{syntax,stdlib,general}`: for every deterministic
//! example, running from source must produce byte-identical output to running
//! the serialized-then-reloaded bytecode module (`lk compile bytecode` →
//! `.lkm` → `lk FILE.lkm`). This is the `VM(source) == VM(bytecode)` oracle
//! (plan M1.2): it proves the `ModuleArtifact` serialization round-trip
//! preserves execution semantics, independent of the LLVM/AOT backend.
//!
//! Non-deterministic examples (time/random/uuid/net) are detected automatically
//! by running the source twice and skipping any whose two runs disagree, so the
//! corpus needs no hand-maintained skip list. Examples the bytecode compiler
//! rejects (imports/proc-macros it cannot serialize standalone) are recorded as
//! coverage gaps, not failures.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const RUN_TIMEOUT: Duration = Duration::from_secs(30);

fn bin_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lk"))
}

fn examples_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../examples")
}

struct RunResult {
    stdout: String,
    success: bool,
    timed_out: bool,
}

/// Runs a command with stdout/stderr captured to files (avoids pipe-buffer
/// deadlocks) and a hard timeout, killing the child on expiry.
fn run_with_timeout(mut command: Command, scratch: &Path, tag: &str) -> RunResult {
    let stdout_path = scratch.join(format!("{tag}.stdout"));
    let stderr_path = scratch.join(format!("{tag}.stderr"));
    let stdout_file = File::create(&stdout_path).expect("create stdout capture");
    let stderr_file = File::create(&stderr_path).expect("create stderr capture");
    let mut child = command
        .stdin(Stdio::null())
        .stdout(stdout_file)
        .stderr(stderr_file)
        .spawn()
        .expect("spawn command");
    let started = Instant::now();
    let status = loop {
        match child.try_wait().expect("poll child") {
            Some(status) => break Some(status),
            None if started.elapsed() > RUN_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    RunResult {
        stdout: fs::read_to_string(&stdout_path).unwrap_or_default(),
        success: status.is_some_and(|status| status.success()),
        timed_out: status.is_none(),
    }
}

fn collect_examples(corpus_root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for area in ["syntax", "stdlib", "general"] {
        let dir = corpus_root.join(area);
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("dir entry").path();
            if path.extension() == Some(OsStr::new("lk")) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Copies the whole examples tree so relative imports keep working while the
/// emitted `.lkm` modules land outside the repository tree.
fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("create tree dir");
    for entry in fs::read_dir(from).expect("read tree dir") {
        let entry = entry.expect("tree entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy tree file");
        }
    }
}

fn run_source(dir: &Path, file: &str, scratch: &Path, tag: &str) -> RunResult {
    let mut cmd = Command::new(bin_path());
    cmd.current_dir(dir).arg(file).env("LK_FORCE_VM", "1");
    run_with_timeout(cmd, scratch, tag)
}

#[test]
fn vm_bytecode_corpus_differential() {
    let scratch = std::env::temp_dir().join(format!("lk_vm_bytecode_diff_{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).expect("create scratch dir");
    let corpus = scratch.join("examples");
    copy_tree(&examples_root(), &corpus);

    let mut unsupported: Vec<(String, String)> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();
    let mut compared: Vec<String> = Vec::new();
    let mut divergences: Vec<String> = Vec::new();

    for example in collect_examples(&corpus) {
        let dir = example.parent().expect("example dir").to_path_buf();
        let file = example
            .file_name()
            .and_then(OsStr::to_str)
            .expect("example file name")
            .to_string();
        let stem = example
            .file_stem()
            .and_then(OsStr::to_str)
            .expect("example stem")
            .to_string();
        let label = format!("{}/{}", dir.file_name().and_then(OsStr::to_str).unwrap_or("?"), file);

        // Source reference run.
        let source = run_source(&dir, &file, &scratch, &format!("{stem}_src1"));
        if source.timed_out {
            skipped.push((label, "source run timed out".to_string()));
            continue;
        }
        if !source.success {
            // Programs that intentionally fail (or need args/stdin) are not a
            // clean golden: skip rather than guess the expected failure mode.
            skipped.push((label, "source run did not succeed".to_string()));
            continue;
        }

        // Determinism gate: a second source run must match, else the example is
        // non-deterministic (time/random/uuid/net) and cannot be a golden.
        let source2 = run_source(&dir, &file, &scratch, &format!("{stem}_src2"));
        if source2.stdout != source.stdout || !source2.success {
            skipped.push((label, "non-deterministic output across source runs".to_string()));
            continue;
        }

        // Compile to a bytecode module (writes `<stem>.lkm` next to the source).
        let mut compile = Command::new(bin_path());
        compile.current_dir(&dir).args(["compile", "bytecode", &file]);
        let compiled = run_with_timeout(compile, &scratch, &format!("{stem}_compile"));
        if !compiled.success {
            let reason = fs::read_to_string(scratch.join(format!("{stem}_compile.stderr")))
                .unwrap_or_default()
                .lines()
                .last()
                .unwrap_or("unknown")
                .to_string();
            unsupported.push((label, reason));
            continue;
        }
        let module = format!("{stem}.lkm");
        if !dir.join(&module).exists() {
            unsupported.push((label, "compile reported success but no .lkm emitted".to_string()));
            continue;
        }

        // Bytecode round-trip run.
        let mut bytecode_cmd = Command::new(bin_path());
        bytecode_cmd.current_dir(&dir).arg(&module).env("LK_FORCE_VM", "1");
        let bytecode = run_with_timeout(bytecode_cmd, &scratch, &format!("{stem}_bytecode"));
        if bytecode.timed_out {
            divergences.push(format!("[{label}] bytecode run timed out while the source completed"));
            continue;
        }

        if source.stdout != bytecode.stdout {
            divergences.push(format!(
                "[{label}] stdout diverged\n--- source ---\n{}\n--- bytecode ---\n{}",
                source.stdout, bytecode.stdout
            ));
        } else if source.success != bytecode.success {
            divergences.push(format!(
                "[{label}] success/failure diverged: source={} bytecode={}",
                source.success, bytecode.success
            ));
        } else {
            compared.push(label);
        }
    }

    let _ = fs::remove_dir_all(&scratch);

    println!(
        "vm/bytecode corpus: {} compared, {} unsupported, {} skipped",
        compared.len(),
        unsupported.len(),
        skipped.len()
    );
    for label in &compared {
        println!("  compared: {label}");
    }
    for (label, reason) in &unsupported {
        println!("  unsupported: {label}: {reason}");
    }
    for (label, reason) in &skipped {
        println!("  skipped: {label}: {reason}");
    }

    assert!(
        divergences.is_empty(),
        "VM(source) != VM(bytecode) divergences:\n{}",
        divergences.join("\n")
    );
    assert!(
        !compared.is_empty(),
        "no examples were compared source-vs-bytecode; harness or corpus is broken"
    );
}
