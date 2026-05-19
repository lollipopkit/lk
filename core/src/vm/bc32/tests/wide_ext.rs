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

#[test]
fn cmp_imm_roundtrips_i16_immediate() {
    let function = Function {
        consts: Vec::new(),
        code: vec![
            Op::CmpEqImm(1, 0, -129),
            Op::CmpNeImm(2, 0, 128),
            Op::CmpLtImm(3, 0, 300),
            Op::CmpLeImm(4, 0, 400),
            Op::CmpGtImm(5, 0, 900),
            Op::CmpGeImm(6, 0, 1000),
            Op::Ret { base: 5, retc: 1 },
        ],
        n_regs: 7,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let bc32 = Bc32Function::try_from_function(&function).expect("wide compare immediates should pack");
    let decoded = bc32.decode();

    assert!(matches!(decoded.code.first(), Some(Op::CmpEqImm(1, 0, -129))));
    assert!(matches!(decoded.code.get(1), Some(Op::CmpNeImm(2, 0, 128))));
    assert!(matches!(decoded.code.get(2), Some(Op::CmpLtImm(3, 0, 300))));
    assert!(matches!(decoded.code.get(3), Some(Op::CmpLeImm(4, 0, 400))));
    assert!(matches!(decoded.code.get(4), Some(Op::CmpGtImm(5, 0, 900))));
    assert!(matches!(decoded.code.get(5), Some(Op::CmpGeImm(6, 0, 1000))));
}
