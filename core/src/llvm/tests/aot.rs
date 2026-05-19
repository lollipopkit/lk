use crate::{
    expr::{Expr, Pattern},
    op::BinOp,
    stmt::{Program, Stmt},
    val::Val,
};

use super::{LlvmBackendOptions, compile_program_to_llvm};

#[test]
fn lowers_zero_capture_function_closure_to_aot_function() {
    let program = Program::new(vec![Box::new(Stmt::Function {
        name: "inc".to_string(),
        params: vec!["x".to_string()],
        param_types: vec![None],
        named_params: Vec::new(),
        return_type: None,
        body: Box::new(Stmt::Block {
            statements: vec![
                Box::new(Stmt::Let {
                    pattern: Pattern::Variable("y".to_string()),
                    type_annotation: None,
                    value: Box::new(Expr::Bin(
                        Box::new(Expr::Var("x".to_string())),
                        BinOp::Add,
                        Box::new(Expr::Val(Val::Int(1))),
                    )),
                    span: None,
                    is_const: false,
                }),
                Box::new(Stmt::Return {
                    value: Some(Box::new(Expr::Var("y".to_string()))),
                }),
            ],
        }),
    })])
    .expect("program");

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_program_to_llvm(&program, options).expect("LLVM backend should lower closure");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("call i64 @lk_rt_make_aot_function"),
        "expected MakeClosure to lower to AOT function helper:\n{}",
        ir
    );
    assert!(
        ir.contains("define i64 @lk_entry_proto_0"),
        "expected nested closure proto to be emitted as native function:\n{}",
        ir
    );
    assert_eq!(
        ir.matches("call void @lk_rt_define_global").count(),
        1,
        "nested closure locals should not write globals:\n{}",
        ir
    );
}
