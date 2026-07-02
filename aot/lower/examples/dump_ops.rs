// Compiles an LK source string and dumps the entry function's opcodes, so we can
// see exactly which (possibly fused) branch/arith opcodes a loop lowers to.
use lk_core::stmt::stmt_parser::StmtParser;
use lk_core::token::Tokenizer;
use lk_core::vm::{Compiler, Instr, ModuleArtifact};

fn main() {
    let src = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "let s = 0; let i = 1; while (i <= 5) { s += i; i += 1; } return s;".to_string());
    let tokens = Tokenizer::tokenize(&src).expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler::compile_module(&program).expect("module");
    let art = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    eprintln!("entry={} globals={:?}", art.module.entry, art.module.globals);
    for (fi, f) in art.module.functions.iter().enumerate() {
        eprintln!(
            "--- fn {fi} regs={} params={} pos={} caps={} ---",
            f.register_count, f.param_count, f.positional_param_count, f.capture_count
        );
        for (pc, raw) in f.code.iter().enumerate() {
            let instr = Instr::try_from_raw(*raw).unwrap();
            eprintln!(
                "{pc:3}: {:?}  a={} b={} c={} bx={} sc={} sj={}",
                instr.opcode(),
                instr.a(),
                instr.b(),
                instr.c(),
                instr.bx(),
                instr.sc(),
                instr.sj_arg(),
            );
        }
    }
}
