use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Context;
use lk_core::{
    stmt::Program,
    syntax::{ParseOptions, parse_program_source},
    vm::{
        Opcode, VM_INDEX_KEY_METRIC_NAMES, VM_REGISTER_WRITE_SOURCE_NAMES, VmRuntimeMetrics,
        compile_program_module_with_ctx, vm_runtime_metrics_enabled, vm_runtime_metrics_reset,
        vm_runtime_metrics_snapshot,
    },
};

use crate::build_vm_context;

pub(crate) fn run_coverage_report(path: &Path, disassemble: bool, runtime: bool) -> anyhow::Result<()> {
    let source = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let program = parse_program(&source)?;
    if runtime {
        let mut ctx = build_vm_context(path)?;
        vm_runtime_metrics_reset();
        let result = program
            .execute_with_ctx(&mut ctx)
            .with_context(|| format!("execute {} for runtime coverage", path.display()))?;
        ctx.shutdown_async_runtime();
        print_static_coverage(path, &result.module);
        if disassemble {
            println!("{}", lk_core::vm::disassemble_module(&result.module));
        }
        if vm_runtime_metrics_enabled() {
            print_runtime_metrics(vm_runtime_metrics_snapshot());
        } else {
            print_runtime_metrics_disabled();
        }
    } else {
        let mut ctx = build_vm_context(path)?;
        let module = compile_program_module_with_ctx(&program, &mut ctx)
            .with_context(|| format!("compile Instr module for {}", path.display()))?;
        print_static_coverage(path, &module);
        if disassemble {
            println!("{}", lk_core::vm::disassemble_module(&module));
        }
    }

    Ok(())
}

fn parse_program(source: &str) -> anyhow::Result<Program> {
    Ok(parse_program_source(source, ParseOptions::default())?)
}

fn print_static_coverage(path: &Path, module: &lk_core::vm::Module) {
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

    println!("Instr coverage: {}", path.display());
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
    print_register_write_sources(&metrics);
    print_index_key_metrics(&metrics);
    print_dynamic_opcode_histogram(&metrics);
}

fn print_runtime_metrics_disabled() {
    println!("Runtime metrics: disabled; rebuild with `--features vm-profile` to collect counters");
}

fn print_dynamic_opcode_histogram(metrics: &VmRuntimeMetrics) {
    println!("  dynamic_opcodes:");
    for bits in 0..Opcode::COUNT {
        let count = metrics.opcode_histogram[bits as usize];
        if count == 0 {
            continue;
        }
        let opcode = Opcode::from_bits(bits).expect("valid opcode histogram slot");
        println!("    {opcode:?}: {count}");
    }
}

fn print_register_write_sources(metrics: &VmRuntimeMetrics) {
    println!("  register_write_sources:");
    for (name, count) in VM_REGISTER_WRITE_SOURCE_NAMES
        .iter()
        .zip(metrics.register_write_sources.iter())
    {
        if *count != 0 {
            println!("    {name}: {count}");
        }
    }
}

fn print_index_key_metrics(metrics: &VmRuntimeMetrics) {
    println!("  index_key_metrics:");
    for (name, count) in VM_INDEX_KEY_METRIC_NAMES.iter().zip(metrics.index_key_metrics.iter()) {
        if *count != 0 {
            println!("    {name}: {count}");
        }
    }
}
