use super::*;
use crate::{expr::Pattern, stmt::Stmt, vm::bytecode::PatternBinding};
#[test]
fn test_bc32_roundtrip_simple() {
    let f = Function {
        consts: vec![Val::Int(42)],
        code: vec![
            Op::LoadK(0, 0),
            Op::Move(1, 0),
            Op::ToStr(2, 1),
            Op::ToBool(2, 1),
            Op::Jmp(1),
            Op::JmpFalse(2, -1),
        ],
        n_regs: 3,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };
    let bc = Bc32Function::try_from_function(&f).expect("encodable");
    let f2 = bc.decode();
    assert_eq!(format!("{:?}", f.code), format!("{:?}", f2.code));
}

#[test]
fn test_bc32_call_multi_return() {
    let f = Function {
        consts: vec![],
        code: vec![Op::Call {
            f: 0,
            base: 1,
            argc: 2,
            retc: 2,
        }],
        n_regs: 4,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };
    let bc = Bc32Function::try_from_function(&f).expect("encodable multi-ret call");
    assert_eq!(bc.code32.len(), 2, "CallX should occupy two words");
    let decoded = bc.decode();
    let expected = vec![Op::Call {
        f: 0,
        base: 1,
        argc: 2,
        retc: 2,
    }];
    assert_eq!(format!("{:?}", decoded.code), format!("{:?}", expected));
}

#[test]
fn test_bc32_call_out_of_range_regs_uses_callx() {
    let f = Function {
        consts: vec![],
        code: vec![Op::Call {
            f: 300,
            base: 301,
            argc: 2,
            retc: 1,
        }],
        n_regs: 304,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };
    let bc = Bc32Function::try_from_function(&f).expect("reg-ext call encodable");
    assert_eq!(bc.code32.len(), 3);
    let decoded = bc.decode();
    assert_eq!(format!("{:?}", decoded.code), format!("{:?}", f.code));
}

#[test]
fn test_bc32_loadk_out_of_range_dst_uses_reg_ext() {
    let f = Function {
        consts: vec![Val::Int(1)],
        code: vec![Op::LoadK(300, 0)],
        n_regs: 301,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };
    let bc = Bc32Function::try_from_function(&f).expect("LoadK dst reg-ext encodable");
    assert_eq!(bc.code32.len(), 2);
    let decoded = bc.decode();
    assert_eq!(format!("{:?}", decoded.code), format!("{:?}", f.code));
}

#[test]
fn test_bc32_move_out_of_range_regs_use_reg_ext() {
    let f = Function {
        consts: vec![],
        code: vec![Op::Move(300, 299)],
        n_regs: 301,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };
    let bc = Bc32Function::try_from_function(&f).expect("Move reg-ext encodable");
    assert_eq!(bc.code32.len(), 2);
    let decoded = bc.decode();
    assert_eq!(format!("{:?}", decoded.code), format!("{:?}", f.code));
}

#[test]
fn test_bc32_global_ops_out_of_range_regs_use_reg_ext() {
    let f = Function {
        consts: vec![Val::from_str("global_name")],
        code: vec![Op::LoadGlobal(300, 0), Op::DefineGlobal(0, 301)],
        n_regs: 302,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };
    let bc = Bc32Function::try_from_function(&f).expect("global reg-ext encodable");
    assert_eq!(bc.code32.len(), 4);
    let decoded = bc.decode();
    assert_eq!(format!("{:?}", decoded.code), format!("{:?}", f.code));
}

#[test]
fn test_bc32_make_closure_out_of_range_dst_uses_reg_ext() {
    let proto_template = ClosureProto {
        self_name: None,
        params: Arc::new(Vec::new()),
        named_params: Arc::new(Vec::new()),
        default_funcs: Arc::new(Vec::new()),
        func: None,
        body: Arc::new(Stmt::Block { statements: Vec::new() }),
        captures: Arc::new(Vec::new()),
        capture_names: Arc::<[String]>::from(Vec::new()),
        code: crate::vm::closure_code_cell(None),
        empty_env: crate::vm::closure_empty_env(),
        empty_upvalues: crate::vm::closure_empty_upvalues(),
        empty_captures: crate::vm::closure_empty_captures(),
        empty_closure: crate::vm::closure_empty_closure_cell(),
    };
    let f = Function {
        consts: vec![],
        code: vec![Op::MakeClosure { dst: 300, proto: 0 }],
        n_regs: 301,
        protos: vec![proto_template],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };
    let bc = Bc32Function::try_from_function(&f).expect("make closure dst reg-ext encodable");
    assert_eq!(bc.code32.len(), 2);
    let decoded = bc.decode();
    assert_eq!(format!("{:?}", decoded.code), format!("{:?}", f.code));
}

