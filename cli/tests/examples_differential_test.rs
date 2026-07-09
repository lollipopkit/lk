//! Differential corpus over `examples/{syntax,stdlib,general}`: every example
//! that the MIR pipeline can lower natively must behave exactly like the VM
//! (stdout + success/failure). Examples the pipeline rejects are recorded as
//! coverage gaps, not failures — this doubles as a lowering-coverage snapshot
//! over real programs instead of hand-written differential cases only.
#![cfg(feature = "llvm")]

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
    stderr: String,
    exit_code: Option<i32>,
    success: bool,
    timed_out: bool,
}

impl RunResult {
    /// Diagnostic block for divergence reports: exit code plus the captured
    /// stderr (the CI log otherwise hides why a side failed).
    fn diagnostics(&self, side: &str) -> String {
        let code = match self.exit_code {
            Some(code) => code.to_string(),
            None => "killed".to_string(),
        };
        format!("--- {side} exit={code} stderr ---\n{}", self.stderr)
    }
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
        stderr: fs::read_to_string(&stderr_path).unwrap_or_default(),
        exit_code: status.and_then(|status| status.code()),
        success: status.is_some_and(|status| status.success()),
        timed_out: status.is_none(),
    }
}

fn collect_examples(corpus_root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for area in ["syntax", "stdlib", "general"] {
        let dir = corpus_root.join(area);
        let entries = fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
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

/// Copies the whole examples tree so relative imports keep working while
/// compile outputs (`.ll`, executables) land outside the repository tree.
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

#[test]
fn examples_corpus_differential() {
    let scratch = std::env::temp_dir().join(format!("lk_examples_diff_{}", std::process::id()));
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

        // IR compile: success means the MIR pipeline accepted the program.
        let mut compile_ir = Command::new(bin_path());
        compile_ir.current_dir(&dir).args(["compile", "llvm", &file]);
        let ir_result = run_with_timeout(compile_ir, &scratch, &format!("{stem}_ir"));
        if !ir_result.success {
            let reason = fs::read_to_string(scratch.join(format!("{stem}_ir.stderr")))
                .unwrap_or_default()
                .lines()
                .last()
                .unwrap_or("unknown")
                .to_string();
            unsupported.push((label, reason));
            continue;
        }
        let ir = fs::read_to_string(dir.join(format!("{stem}.ll"))).unwrap_or_default();
        if !ir.contains("; ModuleID = 'lk_aot'") {
            unsupported.push((label, "compiled but not via the MIR pipeline".to_string()));
            continue;
        }

        // VM reference run.
        let mut vm_cmd = Command::new(bin_path());
        vm_cmd.current_dir(&dir).arg(&file).env("LK_FORCE_VM", "1");
        let vm = run_with_timeout(vm_cmd, &scratch, &format!("{stem}_vm"));
        if vm.timed_out {
            skipped.push((label, "VM run timed out".to_string()));
            continue;
        }

        // Native build + run.
        let mut compile_exe = Command::new(bin_path());
        compile_exe.current_dir(&dir).args(["compile", &file]);
        let exe = run_with_timeout(compile_exe, &scratch, &format!("{stem}_exe"));
        if !exe.success {
            let reason = fs::read_to_string(scratch.join(format!("{stem}_exe.stderr"))).unwrap_or_default();
            divergences.push(format!(
                "[{label}] IR compiled via MIR but native executable build failed:\n{reason}"
            ));
            continue;
        }
        let native = run_with_timeout(Command::new(dir.join(&stem)), &scratch, &format!("{stem}_native"));
        if native.timed_out {
            divergences.push(format!("[{label}] native run timed out while the VM completed"));
            continue;
        }

        if vm.stdout != native.stdout {
            divergences.push(format!(
                "[{label}] stdout diverged\n--- vm ---\n{}\n--- native ---\n{}\n{}\n{}",
                vm.stdout,
                native.stdout,
                vm.diagnostics("vm"),
                native.diagnostics("native")
            ));
        } else if vm.success != native.success {
            divergences.push(format!(
                "[{label}] success/failure diverged: vm={} native={}\n{}\n{}",
                vm.success,
                native.success,
                vm.diagnostics("vm"),
                native.diagnostics("native")
            ));
        } else {
            compared.push(label);
        }
    }

    println!(
        "examples corpus: {} compared, {} unsupported, {} skipped",
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

    let _ = fs::remove_dir_all(&scratch);

    assert!(
        divergences.is_empty(),
        "examples corpus diverged between VM and native:\n{}",
        divergences.join("\n\n")
    );
    // Coverage floor: the MIR pipeline currently lowers 10 of the 44 examples
    // (general/fib, stdlib/{datetime,io,math,os,time}_demo, syntax/internal,
    // syntax/named_args, syntax/named_params, syntax/numeric_auto_promotion).
    // Falling below this means the differential corpus silently stopped
    // comparing anything — raise the floor as lowering coverage grows.
    assert!(
        compared.len() >= 10,
        "expected at least 10 natively compared examples, got {}",
        compared.len()
    );
}
