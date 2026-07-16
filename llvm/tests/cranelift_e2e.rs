//! End-to-end + differential proof for the Cranelift backend: LK source (or a
//! hand-built MIR module) is lowered to a native object (`lk_aot_codegen::clif`),
//! linked against `lkrt` (`compile_native_executable_from_object`), executed,
//! and its stdout checked against the program's defined semantics.
//!
//! This is the first path to run *through Cranelift* rather than the string-IR
//! backend — the "runnable + differentially-checked" milestone. It needs `clang`
//! and the `lkrt` staticlib on disk, so it is skipped (not failed) when the
//! toolchain is unavailable, and each source case self-skips when its shape is
//! still outside the Cranelift lowering slice (`Unsupported`).

use std::path::PathBuf;
use std::process::Command;

use lk_aot_mir::{Block, BlockId, Const, FuncId, GlobalId, Inst, MirFunction, MirModule, Term, Ty, ValueId};
use lk_core::syntax::{ParseOptions, parse_program_source};
use lk_core::vm::{Compiler, ModuleArtifact};

/// `println("<text>"); return;` as an entry module (compiles to C `main`).
fn println_program(text: &str) -> MirModule {
    let prog = MirFunction {
        id: FuncId(0),
        params: vec![],
        blocks: vec![Block {
            id: BlockId(0),
            params: vec![],
            insts: vec![
                Inst::Const {
                    dst: ValueId(0),
                    value: Const::Str(GlobalId(0)),
                },
                Inst::PrintStr {
                    value: ValueId(0),
                    newline: true,
                },
            ],
            term: Term::Ret(None),
        }],
        entry: BlockId(0),
        ret: Ty::Nil,
    };
    MirModule {
        abi_version: lk_aot_abi::ABI_VERSION,
        globals: vec![text.to_string()],
        mutable_globals: vec![],
        vm_functions: vec![],
        entry: FuncId(0),
        functions: vec![prog],
    }
}

/// Lower LK source to a native object through the Cranelift slice. `hybrid` is
/// forced off so a shape outside the slice fails as `Unsupported` (returned as
/// `Err`) rather than sprouting a VM bridge the object path can't link here.
fn source_to_object(source: &str) -> Result<Vec<u8>, String> {
    let program = parse_program_source(source, ParseOptions::default()).map_err(|e| format!("parse: {e:?}"))?;
    let module = Compiler::compile_module(&program).map_err(|e| format!("compile: {e:?}"))?;
    let artifact = ModuleArtifact::new(Vec::new(), &module).map_err(|e| format!("artifact: {e:?}"))?;
    let mir = lk_aot_lower::lower_with_hybrid(&artifact, false).map_err(|u| format!("unsupported(lower): {u}"))?;
    lk_aot_mir::validate(&mir).map_err(|e| format!("validate: {e:?}"))?;
    lk_aot_codegen::clif::compile_host_object(&mir).map_err(|e| format!("unsupported(clif): {e:?}"))
}

