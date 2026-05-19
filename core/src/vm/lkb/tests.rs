use super::*;
use crate::{expr::Expr, vm, vm::Compiler};

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
}
