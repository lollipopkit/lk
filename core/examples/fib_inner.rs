use lk_core::stmt::{Stmt, StmtParser};
use lk_core::token::Tokenizer;
use lk_core::vm::Compiler;

fn main() {
    let script = r#"
fn iterative(n) {
    if (n <= 1) { return n; }
    let a = 0;
    let b = 1;
    for _ in 2..=n {
        let t = a + b;
        a = b;
        b = t;
    }
    return b;
}
"#;
    let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(script).unwrap();
    let mut parser = StmtParser::new_with_spans(&tokens, &spans);
    let parsed = parser.parse_program_with_enhanced_errors(script).unwrap();
    let block = Stmt::Block {
        statements: parsed.statements,
    };
    let fun = Compiler::new().compile_stmt(&block);

    println!("n_regs: {}", fun.n_regs);
    println!("code len: {}", fun.code.len());
    if let Some(code32) = &fun.code32 {
        println!("code32 len: {}", code32.len());
    }
    // Also show the closure's code
    println!("\n=== Main function ===");
    for (i, op) in fun.code.iter().enumerate() {
        println!("  {:4}: {:?}", i, op);
    }
    if let Some(proto) = fun.protos.first() {
        if let Some(inner) = &proto.func {
            println!("\n=== iterative() closure ===");
            println!("n_regs: {}", inner.n_regs);
            println!("code len: {}", inner.code.len());
            for (i, op) in inner.code.iter().enumerate() {
                println!("  {:4}: {:?}", i, op);
            }
        }
    }
}
