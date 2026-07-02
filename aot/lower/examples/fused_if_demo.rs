// Hand-built `if a <= 5 { return a } return 99` using a fused TestLeIntI + Jmp,
// to smoke-test fused compare-and-branch lowering end-to-end (lower->clang->run).
use lk_core::vm::{ConstPoolData, FunctionData, Instr, MODULE_ARTIFACT_VERSION, ModuleArtifact, ModuleData, Opcode};

fn main() {
    let a: i64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(3);
    let art = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: ModuleData {
            entry: 0,
            globals: Vec::new(),
            functions: vec![FunctionData {
                consts: ConstPoolData {
                    ints: vec![a, 99],
                    floats: vec![],
                    strings: vec![],
                    heap_values: vec![],
                },
                code: vec![
                    Instr::abx(Opcode::LoadInt, 0, 0).raw(),       // 0: r0=a
                    Instr::abc(Opcode::TestLeIntI, 0, 0, 5).raw(), // 1: test r0<=5 (b=0 => jump when false)
                    Instr::sj(Opcode::Jmp, 1).raw(),               // 2: (fused) -> pc4 when !(r0<=5)
                    Instr::abc(Opcode::Return, 0, 1, 0).raw(),     // 3: return r0
                    Instr::abx(Opcode::LoadInt, 1, 1).raw(),       // 4: r1=99
                    Instr::abc(Opcode::Return, 1, 1, 0).raw(),     // 5: return r1
                ],
                performance: Default::default(),
                register_count: 2,
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
