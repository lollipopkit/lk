//! End-to-end proof for the Cranelift backend: a hand-built MIR module is
//! lowered to a native object (`lk_aot_codegen::clif`), linked against `lkrt`
//! (`compile_native_executable_from_object`), executed, and its stdout checked.
//!
//! This is the first program to run *through Cranelift* rather than the
//! string-IR path — the "runnable + differentially-checkable" milestone. It
//! needs `clang` and the `lkrt` staticlib on disk, so it is skipped when the
//! toolchain is unavailable rather than failing a lint-only environment.

use std::path::PathBuf;
use std::process::Command;

use lk_aot_mir::{Block, BlockId, Const, FuncId, GlobalId, Inst, MirFunction, MirModule, Term, Ty, ValueId};

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

    let mir = println_program("hi from cranelift");
    let object = lk_aot_codegen::clif::compile_host_object(&mir).expect("MIR must lower to a native object");

    let workdir = std::env::temp_dir().join(format!("lk-clif-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&workdir).expect("create work dir");
    let exe = workdir.join("clif_hello");
    let stamp = workdir.join("clif_hello.src");

    lk_llvm::compile_native_executable_from_object(&stamp, &exe, &object).expect("link native object against lkrt");

    let run = Command::new(&exe).output().expect("run the compiled executable");
    let stdout = String::from_utf8_lossy(&run.stdout);
    let _ = std::fs::remove_dir_all(&workdir);

    assert!(
        run.status.success(),
        "compiled program exited non-zero: status={:?} stderr={}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        stdout, "hi from cranelift\n",
        "stdout from the Cranelift-compiled program must match the source"
    );
}
