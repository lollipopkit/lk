use super::*;

#[test]
fn ext_op_roundtrips_wide_third_register() {
    let function = Function {
        consts: Vec::new(),
        code: vec![Op::StrConcatKnownCap(1, 2, 300), Op::Ret { base: 1, retc: 1 }],
        n_regs: 301,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let bc32 = Bc32Function::try_from_function(&function).expect("wide third ext operand should pack");
    let decoded = bc32.decode();

    assert!(
        matches!(decoded.code.first(), Some(Op::StrConcatKnownCap(1, 2, 300))),
        "expected wide third operand to survive BC32 roundtrip: {:?}",
        decoded.code
    );
}
