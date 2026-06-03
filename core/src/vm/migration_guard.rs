use std::{fs, path::Path};

const FORBIDDEN_TOKENS: &[(&str, &str)] = &[
    ("Frame32", "shared stack Executor32 must remain the VM call hot path"),
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
    let source = fs::read_to_string(path).expect("read source file");
    let relative = path.strip_prefix(manifest_dir).unwrap_or(path);
    for (token, reason) in FORBIDDEN_TOKENS {
        for (line_index, line) in source.lines().enumerate() {
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
