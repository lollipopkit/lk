use lk_core::vm::{Compiler32, disassemble_module32};

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
    let module = Compiler32::compile_source_module(script).unwrap();
    println!("{}", disassemble_module32(&module));
}
