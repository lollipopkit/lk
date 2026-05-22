use lk_core::vm::{Compiler32, disassemble_module32, execute_source32_to_val};

fn main() {
    // Test: let variable with function call
    let program = r#"
fn g() { return 42 }
let a = g()
return a
"#;
    let module = Compiler32::compile_source_module(program).unwrap();
    println!("{}", disassemble_module32(&module));
    let result = execute_source32_to_val(program).unwrap();
    println!("Result: {:?}", result);
}