#[test]
fn test_bc32_ret_out_of_range_base_uses_reg_ext() {
    let f = Function {
        consts: vec![],
        code: vec![Op::Ret { base: 300, retc: 1 }],
        n_regs: 301,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };
    let bc = Bc32Function::try_from_function(&f).expect("ret base reg-ext encodable");
    assert_eq!(bc.code32.len(), 2);
    let decoded = bc.decode();
    assert_eq!(format!("{:?}", decoded.code), format!("{:?}", f.code));
}

#[test]
fn test_bc32_local_ops_out_of_range_regs_use_reg_ext() {
    let f = Function {
        consts: vec![],
        code: vec![Op::LoadLocal(300, 301), Op::StoreLocal(301, 300)],
        n_regs: 302,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };
    let bc = Bc32Function::try_from_function(&f).expect("local reg-ext encodable");
    assert_eq!(bc.code32.len(), 4);
    let decoded = bc.decode();
    assert_eq!(format!("{:?}", decoded.code), format!("{:?}", f.code));
}

#[test]
fn test_bc32_arithmetic_and_unary_out_of_range_regs_use_reg_ext() {
    let f = Function {
        consts: vec![Val::Int(7)],
        code: vec![
            Op::Add(300, 301, 302),
            Op::Sub(303, 300, 301),
            Op::Mul(304, 300, 302),
            Op::Div(305, 304, 301),
            Op::Mod(306, 305, 300),
            Op::AddIntImm(307, 306, 1),
            Op::CmpEq(308, 307, 300),
            Op::CmpLtImm(309, 308, 1),
            Op::ToBool(310, 309),
            Op::ToStr(311, 310),
            Op::Not(312, 310),
            Op::Len { dst: 313, src: 311 },
            Op::Index {
                dst: 314,
                base: 300,
                idx: 301,
            },
            Op::Access(315, 300, 301),
            Op::AccessK(316, 300, 0),
            Op::IndexK(317, 300, 0),
        ],
        n_regs: 318,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };
    let bc = Bc32Function::try_from_function(&f).expect("arithmetic reg-ext encodable");
    assert_eq!(bc.code32.len(), 32);
    let decoded = bc.decode();
    assert_eq!(format!("{:?}", decoded.code), format!("{:?}", f.code));
}

#[test]
fn test_bc32_conditional_jumps_out_of_range_regs_use_reg_ext() {
    let f = Function {
        consts: vec![],
        code: vec![Op::JmpFalse(300, 2), Op::JmpIfNil(301, 1), Op::JmpIfNotNil(302, -2)],
        n_regs: 303,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };
    let bc = Bc32Function::try_from_function(&f).expect("conditional jump reg-ext encodable");
    assert_eq!(bc.code32.len(), 6);
    let decoded = bc.decode();
    let expected = vec![Op::JmpFalse(300, 4), Op::JmpIfNil(301, 2), Op::JmpIfNotNil(302, -4)];
    assert_eq!(format!("{:?}", decoded.code), format!("{:?}", expected));
}

#[test]
fn test_bc32_fused_jumps_out_of_range_regs_use_reg_ext() {
    let f = Function {
        consts: vec![],
        code: vec![
            Op::CmpLtImmJmp { r: 300, imm: 7, ofs: 2 },
            Op::CmpLeImmJmp { r: 301, imm: 8, ofs: 1 },
            Op::AddIntImmJmp {
                r: 302,
                imm: 1,
                ofs: -2,
            },
        ],
        n_regs: 303,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };
    let bc = Bc32Function::try_from_function(&f).expect("fused jump reg-ext encodable");
    assert_eq!(bc.code32.len(), 9);
    let decoded = bc.decode();
    let expected = vec![
        Op::CmpLtImmJmp { r: 300, imm: 7, ofs: 6 },
        Op::CmpLeImmJmp { r: 301, imm: 8, ofs: 3 },
        Op::AddIntImmJmp {
            r: 302,
            imm: 1,
            ofs: -6,
        },
    ];
    assert_eq!(format!("{:?}", decoded.code), format!("{:?}", expected));
}

#[test]
fn test_bc32_call_named_out_of_range_uses_extended_path() {
    let f = Function {
        consts: vec![],
        code: vec![Op::CallNamed {
            f: 300,
            base_pos: 301,
            posc: 2,
            base_named: 512,
            namedc: 3,
            retc: 2,
        }],
        n_regs: 600,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };
    let bc = Bc32Function::try_from_function(&f).expect("extended call encodable");
    assert_eq!(bc.code32.len(), 3);
    let decoded = bc.decode();
    assert_eq!(format!("{:?}", decoded.code), format!("{:?}", f.code));
}

