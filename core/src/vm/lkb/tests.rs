use super::*;
use std::sync::Arc;

use crate::{
    expr::Expr,
    stmt::{Stmt, StmtParser},
    token::Tokenizer,
    val::{Type, Val},
    vm,
    vm::{Compiler, IntCmpKind, Op, compile_program},
};

#[test]
fn round_trip_simple_expr() {
    let expr = Expr::parse_cached_arc("1 + 2 * 3").expect("parse");
    let func = Compiler::new().compile_expr(&expr);
    let module = BytecodeModule::new(func);
    let bytes = encode_module(&module).expect("encode");
    let decoded = decode_module(&bytes).expect("decode");
    assert_eq!(decoded.version, CURRENT_VERSION);
    assert_eq!(decoded.flags.bits(), ModuleFlags::NONE.bits());
    let mut vm = vm::Vm::new();
    let mut ctx = vm::VmContext::new();
    let original = vm.exec(&module.entry, &mut ctx).expect("vm exec");
    let decoded_res = vm.exec(&decoded.entry, &mut ctx).expect("vm exec decoded");
    assert_eq!(original, decoded_res);
}

#[test]
fn round_trip_with_bundled_modules() {
    let expr = Expr::parse_cached_arc("42").expect("parse");
    let func = Compiler::new().compile_expr(&expr);
    let mut child = BytecodeModule::new(func.clone());
    let meta = ModuleMeta {
        source: Some("child.lk".to_string()),
        ..ModuleMeta::default()
    };
    child.meta = Some(meta);

    let mut parent = BytecodeModule::new(func);
    parent.bundled_modules.push(BundledModule {
        path: "child.lk".to_string(),
        module: child,
    });

    let bytes = encode_module(&parent).expect("encode bundled");
    let decoded = decode_module(&bytes).expect("decode bundled");
    assert_eq!(decoded.bundled_modules.len(), 1);
    assert_eq!(decoded.bundled_modules[0].path, "child.lk");
}

#[test]
fn function_analysis_survives_round_trip() {
    let expr = Expr::parse_cached_arc("1 + 2").expect("parse");
    let func = Compiler::new().compile_expr(&expr);
    let original_analysis = func.analysis.clone().expect("compiler should attach FunctionAnalysis");

    let module = BytecodeModule::new(func);
    let bytes = encode_module(&module).expect("encode with analysis");
    let decoded = decode_module(&bytes).expect("decode with analysis");

    let decoded_analysis = decoded
        .entry
        .analysis
        .clone()
        .expect("decoded function should retain analysis");

    assert_eq!(
        decoded_analysis.escape.return_class,
        original_analysis.escape.return_class
    );
    assert_eq!(
        decoded_analysis.escape.escaping_values,
        original_analysis.escape.escaping_values
    );
    assert_eq!(
        decoded_analysis.region_plan.return_region,
        original_analysis.region_plan.return_region
    );
    assert_eq!(
        decoded_analysis.region_plan.values.len(),
        original_analysis.region_plan.values.len()
    );
    assert_eq!(decoded_analysis.perf.values, original_analysis.perf.values);
    assert_eq!(decoded_analysis.perf.registers, original_analysis.perf.registers);
    assert_eq!(decoded_analysis.perf.local_slots, original_analysis.perf.local_slots);
    assert_eq!(decoded_analysis.perf.key_ops, original_analysis.perf.key_ops);
    assert_eq!(decoded_analysis.perf.dead_writes, original_analysis.perf.dead_writes);
    assert_eq!(
        decoded_analysis.perf.register_copies,
        original_analysis.perf.register_copies
    );
    assert_eq!(decoded_analysis.perf.local_copies, original_analysis.perf.local_copies);
    assert_eq!(
        decoded_analysis.perf.container_moves,
        original_analysis.perf.container_moves
    );
    assert_eq!(decoded_analysis.perf.control_flow, original_analysis.perf.control_flow);
}

#[test]
fn performance_fact_queries_survive_round_trip() {
    let source = r#"
        let data = [1, 2, 3];
        return data[1] + 1;
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokenize");
    let mut parser = StmtParser::new(&tokens);
    let program = parser.parse_program().expect("parse program");
    let function = compile_program(&program);
    let original_list_reg = function
        .code
        .iter()
        .find_map(|op| match op {
            Op::BuildList { dst, .. } => Some(*dst),
            _ => None,
        })
        .expect("list register");

    let module = BytecodeModule::new(function);
    let bytes = encode_module(&module).expect("encode with performance facts");
    let decoded = decode_module(&bytes).expect("decode with performance facts");
    let decoded_perf = &decoded.entry.analysis.as_ref().expect("decoded analysis").perf;

    assert_eq!(decoded_perf.value_kind(original_list_reg), vm::PerfValueKind::List);
    assert_eq!(
        decoded_perf.list_value_kind(original_list_reg),
        Some(vm::PerfValueKind::Int)
    );
    assert_eq!(decoded_perf.list_known_len(original_list_reg), Some(3));
}

