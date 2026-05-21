use super::*;

#[test]
fn lowers_cmove_int_to_select() {
    let func = Function {
        consts: vec![Val::Int(10), Val::Int(7)],
        code: vec![
            Op::LoadK(0, 0),
            Op::LoadK(1, 1),
            Op::CMoveInt {
                dst: 0,
                src: 1,
                a: 1,
                b: 0,
                kind: IntCmpKind::Lt,
            },
            Op::Ret { base: 0, retc: 1 },
        ],
        n_regs: 2,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let options = LlvmBackendOptions {
        run_optimizations: false,
        ..LlvmBackendOptions::default()
    };
    let artifact = compile_function_to_llvm(&func, "cmove_int", options).expect("LLVM backend should succeed");
    let ir = artifact.module.ir;
    assert!(
        ir.contains("select i1"),
        "expected CMoveInt to lower to select:\n{}",
        ir
    );
    assert!(
        !ir.contains("call i64 @lk_rt_compare"),
        "CMoveInt should not lower through generic compare helper:\n{}",
        ir
    );
}
