#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use std::{fs, path::Path};

const FORBIDDEN_TOKENS: &[(&str, &str)] = &[
    ("struct Frame", "shared stack Executor must remain the VM call hot path"),
    ("enum Op {", "runtime must not reintroduce the old Op instruction enum"),
    (
        "struct Op {",
        "runtime must not reintroduce the old Op instruction type",
    ),
    ("ListFoldAdd", "benchmark-shaped fused opcodes are forbidden"),
    ("MapValuesFoldAdd", "benchmark-shaped fused opcodes are forbidden"),
    ("AddRangeCountImm", "benchmark-shaped fused opcodes are forbidden"),
    (
        "quickening",
        "runtime feedback/quickening must not return to the VM path",
    ),
    ("unsafe ", "LLVM-external VM/value code must stay safe Rust"),
    ("unsafe{", "LLVM-external VM/value code must stay safe Rust"),
    ("unsafe\n", "LLVM-external VM/value code must stay safe Rust"),
];

#[test]
fn vm_rewrite_guard_blocks_old_vm_compatibility_paths() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut violations = Vec::new();
    for root in [manifest_dir.join("src/vm"), manifest_dir.join("src/val")] {
        collect_violations(&root, manifest_dir, &mut violations);
    }
    assert!(
        violations.is_empty(),
        "VM rewrite guard found forbidden compatibility paths:\n{}",
        violations.join("\n")
    );
}

fn collect_violations(path: &Path, manifest_dir: &Path, violations: &mut Vec<String>) {
    let Ok(metadata) = fs::metadata(path) else {
        return;
    };
    if metadata.is_dir() {
        let mut entries = fs::read_dir(path)
            .expect("read source directory")
            .collect::<Result<Vec<_>, _>>()
            .expect("read source entries");
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            collect_violations(&entry.path(), manifest_dir, violations);
        }
        return;
    }
    if path.extension().and_then(|ext| ext.to_str()) != Some("rs") || path.ends_with("migration_guard.rs") {
        return;
    }
    // `vm/hardware.rs` is exempt from the no-`unsafe` rule.
    //
    // That rule exists because a memory error inside the interpreter is
    // unfindable. The reasoning does not reach volatile MMIO and interrupt
    // masking: touching a device register *is* the operation, and there is no
    // safe spelling of it. Since the exception must exist, it is confined to
    // one small file named for what it holds, rather than letting `unsafe`
    // spread through the executor. Nothing else under `vm/` may contain it.
    if path.ends_with("hardware.rs") {
        return;
    }
    let source = fs::read_to_string(path).expect("read source file");
    let relative = path.strip_prefix(manifest_dir).unwrap_or(path);
    let scannable = scannable_lines(&source);
    for (token, reason) in FORBIDDEN_TOKENS {
        for (line_index, line) in scannable.iter().enumerate() {
            if line.contains(token) {
                violations.push(format!(
                    "{}:{} contains `{}` ({})",
                    relative.display(),
                    line_index + 1,
                    token,
                    reason
                ));
            }
        }
    }
}

/// A line reduced to the Rust it actually compiles: string literals blanked,
/// line comments dropped.
///
/// The guard matches raw text, so an LK program *quoted in a test* — or merely
/// *described in a doc comment* — looked like Rust. `unsafe { … }` is a
/// construct of the language this crate implements, so neither testing it nor
/// writing it down was possible anywhere under `vm/`: a hole the guard itself
/// created, and one that would have silenced the guard by teaching people to
/// avoid the word.
///
/// Quotes are tracked one line at a time, with `\"` escaped. A multi-line raw
/// string (`r#"…"#`) and a block comment are not understood, so a forbidden
/// token inside one still reports; that is the safe direction to be wrong in.
fn scannable_lines(source: &str) -> Vec<String> {
    // Carried across lines: a Rust string literal may span them with a
    // trailing `\`, which is how a multi-line LK sample is written in a test.
    let mut in_string = false;
    source
        .lines()
        .map(|line| scannable_code(line, &mut in_string))
        .collect()
}

fn scannable_code(line: &str, in_string: &mut bool) -> String {
    let mut out = String::with_capacity(line.len());
    let mut escaped = false;
    for ch in line.chars() {
        if *in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                *in_string = false;
            }
            out.push(' ');
            continue;
        }
        if ch == '"' {
            *in_string = true;
            out.push(' ');
            continue;
        }
        if ch == '/' && out.ends_with('/') {
            out.pop();
            break;
        }
        out.push(ch);
    }
    out
}