#[test]
fn closure_param_types_survive_round_trip() {
    let source = r#"
        fn score(price: Int, qty: Int, discount: Int) -> Int {
            return price * qty - discount;
        }
        return score(7, 6, 5);
    "#;
    let tokens = Tokenizer::tokenize(source).expect("tokenize");
    let mut parser = StmtParser::new(&tokens);
    let program = parser.parse_program().expect("parse program");
    let function = compile_program(&program);

    let original_proto = function.protos.first().expect("closure proto");
    let original_param_types = Arc::clone(&original_proto.param_types);
    assert_eq!(
        original_param_types.as_ref(),
        &vec![Some(Type::Int), Some(Type::Int), Some(Type::Int)]
    );

    let module = BytecodeModule::new(function);
    let bytes = encode_module(&module).expect("encode");
    let decoded = decode_module(&bytes).expect("decode");
    let decoded_proto = decoded.entry.protos.first().expect("decoded closure proto");
    assert_eq!(decoded_proto.param_types, original_param_types);
    let nested = decoded_proto.func.as_ref().expect("decoded nested function");
    assert!(
        nested.code.iter().any(|op| matches!(op, Op::MulInt(_, _, _))),
        "decoded typed param function should retain MulInt lowering in {:?}",
        nested.code
    );
    assert!(
        nested.code.iter().any(|op| matches!(op, Op::SubInt(_, _, _))),
        "decoded typed param function should retain SubInt lowering in {:?}",
        nested.code
    );

    let mut vm = vm::Vm::new();
    let mut ctx = vm::VmContext::new();
    let result = vm.exec(&decoded.entry, &mut ctx).expect("vm exec decoded");
    assert_eq!(result, Val::Int(37));
}

#[test]
fn register_index_opcodes_survive_round_trip() {
    let function = Compiler::new().compile_function_with_param_types_and_captures(
        &["values".to_string(), "text".to_string(), "idx".to_string()],
        &[
            Some(Type::List(Box::new(Type::Int))),
            Some(Type::String),
            Some(Type::Int),
        ],
        &[],
        &Stmt::Block {
            statements: vec![
                Box::new(Stmt::Expr(Box::new(Expr::Access(
                    Box::new(Expr::Var("text".to_string())),
                    Box::new(Expr::Var("idx".to_string())),
                )))),
                Box::new(Stmt::Return {
                    value: Some(Box::new(Expr::Access(
                        Box::new(Expr::Var("values".to_string())),
                        Box::new(Expr::Var("idx".to_string())),
                    ))),
                }),
            ],
        },
        &[],
    );

    assert!(
        function.code.iter().any(|op| matches!(op, Op::ListIndex(_, _, _))),
        "expected ListIndex before LKB roundtrip in {:?}",
        function.code
    );
    assert!(
        function.code.iter().any(|op| matches!(op, Op::StrIndex(_, _, _))),
        "expected StrIndex before LKB roundtrip in {:?}",
        function.code
    );

    let module = BytecodeModule::new(function);
    let bytes = encode_module(&module).expect("encode");
    let decoded = decode_module(&bytes).expect("decode");
    assert!(
        decoded.entry.code.iter().any(|op| matches!(op, Op::ListIndex(_, _, _))),
        "expected ListIndex after LKB roundtrip in {:?}",
        decoded.entry.code
    );
    assert!(
        decoded.entry.code.iter().any(|op| matches!(op, Op::StrIndex(_, _, _))),
        "expected StrIndex after LKB roundtrip in {:?}",
        decoded.entry.code
    );
}

#[test]
fn cmove_int_survives_round_trip() {
    let function = Function {
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
        protos: vec![],
        param_regs: vec![],
        named_param_regs: vec![],
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    };

    let module = BytecodeModule::new(function);
    let bytes = encode_module(&module).expect("encode");
    let decoded = decode_module(&bytes).expect("decode");
    assert!(
        decoded.entry.code.iter().any(|op| matches!(op, Op::CMoveInt { .. })),
        "expected CMoveInt after LKB roundtrip in {:?}",
        decoded.entry.code
    );
}
