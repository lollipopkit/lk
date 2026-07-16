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
    let externals: Vec<String> = ["try$call", "assert", "error", "println"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let module = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), externals).expect("compile");
    ModuleArtifact::new(Vec::new(), &module).expect("artifact")
}

/// A statement-position call to a function whose body does not lower (a
/// *dynamic* format string with extra args is the documented println
/// reject) with a scalar parameter. try/catch — the previous ingredient —
/// lowers natively since plan G.
const REPORT_PROGRAM: &str = "\
fn report(x) { let f = \"v={}\".trim(); println(f, x); }\n\
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
    // Nothing beyond `report` may leak into the bridge surface.
}

#[test]
fn hybrid_binds_a_used_bridge_result_as_dyn() {
    // v2 return bridge: the bridged callee's result is *used* — the
    // destination binds as `Dyn` and codegen emits the value-returning
    // `lk_hybrid_call_r` (v1 rejected this shape whole-module).
    let artifact = artifact(
        "fn get(x) { let f = \"v={}\".trim(); println(f, x); return x; }\n\
         let v = get(3);\n\
         return v;\n",
    );
    let mir = lk_aot_lower::lower_with_hybrid(&artifact, true).expect("a used bridged result lowers (v2)");
    lk_aot_mir::validate(&mir).expect("hybrid module validates");
    let rendered = lk_aot_mir::render(&mir);
    assert!(
        rendered.contains("= call.vm f"),
        "the bridge call binds its destination:\n{rendered}"
    );
}

#[test]
fn hybrid_degrades_a_discarded_bridge_result_to_the_void_call() {
    // Statement-position bridge calls keep the v1 zero-marshal path: the
    // bound destination is never read, so codegen degrades to call_v.
    let artifact = artifact(REPORT_PROGRAM);
    let mir = lk_aot_lower::lower_with_hybrid(&artifact, true).expect("hybrid lowering succeeds");
    // The discarded bridge result must produce a VM-executed function.
    assert_eq!(mir.vm_functions.len(), 1, "the bridged callee is VM-executed");
    // Lowering always binds a `CallVm` destination; the void-call degrade is a
    // codegen concern gated on the destination being unread. Assert that
    // *condition* here: the statement-position bridge call's result is never
    // used, so codegen (`clif::call_vm`) degrades it to `lk_hybrid_call_v`.
    let (func, call_dst) = mir
        .functions
        .iter()
        .find_map(|f| {
            f.blocks.iter().flat_map(|b| &b.insts).find_map(|inst| match inst {
                lk_aot_mir::Inst::CallVm { dst: Some(dst), .. } => Some((f, *dst)),
                _ => None,
            })
        })
        .expect("a CallVm with a bound destination");
    assert!(
        !lk_aot_mir::value_is_used(func, call_dst),
        "the discarded bridge result must be unread (so codegen degrades to the void call)"
    );
}

#[test]
fn hybrid_rejects_a_global_touching_callee() {
    // The callee's VM-side subtree writes a module global: the bridge VM's
    // copy would diverge from native storage, so it must reject.
    let artifact = artifact(
        "let counter = 0;\n\
         fn bump() { let f = \"v={}\".trim(); println(f, counter); counter = counter + 1; }\n\
         bump();\n\
         return counter;\n",
    );
    lk_aot_lower::lower_with_hybrid(&artifact, true).expect_err("global-writing callee must reject");
}

#[test]
fn hybrid_on_by_default() {
    // `lower()` bridges by default; `LK_AOT_HYBRID=0` opts out (the same
    // env-guard discipline as the old opt-in test: skip when the ambient
    // process env pins the flag either way).
    let artifact = artifact(REPORT_PROGRAM);
    if std::env::var_os("LK_AOT_HYBRID").is_none() {
        let mir = lk_aot_lower::lower(&artifact).expect("hybrid is on by default");
        assert_eq!(mir.vm_functions.len(), 1, "the helper bridges by default");
    }
}
