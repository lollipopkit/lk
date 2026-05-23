use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Context;
use lk_core::{
    rt,
    stmt::{Program, stmt_parser::StmtParser},
    token::Tokenizer,
    vm::{VmRuntimeMetrics, compile_program32_module_with_ctx, vm_runtime_metrics_reset, vm_runtime_metrics_snapshot},
};

use crate::build_vm_context;

pub(crate) fn run_coverage_report(path: &Path, runtime: bool) -> anyhow::Result<()> {
    let source = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let program = parse_program(&source)?;
    if runtime {
        let mut ctx = build_vm_context(path)?;
        vm_runtime_metrics_reset();
        let result = program
            .execute32_with_ctx(&mut ctx)
            .with_context(|| format!("execute {} for runtime coverage", path.display()))?;
        rt::shutdown_runtime();
        print_static_coverage(path, &result.module);
        print_runtime_metrics(vm_runtime_metrics_snapshot());
    } else {
        let mut ctx = build_vm_context(path)?;
        let module = compile_program32_module_with_ctx(&program, &mut ctx)
            .with_context(|| format!("compile Instr32 module for {}", path.display()))?;
        print_static_coverage(path, &module);
    }

    Ok(())
}

fn parse_program(source: &str) -> anyhow::Result<Program> {
    let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(source)?;
    let mut parser = StmtParser::new_with_spans(&tokens, &spans);
    Ok(parser.parse_program_with_enhanced_errors(source)?)
}

fn print_static_coverage(path: &Path, module: &lk_core::vm::Module32) {
    let mut opcode_counts = BTreeMap::<String, usize>::new();
    let mut instructions = 0usize;
    let mut registers = 0usize;
    let mut int_consts = 0usize;
    let mut float_consts = 0usize;
    let mut string_consts = 0usize;
    let mut heap_consts = 0usize;

    for function in &module.functions {
        instructions += function.code.len();
        registers += function.register_count as usize;
        int_consts += function.consts.ints.len();
        float_consts += function.consts.floats.len();
        string_consts += function.consts.strings.len();
        heap_consts += function.consts.heap_values.len();
        for instr in &function.code {
            *opcode_counts.entry(format!("{:?}", instr.opcode())).or_default() += 1;
        }
    }

    println!("Instr32 coverage: {}", path.display());
    println!("  functions: {}", module.functions.len());
    println!("  natives: {}", module.natives.len());
    println!("  globals: {}", module.globals.len());
    println!("  instructions: {instructions}");
    println!("  registers: {registers}");
    println!("  consts: int={int_consts} float={float_consts} string={string_consts} heap={heap_consts}");
    println!("  opcodes:");
    for (opcode, count) in opcode_counts {
        println!("    {opcode}: {count}");
    }
}

fn print_runtime_metrics(metrics: VmRuntimeMetrics) {
    println!("Runtime metrics:");
    println!("  opcode_steps: {}", metrics.opcode_steps);
    println!("  copy_policy_heap_clones: {}", metrics.copy_policy_heap_clones);
    println!("  register_copy_heap_clones: {}", metrics.register_copy_heap_clones);
    println!("  local_copy_heap_clones: {}", metrics.local_copy_heap_clones);
    println!("  local_load_heap_clones: {}", metrics.local_load_heap_clones);
    println!("  local_store_heap_clones: {}", metrics.local_store_heap_clones);
    println!("  const_load_heap_clones: {}", metrics.const_load_heap_clones);
    println!("  call_arg_heap_clones: {}", metrics.call_arg_heap_clones);
    println!("  container_copy_heap_clones: {}", metrics.container_copy_heap_clones);
    println!("  register_writes: {}", metrics.register_writes);
    println!("  return_value_moves: {}", metrics.return_value_moves);
    println!("  branch_ops: {}", metrics.branch_ops);
    println!("  typed_branch_ops: {}", metrics.typed_branch_ops);
    println!("  call_ops: {}", metrics.call_ops);
    println!("  native_call_ops: {}", metrics.native_call_ops);
    println!("  closure_call_ops: {}", metrics.closure_call_ops);
    println!("  exact_call_ops: {}", metrics.exact_call_ops);
    println!("  named_call_ops: {}", metrics.named_call_ops);
    println!("  method_call_ops: {}", metrics.method_call_ops);
    println!("  container_ops: {}", metrics.container_ops);
    println!("  list_ops: {}", metrics.list_ops);
    println!("  map_ops: {}", metrics.map_ops);
    println!("  string_ops: {}", metrics.string_ops);
}
