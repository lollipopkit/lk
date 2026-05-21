use super::*;

use crate::val::Val;
use crate::vm::bytecode::Function;

fn test_function(code: Vec<Op>) -> Function {
    Function {
        consts: Vec::new(),
        code,
        n_regs: 4,
        protos: Vec::new(),
        param_regs: Vec::new(),
        named_param_regs: Vec::new(),
        named_param_layout: Vec::new(),
        pattern_plans: Vec::new(),
        code32: None,
        bc32_decoded: None,
        analysis: None,
    }
}

#[test]
fn vm_coverage_report_counts_opcodes_categories_and_call_sites() {
    let function = test_function(vec![
        Op::LoadK(0, 0),
        Op::AddInt(1, 0, 0),
        Op::Call {
            f: 1,
            base: 0,
            argc: 1,
            retc: 1,
        },
        Op::Ret { base: 0, retc: 1 },
    ]);

    let report = vm_coverage_report(&function);

    assert_eq!(report.totals.functions, 1);
    assert_eq!(report.totals.instructions, 4);
    assert_eq!(report.totals.call_sites, 1);
    assert_eq!(
        report
            .totals
            .category_counts
            .iter()
            .find(|(category, _)| *category == VmOpcodeCategory::Numeric)
            .map(|(_, count)| *count),
        Some(1)
    );
    assert!(
        report
            .totals
            .opcode_counts
            .iter()
            .any(|entry| entry.opcode == "Call" && entry.count == 1)
    );
    assert!(
        report
            .totals
            .bc32_typed_gate_counts
            .iter()
            .any(|entry| entry.opcode == "AddInt" && entry.count == 1)
    );
}

#[test]
fn runtime_metrics_count_immediate_and_heap_val_clones() {
    vm_runtime_metrics_reset();

    let _ = Val::Int(1).clone();
    let _ = Val::from_str("longer-than-short").clone();

    let metrics = vm_runtime_metrics_snapshot();
    assert_eq!(metrics.val_clones, 2);
    assert_eq!(metrics.immediate_val_clones, 1);
    assert_eq!(metrics.heap_val_clones, 1);
}
