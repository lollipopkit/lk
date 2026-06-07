use lk_core::vm::{Compiler, Module, disassemble_module};
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

fn compile_script(source: &str) -> Module {
    Compiler::compile_source_module(source).unwrap()
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

fn dump_function(name: &str, module: &Module) {
    println!("=== {} ===", name);
    println!("{}", disassemble_module(module));
}
