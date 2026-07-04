//! Tier 1 hybrid lowering tests (`docs/llvm/tier1-hybrid.md`): with hybrid
//! mode on, a reachable non-entry function whose body does not lower is marked
//! VM-executed when bridge-eligible (scalar params, no captures, transitively
//! user-global-free) and its call sites become `call.vm`; anything outside
//! that envelope still fails the module whole (Tier 0 fallback path).

use lk_core::syntax::{ParseOptions, parse_program_source};
use lk_core::vm::{Compiler, ModuleArtifact};

fn artifact(source: &str) -> ModuleArtifact {
    let program = parse_program_source(source, ParseOptions::default()).expect("parse");
    // The try/catch desugar references runtime builtins; declare them as
    // external globals the way a CLI compile (full stdlib context) would.
    let externals: Vec<String> = ["pcall", "assert", "error", "println"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), externals).expect("compile");
    ModuleArtifact::new(Vec::new(), &module).expect("artifact")
}

/// A statement-position call to a function whose body uses try/catch (pcall
/// desugar — dynamic `Call`, outside the subset) with a scalar parameter.
const REPORT_PROGRAM: &str = "\
fn report(x) { try { assert(x > 0); } catch e { } }\n\
let acc = 0;\n\
for i in 0..10 { acc += i; }\n\
report(acc);\n\
return acc;\n";

#[test]
fn hybrid_marks_eligible_unlowerable_callee_as_vm_executed() {
    let artifact = artifact(REPORT_PROGRAM);
    // Without hybrid mode the module falls back whole (pre-existing behavior).
    lk_aot_lower::lower_with_hybrid(&artifact, false).expect_err("report's body must not lower natively");

    let mir = lk_aot_lower::lower_with_hybrid(&artifact, true).expect("hybrid lowering succeeds");
    lk_aot_mir::validate(&mir).expect("hybrid module validates");
    assert_eq!(mir.vm_functions.len(), 1, "exactly `report` is VM-executed");
    assert_eq!(
        mir.vm_functions[0].params,
        vec![lk_aot_mir::Ty::I64],
        "the bridge marshals report's parameter as i64"
    );
    let rendered = lk_aot_mir::render(&mir);
    assert!(
        rendered.contains("vm fn f"),
        "render lists the VM function:\n{rendered}"
    );
    assert!(rendered.contains("call.vm f"), "the call site bridges:\n{rendered}");
    // The try/catch desugar closure is reachable only through `report` — it
    // must not appear as a native function or a VM signature.
    let codegen = lk_aot_codegen::render_module(&mir);
    assert!(
        codegen.contains("declare void @lk_hybrid_call_v(i32, ptr, i64)"),
        "codegen declares the bridge:\n{codegen}"
    );
    assert!(
        codegen.contains("call void @lk_hybrid_call_v(i32 "),
        "codegen emits the bridge call:\n{codegen}"
    );
    assert!(
        codegen.contains("call i64 @lkrt_io_std_flush(i64 1)"),
        "C stdio flushes before entering the VM:\n{codegen}"
    );
}

#[test]
fn hybrid_rejects_a_used_bridge_result() {
    // The bridged callee's result is *used* — the destination register stays
    // unbound, so the module must fall back whole rather than miscompile.
    let artifact = artifact(
        "fn get(x) { try { return x; } catch e { return 0; } }\n\
         let v = get(3);\n\
         return v;\n",
    );
    lk_aot_lower::lower_with_hybrid(&artifact, true).expect_err("using a bridged result must reject");
}

#[test]
fn hybrid_rejects_a_global_touching_callee() {
    // The callee's VM-side subtree writes a module global: the bridge VM's
    // copy would diverge from native storage, so it must reject.
    let artifact = artifact(
        "let counter = 0;\n\
         fn bump() { try { counter = counter + 1; } catch e { } }\n\
         bump();\n\
         return counter;\n",
    );
    lk_aot_lower::lower_with_hybrid(&artifact, true).expect_err("global-writing callee must reject");
}

#[test]
fn hybrid_off_by_default() {
    // `lower()` keeps the pre-hybrid behavior until the CLI links the bridge
    // runtime (LK_AOT_HYBRID opt-in): same program, whole-module fallback.
    let artifact = artifact(REPORT_PROGRAM);
    if std::env::var_os("LK_AOT_HYBRID").is_none() {
        lk_aot_lower::lower(&artifact).expect_err("hybrid stays opt-in by default");
    }
}
