// Quick test to check let binding behavior
use lkr_core::vm::{Compiler, Vm, VmContext};
use lkr_core::stmt::{Stmt, StmtParser};
use lkr_core::token::Tokenizer;
use lkr_core::val::Val;

fn main() {
    // Test: let variable with function call
    let program = r#"
fn g() { return 42 }
let a = g()
return a
"#;
    let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(program).unwrap();
    let mut parser = StmtParser::new_with_spans(&tokens, &spans);
    let parsed = parser.parse_program_with_enhanced_errors(program).unwrap();
    let block = Stmt::Block { statements: parsed.statements };
    let fun = Compiler::new().compile_stmt(&block);
    
    println!("n_regs: {}", fun.n_regs);
    for (i, op) in fun.code.iter().enumerate() {
        println!("  {:4}: {:?}", i, op);
    }
    
    let mut vm = Vm::new();
    let mut ctx = VmContext::new();
    let result = vm.exec_with(&fun, &mut ctx, None).unwrap();
    println!("Result: {:?}", result);
}
