// Builds `if a < b { r3 = a } else { r3 = b }; return r3` (min) bytecode with a
// LIVE if/else merge, lowers it to MIR, and renders LLVM IR. Smoke-tests the
// control-flow + SSA-phi lowering end-to-end (lower -> codegen -> clang -> run)
// with a real merge that constant folding cannot remove.
use lk_core::vm::{ConstPoolData, FunctionData, Instr, MODULE_ARTIFACT_VERSION, ModuleArtifact, ModuleData, Opcode};

fn main() {
    let a: i64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(3);
    let b: i64 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(5);
    let art = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: ModuleData {
            entry: 0,
            globals: Vec::new(),
            functions: vec![FunctionData {
                consts: ConstPoolData {
                    ints: vec![a, b],
                    floats: vec![],
                    strings: vec![],
                    heap_values: vec![],
                },
                code: vec![
                    Instr::abx(Opcode::LoadInt, 0, 0).raw(),     // 0: r0=a
                    Instr::abx(Opcode::LoadInt, 1, 1).raw(),     // 1: r1=b
                    Instr::abc(Opcode::CmpLtInt, 2, 0, 1).raw(), // 2: r2 = a<b
                    Instr::as_bx(Opcode::BrFalse, 2, 2).raw(),   // 3: !(a<b) -> pc 6
                    Instr::abc(Opcode::Move, 3, 0, 0).raw(),     // 4: r3=a
                    Instr::sj(Opcode::Jmp, 1).raw(),             // 5: -> pc 7
                    Instr::abc(Opcode::Move, 3, 1, 0).raw(),     // 6: r3=b
                    Instr::abc(Opcode::Return, 3, 1, 0).raw(),   // 7: return r3 (phi)
                ],
                performance: Default::default(),
                register_count: 4,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
            }],
        },
    };
    let mir = lk_aot_lower::lower(&art).expect("lowers");
    lk_aot_mir::validate(&mir).expect("valid");
    print!("{}", lk_aot_codegen::render_module(&mir));
}
