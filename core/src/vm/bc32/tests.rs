use super::*;
use crate::{
    expr::Pattern,
    stmt::Stmt,
    util::fast_map::fast_hash_map_with_capacity,
    val::NativeArgs,
    vm::IntCmpKind,
    vm::bytecode::{PatternBinding, rk_make_const},
};

mod call_ops;
mod wide_ext;

fn native_add_one(args: NativeArgs<'_>, _ctx: &mut crate::vm::VmContext) -> anyhow::Result<Val> {
    match args.get(0) {
        Some(Val::Int(value)) => Ok(Val::Int(value + 1)),
        other => anyhow::bail!("expected int argument, got {:?}", other),
    }
}

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
    let expected = vec![
        Op::LoadK(0, 0),
        Op::Move(1, 0),
        Op::ToStr(2, 1),
        Op::ToBool(2, 1),
        Op::Jmp(1),
        Op::BoolBranch(2, -1),
    ];
    assert_eq!(format!("{:?}", f2.code), format!("{:?}", expected));
}

#[test]
fn test_bc32_typed_arith_with_rk_const_stays_packable() {
    let rhs = rk_make_const(0);
    let f = Function {
        consts: vec![Val::Int(7)],
        code: vec![Op::ModInt(2, 1, rhs), Op::Ret { base: 2, retc: 1 }],
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

    let bc = Bc32Function::try_from_function(&f).expect("typed arithmetic with RK const remains packable");
    let decoded = bc.decode();
    assert!(
        matches!(decoded.code.first(), Some(Op::Mod(2, 1, value)) if *value == rhs),
        "typed RK arithmetic should decode through the generic RK op: {:?}",
        decoded.code
    );
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
        param_types: Arc::new(Vec::new()),
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
fn test_bc32_current_typed_ops_roundtrip_gate() {
    let f = Function {
        consts: vec![Val::Int(7), Val::from_str("field"), Val::from_str("needle")],
        code: vec![
            Op::AddInt(0, 1, 2),
            Op::AddFloat(1, 2, 3),
            Op::AddIntImm(2, 3, -4),
            Op::SubInt(3, 4, 5),
            Op::SubFloat(4, 5, 6),
            Op::MulInt(5, 6, 7),
            Op::MulFloat(6, 7, 8),
            Op::DivFloat(7, 8, 9),
            Op::ModInt(8, 9, 10),
            Op::ModFloat(9, 10, 11),
            Op::StrConcatKnownCap(9, 1, 2),
            Op::StrConcatToStr(9, 1, 2),
            Op::CmpEqImm(10, 11, 1),
            Op::CmpNeImm(11, 12, 2),
            Op::CmpLtImm(12, 13, 3),
            Op::CmpLeImm(13, 14, 4),
            Op::CmpGtImm(14, 15, 5),
            Op::CmpGeImm(15, 16, 6),
            Op::CmpI {
                dst: 16,
                a: 15,
                b: 14,
                kind: IntCmpKind::Ge,
            },
            Op::Floor { dst: 16, src: 17 },
            Op::ListLen { dst: 17, src: 18 },
            Op::MapLen { dst: 18, src: 19 },
            Op::StrLen { dst: 19, src: 20 },
            Op::StartsWithK(20, 21, 1),
            Op::ContainsK(21, 22, 2),
            Op::IndexK(22, 23, 0),
            Op::ListIndexI(23, 24, 2),
            Op::ListSetI {
                dst: 23,
                list: 24,
                index: 2,
                val: 25,
            },
            Op::StrIndexI(24, 25, 3),
            Op::AccessK(25, 26, 1),
            Op::MapGetInterned(26, 27, 1),
            Op::MapSetInterned(27, 1, 28),
            Op::MapGetDynamic(28, 27, 29),
            Op::MapHasK(27, 28, 1),
            Op::ListPush { list: 28, val: 29 },
            Op::MapSet {
                map: 29,
                key: 30,
                val: 31,
            },
            Op::MapSetMove {
                map: 29,
                key: 30,
                val: 31,
            },
            Op::CallNativeFast {
                f: 30,
                base: 31,
                argc: 1,
                retc: 1,
            },
            Op::CallMethod0 {
                dst: 30,
                receiver: 31,
                method: 1,
            },
            Op::CallGlobalMethod0 {
                dst: 30,
                receiver: 0,
                method: 1,
            },
            Op::CallExact {
                f: 30,
                base: 31,
                argc: 1,
                retc: 1,
            },
            Op::CallClosureExact {
                f: 30,
                base: 31,
                argc: 1,
                retc: 1,
            },
            Op::CallNamedFallback {
                f: 30,
                base_pos: 31,
                posc: 1,
                base_named: 32,
                namedc: 1,
                retc: 1,
            },
            Op::CmpLtImmJmp { r: 32, imm: 7, ofs: 2 },
            Op::CmpLeImmJmp { r: 33, imm: 8, ofs: 1 },
            Op::AddIntImmJmp { r: 34, imm: 1, ofs: -2 },
            Op::ForRangePrep {
                idx: 35,
                limit: 36,
                step: 37,
                inclusive: false,
                explicit: true,
            },
            Op::RangeLoopI {
                idx: 35,
                limit: 36,
                step: 37,
                inclusive: false,
                write_idx: true,
                ofs: 1,
            },
            Op::ForRangeStep {
                idx: 35,
                step: 37,
                back_ofs: -1,
            },
        ],
        n_regs: 38,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let expected_gate_names: Vec<_> = f
        .code
        .iter()
        .map(|op| {
            op.bc32_typed_gate_name()
                .expect("sample must be part of the BC32 typed gate")
        })
        .collect();
    let bc = Bc32Function::try_from_function(&f).expect("current typed op family must remain BC32 encodable");
    let decoded = bc.decode();
    let decoded_gate_names: Vec<_> = decoded
        .code
        .iter()
        .map(|op| {
            op.bc32_typed_gate_name()
                .expect("decoded op must stay in the BC32 typed gate")
        })
        .collect();
    assert_eq!(decoded_gate_names, expected_gate_names);
    assert_eq!(expected_gate_names.len(), f.code.len());
    assert!(
        decoded.code.iter().any(|op| matches!(op, Op::MapHasK(27, 28, 1))),
        "MapHasK must roundtrip through BC32 extension encoding"
    );
    assert!(
        decoded.code.iter().any(|op| matches!(op, Op::RangeLoopI { .. })),
        "typed range loop must remain represented in BC32"
    );
}

#[test]
fn test_bc32_call_method0_packed_execution() {
    let mut map = fast_hash_map_with_capacity(1);
    map.insert("answer".into(), Val::Int(42));
    let out = exec_packed_function(Function {
        consts: vec![Val::Map(Arc::new(map)), Val::from_str("answer")],
        code: vec![
            Op::LoadK(0, 0),
            Op::CallMethod0 {
                dst: 1,
                receiver: 0,
                method: 1,
            },
            Op::Ret { base: 1, retc: 1 },
        ],
        n_regs: 2,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    });
    assert_eq!(out, Val::Int(42));
}

#[test]
fn test_bc32_call_global_method0_packed_execution() {
    let mut map = fast_hash_map_with_capacity(1);
    map.insert("answer".into(), Val::Int(42));
    let mut ctx = crate::vm::VmContext::new_without_core_vm_builtins();
    ctx.set("module".to_string(), Val::Map(Arc::new(map)));
    let mut f = Function {
        consts: vec![Val::from_str("module"), Val::from_str("answer")],
        code: vec![
            Op::CallGlobalMethod0 {
                dst: 0,
                receiver: 0,
                method: 1,
            },
            Op::Ret { base: 0, retc: 1 },
        ],
        n_regs: 1,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };
    let bc = Bc32Function::try_from_function(&f).expect("CallGlobalMethod0 packed encoding");
    f.code32 = Some(bc.code32);
    f.bc32_decoded = bc.decoded;

    let mut vm = crate::vm::Vm::new();
    let out = vm.exec(&f, &mut ctx).expect("packed CallGlobalMethod0 execution");
    assert_eq!(out, Val::Int(42));
}

#[test]
fn test_bc32_map_has_k_packed_execution() {
    let mut map = fast_hash_map_with_capacity(1);
    map.insert("needle".into(), Val::Int(1));
    let mut f = Function {
        consts: vec![Val::Map(Arc::new(map)), Val::from_str("needle")],
        code: vec![Op::LoadK(0, 0), Op::MapHasK(1, 0, 1), Op::Ret { base: 1, retc: 1 }],
        n_regs: 2,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };
    let bc = Bc32Function::try_from_function(&f).expect("MapHasK packed encoding");
    f.code32 = Some(bc.code32);
    f.bc32_decoded = bc.decoded;

    let mut vm = crate::vm::Vm::new();
    let mut ctx = crate::vm::VmContext::new_without_core_vm_builtins();
    let out = vm.exec(&f, &mut ctx).expect("packed MapHasK execution");
    assert!(matches!(out, Val::Bool(true)));
}

pub(super) fn exec_packed_function(mut f: Function) -> Val {
    let bc = Bc32Function::try_from_function(&f).expect("typed op packed encoding");
    f.code32 = Some(bc.code32);
    f.bc32_decoded = bc.decoded;

    let mut vm = crate::vm::Vm::new();
    let mut ctx = crate::vm::VmContext::new_without_core_vm_builtins();
    vm.exec(&f, &mut ctx).expect("typed op packed execution")
}

#[test]
fn test_bc32_typed_numeric_packed_execution() {
    let int_out = exec_packed_function(Function {
        consts: vec![Val::Int(6), Val::Int(3)],
        code: vec![
            Op::LoadK(0, 0),
            Op::LoadK(1, 1),
            Op::AddInt(2, 0, 1),
            Op::SubInt(3, 2, 1),
            Op::MulInt(4, 3, 1),
            Op::ModInt(5, 4, 0),
            Op::AddIntImm(6, 5, 3),
            Op::CmpEqImm(7, 6, 3),
            Op::CmpNeImm(8, 6, 4),
            Op::CmpLtImm(9, 6, 4),
            Op::CmpLeImm(10, 6, 3),
            Op::CmpGtImm(11, 6, 2),
            Op::CmpGeImm(12, 6, 3),
            Op::CmpI {
                dst: 13,
                a: 6,
                b: 5,
                kind: IntCmpKind::Ge,
            },
            Op::Ret { base: 13, retc: 1 },
        ],
        n_regs: 14,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    });
    assert!(matches!(int_out, Val::Bool(true)));

    let float_out = exec_packed_function(Function {
        consts: vec![Val::Float(6.0), Val::Float(3.0)],
        code: vec![
            Op::LoadK(0, 0),
            Op::LoadK(1, 1),
            Op::AddFloat(2, 0, 1),
            Op::SubFloat(3, 2, 1),
            Op::MulFloat(4, 3, 1),
            Op::DivFloat(5, 4, 0),
            Op::ModFloat(6, 5, 1),
            Op::Floor { dst: 7, src: 6 },
            Op::Ret { base: 7, retc: 1 },
        ],
        n_regs: 8,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    });
    assert!(matches!(float_out, Val::Int(0)));
}

#[test]
fn test_bc32_typed_access_string_container_packed_execution() {
    let mut map = fast_hash_map_with_capacity(1);
    map.insert("field".into(), Val::Int(41));

    let access_out = exec_packed_function(Function {
        consts: vec![
            Val::Map(Arc::new(map)),
            Val::from_str("field"),
            Val::List(Arc::new(vec![Val::Int(9)])),
            Val::Int(0),
            Val::from_str("abcdef"),
            Val::from_str("abc"),
            Val::from_str("cd"),
            Val::from_str("field"),
        ],
        code: vec![
            Op::LoadK(0, 0),
            Op::MapGetInterned(1, 0, 7),
            Op::LoadK(10, 7),
            Op::MapGetDynamic(11, 0, 10),
            Op::LoadK(2, 2),
            Op::IndexK(3, 2, 3),
            Op::ListIndexI(7, 2, 0),
            Op::LoadK(4, 4),
            Op::LoadK(12, 5),
            Op::StrConcatKnownCap(13, 4, 12),
            Op::StrConcatToStr(13, 13, 1),
            Op::StartsWithK(5, 4, 5),
            Op::ContainsK(6, 4, 6),
            Op::StrIndexI(8, 4, 2),
            Op::StrLen { dst: 9, src: 4 },
            Op::Ret { base: 13, retc: 1 },
        ],
        n_regs: 14,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    });
    assert_eq!(access_out, Val::from_str("abcdefabc41"));

    let collection_out = exec_packed_function(Function {
        consts: vec![
            Val::List(Arc::new(Vec::new())),
            Val::Int(7),
            Val::Map(Arc::new(fast_hash_map_with_capacity(0))),
            Val::from_str("k"),
            Val::Int(8),
            Val::from_str("m"),
            Val::Int(9),
        ],
        code: vec![
            Op::LoadK(0, 0),
            Op::LoadK(1, 1),
            Op::ListPush { list: 0, val: 1 },
            Op::ListSetI {
                dst: 10,
                list: 0,
                index: 0,
                val: 1,
            },
            Op::LoadK(2, 2),
            Op::LoadK(3, 3),
            Op::LoadK(4, 4),
            Op::MapSet { map: 2, key: 3, val: 4 },
            Op::LoadK(5, 5),
            Op::LoadK(6, 6),
            Op::MapSetMove { map: 2, key: 5, val: 6 },
            Op::MapSetInterned(2, 3, 4),
            Op::MapHasK(7, 2, 5),
            Op::ListLen { dst: 8, src: 0 },
            Op::MapLen { dst: 9, src: 2 },
            Op::Ret { base: 9, retc: 1 },
        ],
        n_regs: 11,
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    });
    assert!(matches!(collection_out, Val::Int(2)));
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
    let expected = vec![Op::BoolBranch(300, 4), Op::JmpIfNil(301, 2), Op::JmpIfNotNil(302, -4)];
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
            Op::ListSetI {
                dst: 304,
                list: 300,
                index: 300,
                val: 301,
            },
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
            Op::MapSetInterned(302, 1, 4),
            Op::MapGetDynamic(304, 302, 4),
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
    assert_eq!(bc.code32.len(), 19);

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
                param_types: Arc::new(Vec::new()),
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
