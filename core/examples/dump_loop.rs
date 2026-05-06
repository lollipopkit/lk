use lkr_core::vm::Compiler;
use lkr_core::stmt::{StmtParser, Stmt};
use lkr_core::token::Tokenizer;

fn main() {
    let scripts = [
        ("arith", "let total = 0; let i = 0; while (i < 10) { let step = i + 1; total = total + step * (step + 1); i = i + 1; }"),
        ("call", "fn add(a, b) { return a + b; } let r = 0; let i = 0; while (i < 10) { r = add(r, 1); i = i + 1; }"),
        ("fib", "fn fib(n) { if (n <= 1) { return n; } let a = 0; let b = 1; for _ in 2..=n { let t = a + b; a = b; b = t; } return b; } let r = fib(10);"),
    ];

    for (name, script) in scripts {
        println!("=== {} ===", name);
        let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(script).unwrap();
        let mut parser = StmtParser::new_with_spans(&tokens, &spans);
        let program = parser.parse_program_with_enhanced_errors(script).unwrap();
        let block = Stmt::Block { statements: program.statements };
        let func = Compiler::new().compile_stmt(&block);
        println!("n_regs: {}, code_len: {}", func.n_regs, func.code.len());
        for (i, op) in func.code.iter().enumerate() {
            println!("  {:4}: {:?}", i, op);
        }
        println!();
    }
}