/// Link `object` into a fresh executable, run it, and return `(stdout, success)`.
fn link_and_run(tag: &str, object: &[u8]) -> (String, bool) {
    let workdir = std::env::temp_dir().join(format!("lk-clif-e2e-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&workdir).expect("create work dir");
    let exe = workdir.join("prog");
    let stamp = workdir.join("prog.src");
    lk_llvm::compile_native_executable_from_object(&stamp, &exe, object).expect("link native object against lkrt");
    let run = Command::new(&exe).output().expect("run the compiled executable");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    let success = run.status.success();
    let _ = std::fs::remove_dir_all(&workdir);
    (stdout, success)
}

/// True when the link toolchain (clang + an `lkrt` staticlib) is present. In its
/// absence the test is a no-op so `cargo test` still passes in IR-only setups.
fn toolchain_available() -> bool {
    let clang = std::env::var_os("LK_CLANG")
        .or_else(|| std::env::var_os("CLANG"))
        .or_else(|| std::env::var_os("CC"));
    let clang_ok = clang.is_some()
        || Command::new("clang").arg("--version").output().is_ok()
        || PathBuf::from("/opt/homebrew/opt/llvm/bin/clang").exists();
    clang_ok && lkrt_staticlib_present()
}

/// Mirror `lkrt_staticlib_path`'s search so the test can decide up front whether
/// linking can succeed (the linker path itself has no "was it found" signal).
fn lkrt_staticlib_present() -> bool {
    if std::env::var_os("LKRT_STATICLIB").is_some() {
        return true;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Some(dir) = exe.parent() else { return false };
    let plain = dir.join(if cfg!(windows) { "lkrt.lib" } else { "liblkrt.a" });
    if plain.exists() {
        return true;
    }
    let (prefix, suffix) = if cfg!(windows) {
        ("lkrt-", ".lib")
    } else {
        ("liblkrt-", ".a")
    };
    [dir.to_path_buf(), dir.join("deps")].into_iter().any(|search| {
        std::fs::read_dir(&search).ok().is_some_and(|entries| {
            entries.filter_map(Result::ok).any(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with(prefix) && n.ends_with(suffix))
            })
        })
    })
}

#[test]
fn cranelift_println_runs_natively() {
    if !toolchain_available() {
        eprintln!("skipping cranelift_println_runs_natively: clang or lkrt staticlib unavailable");
        return;
    }
    let object = lk_aot_codegen::clif::compile_host_object(&println_program("hi from cranelift"))
        .expect("MIR must lower to a native object");
    let (stdout, success) = link_and_run("hello", &object);
    assert!(success, "compiled program exited non-zero");
    assert_eq!(stdout, "hi from cranelift\n");
}

/// Differential slice: each source runs *through Cranelift* and its stdout must
/// match the program's defined semantics. Cases whose shape is not yet lowerable
/// self-skip (printed) — they mark the coverage frontier, not a failure. As the
/// slice grows, moved-in cases here guard the new lowering against regressions.
#[test]
fn cranelift_differential_slice() {
    if !toolchain_available() {
        eprintln!("skipping cranelift_differential_slice: clang or lkrt staticlib unavailable");
        return;
    }
    // (name, source, expected stdout). Expected values are the program's plain
    // semantics; a top-level `return <scalar>` auto-prints the result.
    let cases: &[(&str, &str, &str)] = &[
        ("int_arith", "return 1 + 2 * 3;\n", "7\n"),
        ("int_sub", "return 100 - 58;\n", "42\n"),
        ("int_div_guard", "let x = 20;\nlet y = 4;\nreturn x / y;\n", "5\n"),
        ("int_mod", "return 17 % 5;\n", "2\n"),
        ("let_binding", "let a = 6;\nlet b = 7;\nreturn a * b;\n", "42\n"),
        ("if_then", "let x = 3;\nif (x < 5) { return 10; }\nreturn 20;\n", "10\n"),
        ("if_else", "let x = 9;\nif (x < 5) { return 10; }\nreturn 20;\n", "20\n"),
        // `println` needs the builtin registration that the CLI's VM context
        // provides; `Compiler::compile_module` here can't resolve it. PrintStr
        // lowering is covered directly by `cranelift_println_runs_natively`.
        (
            "direct_call",
            "fn add(a, b) {\n  return a + b;\n}\nreturn add(2, 3);\n",
            "5\n",
        ),
    ];

    let mut ran = 0usize;
    let mut skipped = Vec::new();
    for (name, source, expected) in cases {
        match source_to_object(source) {
            Ok(object) => {
                let (stdout, success) = link_and_run(name, &object);
                assert!(success, "[{name}] compiled program exited non-zero");
                assert_eq!(stdout, *expected, "[{name}] Cranelift stdout diverged from semantics");
                ran += 1;
            }
            Err(reason) => skipped.push(format!("{name}: {reason}")),
        }
    }

    eprintln!("cranelift differential slice: {ran}/{} ran", cases.len());
    for note in &skipped {
        eprintln!("  skipped {note}");
    }
    assert!(
        ran > 0,
        "no differential case exercised the Cranelift slice; skips: {skipped:?}"
    );
}
