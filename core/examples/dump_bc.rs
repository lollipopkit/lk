use lkr_core::stmt::{Stmt, StmtParser};
use lkr_core::token::Tokenizer;
use lkr_core::vm::Compiler;

const FIB_SCRIPT: &str = include_str!("../../examples/fib.lkr");

const REPL_SEQUENCE_SCRIPT: &str = r#"
let total = 0;
let i = 0;
while (i < 100) {
    total = total + i;
    i = i + 1;
}
return total;
"#;

const NUMERIC_REDUCTION_SCRIPT: &str = r#"
let total = 0;
let i = 0;
while (i < 200) {
    let step = i + 1;
    total = total + step * (step + 1);
    i = i + 1;
}
return total;
"#;

fn compile_script(source: &str) -> lkr_core::vm::Function {
    let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(source).unwrap();
    let mut parser = StmtParser::new_with_spans(&tokens, &spans);
    let program = parser.parse_program_with_enhanced_errors(source).unwrap();
    let block = Stmt::Block {
        statements: program.statements,
    };
    Compiler::new().compile_stmt(&block)
}

fn main() {
    for (name, script) in [
        ("script_fib", FIB_SCRIPT.to_string() + "\nreturn iterative(30);\n"),
        ("repl_sequence", REPL_SEQUENCE_SCRIPT.to_string()),
        ("numeric_reduction", NUMERIC_REDUCTION_SCRIPT.to_string()),
    ] {
        println!("=== {} ===", name);
        let func = compile_script(&script);
        println!("n_regs: {}", func.n_regs);
        println!("code len: {}", func.code.len());
        if let Some(code32) = &func.code32 {
            println!("code32 len: {}", code32.len());
        }
        for (i, op) in func.code.iter().enumerate() {
            println!("  {:4}: {:?}", i, op);
        }
        println!();
    }
}
