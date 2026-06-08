use lk_core::vm::{Compiler, disassemble_module};

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
    let module = Compiler::compile_source_module(script).unwrap();
    println!("{}", disassemble_module(&module));
}
