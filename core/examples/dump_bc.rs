use lk_core::stmt::{Stmt, StmtParser};
use lk_core::token::Tokenizer;
use lk_core::vm::Compiler;
use std::path::PathBuf;

const FIB_SCRIPT: &str = r#"
fn iterative(n) {
    let a = 0;
    let b = 1;
    let i = 0;
    while (i < n) {
        let next = a + b;
        a = b;
        b = next;
        i = i + 1;
    }
    return a;
}
"#;

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

fn compile_script(source: &str) -> lk_core::vm::Function {
    let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(source).unwrap();
    let mut parser = StmtParser::new_with_spans(&tokens, &spans);
    let program = parser.parse_program_with_enhanced_errors(source).unwrap();
    let block = Stmt::Block {
        statements: program.statements,
    };
    Compiler::new().compile_stmt(&block)
}

fn main() {
    if let Some(path) = std::env::args_os().nth(1) {
        let path = PathBuf::from(path);
        let script =
            std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        dump_function(&path.display().to_string(), &compile_script(&script));
        return;
    }

    for (name, script) in [
        ("script_fib", FIB_SCRIPT.to_string() + "\nreturn iterative(30);\n"),
        ("repl_sequence", REPL_SEQUENCE_SCRIPT.to_string()),
        ("numeric_reduction", NUMERIC_REDUCTION_SCRIPT.to_string()),
    ] {
        let func = compile_script(&script);
        dump_function(name, &func);
    }
}

fn dump_function(name: &str, func: &lk_core::vm::Function) {
    println!("=== {} ===", name);
    println!("n_regs: {}", func.n_regs);
    println!("code len: {}", func.code.len());
    if let Some(code32) = &func.code32 {
        println!("code32 len: {}", code32.len());
    }
    for (i, op) in func.code.iter().enumerate() {
        println!("  {:4}: {:?}", i, op);
    }
    for (idx, proto) in func.protos.iter().enumerate() {
        if let Some(child) = proto.func.as_ref() {
            let child_name = proto.self_name.as_deref().map_or_else(
                || format!("{name}.closure[{idx}]"),
                |self_name| format!("{name}.{self_name}"),
            );
            dump_function(&child_name, child);
        }
    }
    println!();
}