#[test]
fn test_bc32_extended_string_intrinsics() {
    let f = Function {
        consts: vec![Val::from_str("pro"), Val::from_str("needle")],
        code: vec![
            Op::Floor { dst: 300, src: 301 },
            Op::StartsWithK(302, 303, 0),
            Op::ContainsK(304, 305, 1),
            Op::ToIter { dst: 306, src: 307 },
        ],
        n_regs: 308,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };
    let bc = Bc32Function::try_from_function(&f).expect("extended intrinsic ops encodable");
    assert_eq!(bc.code32.len(), 8);
    let decoded = bc.decode();
    assert_eq!(format!("{:?}", decoded.code), format!("{:?}", f.code));
    let decoded_table = Bc32Decoded::from_words(&bc.code32).expect("decoded table");
    assert_eq!(decoded_table.instrs.len(), 4);
}

#[test]
fn test_bc32_pattern_ops() {
    let f = Function {
        consts: vec![Val::Str("fail".into())],
        code: vec![
            Op::PatternMatch {
                dst: 0,
                src: 1,
                plan: 0,
            },
            Op::PatternMatchOrFail {
                src: 1,
                plan: 0,
                err_kidx: 0,
                is_const: false,
            },
            Op::PatternMatchOrFail {
                src: 1,
                plan: 0,
                err_kidx: 0,
                is_const: true,
            },
        ],
        n_regs: 4,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: vec![PatternPlan {
            pattern: Pattern::Variable("x".into()),
            bindings: vec![PatternBinding {
                name: "x".into(),
                reg: 2,
            }],
        }],
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };
    let bc = Bc32Function::try_from_function(&f).expect("pattern ops encodable");
    assert_eq!(bc.code32.len(), 3);

    let decoded = bc.decode();
    assert_eq!(decoded.pattern_plans.len(), 1);
    assert!(matches!(
        decoded.pattern_plans[0].pattern,
        Pattern::Variable(ref name) if name == "x"
    ));
    assert!(matches!(
        decoded.code[0],
        Op::PatternMatch {
            dst: 0,
            src: 1,
            plan: 0
        }
    ));
    assert!(matches!(
        decoded.code[1],
        Op::PatternMatchOrFail { is_const: false, .. }
    ));
    assert!(matches!(decoded.code[2], Op::PatternMatchOrFail { is_const: true, .. }));
}

#[test]
fn test_bc32_build_list_map() {
    let f = Function {
        consts: vec![],
        code: vec![
            Op::BuildList {
                dst: 0,
                base: 1,
                len: 6,
            },
            Op::BuildMap {
                dst: 2,
                base: 8,
                len: 4,
            },
        ],
        n_regs: 16,
        protos: Vec::new(),
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };
    let bc = Bc32Function::try_from_function(&f).expect("build ops encodable");
    assert_eq!(bc.code32.len(), 2);

    let decoded = bc.decode();
    assert_eq!(format!("{:?}", decoded.code), format!("{:?}", f.code));
    assert_eq!(decoded.n_regs, 16);
}

#[test]
fn test_bc32_collection_ops_out_of_range_regs_use_reg_ext() {
    let f = Function {
        consts: vec![],
        code: vec![
            Op::BuildList {
                dst: 300,
                base: 301,
                len: 2,
            },
            Op::BuildMap {
                dst: 302,
                base: 303,
                len: 2,
            },
            Op::ListSlice {
                dst: 304,
                src: 300,
                start: 301,
            },
            Op::ListPush { list: 300, val: 301 },
            Op::MapSet {
                map: 302,
                key: 303,
                val: 304,
            },
            Op::MapSetMove {
                map: 302,
                key: 303,
                val: 304,
            },
        ],
        n_regs: 305,
        protos: Vec::new(),
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };
    let bc = Bc32Function::try_from_function(&f).expect("collection reg-ext encodable");
    assert_eq!(bc.code32.len(), 12);

    let decoded = bc.decode();
    assert_eq!(format!("{:?}", decoded.code), format!("{:?}", f.code));
}

#[test]
fn test_bc32_build_list_map_out_of_range_falls_back() {
    let f = Function {
        consts: vec![],
        code: vec![
            Op::BuildList {
                dst: 3,
                base: 20,
                len: 300,
            },
            Op::MakeClosure { dst: 4, proto: 300 },
        ],
        n_regs: 24,
        protos: {
            let proto_template = ClosureProto {
                self_name: None,
                params: Arc::new(Vec::new()),
                named_params: Arc::new(Vec::new()),
                default_funcs: Arc::new(Vec::new()),
                func: None,
                body: Arc::new(Stmt::Block { statements: Vec::new() }),
                captures: Arc::new(Vec::new()),
                capture_names: Arc::<[String]>::from(Vec::new()),
                code: crate::vm::closure_code_cell(None),
                empty_env: crate::vm::closure_empty_env(),
                empty_upvalues: crate::vm::closure_empty_upvalues(),
                empty_captures: crate::vm::closure_empty_captures(),
                empty_closure: crate::vm::closure_empty_closure_cell(),
            };
            vec![proto_template; 301]
        },
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };
    assert!(Bc32Function::try_from_function(&f).is_none());
}
