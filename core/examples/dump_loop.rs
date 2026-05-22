use lk_core::vm::{Compiler32, disassemble_module32};

fn main() {
    let scripts = [
        (
            "arith",
            "let total = 0; let i = 0; while (i < 10) { let step = i + 1; total = total + step * (step + 1); i = i + 1; }",
        ),
        (
            "call",
            "fn add(a, b) { return a + b; } let r = 0; let i = 0; while (i < 10) { r = add(r, 1); i = i + 1; }",
        ),
        (
            "fib",
            "fn fib(n) { if (n <= 1) { return n; } let a = 0; let b = 1; for _ in 2..=n { let t = a + b; a = b; b = t; } return b; } let r = fib(10);",
        ),
    ];

    for (name, script) in scripts {
        println!("=== {} ===", name);
        let module = Compiler32::compile_source_module(script).unwrap();
        println!("{}", disassemble_module32(&module));
        println!();
    }
}
