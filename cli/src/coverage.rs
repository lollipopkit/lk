use std::path::Path;
use std::sync::Arc;

#[cfg(feature = "llvm")]
use lk_core::llvm::{LlvmBackendOptions, OptLevel, compile_function_to_llvm};
use lk_core::{
    module::ModuleRegistry,
    stmt::{ModuleResolver, execute_imports},
    typ::TypeChecker,
    vm::{
        VmCoverageReport, compile_program, vm_coverage_report, vm_runtime_metrics_reset, vm_runtime_metrics_snapshot,
    },
};

use crate::paths::parse_program_file;
use crate::{bundler, configure_package_resolver, llvm_symbol_fragment, rt};

pub(crate) fn run_coverage_report(path: &Path, runtime: bool) -> anyhow::Result<()> {
    let program = parse_program_file(path)?;
    let func = compile_program(&program);
    println!("Coverage report: {}", path.display());
    #[cfg(feature = "llvm")]
    {
        let module_name = path
            .file_stem()
            .map(|s| llvm_symbol_fragment(s.to_string_lossy().as_ref()))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "lk_module".to_string());
        let options = LlvmBackendOptions {
            module_name,
            target_triple: None,
            run_optimizations: true,
            opt_level: OptLevel::O2,
        };
        match compile_function_to_llvm(&func, "lk_entry", options) {
            Ok(_) => println!("AOT entry: native-lowerable"),
            Err(err) => {
                println!("AOT entry: fallback ({err})");
                for cause in err.chain().skip(1) {
                    println!("  caused by: {cause}");
                }
            }
        }
    }
    #[cfg(not(feature = "llvm"))]
    println!("AOT entry: disabled (cli built without llvm feature)");
    let report = vm_coverage_report(&func);
    print_vm_coverage(&report);
    if runtime {
        match collect_runtime_metrics(path, &program, &func) {
            Ok(()) => print_runtime_metrics(),
            Err(err) => println!("Runtime metrics: skipped after execution error ({err})"),
        }
    } else {
        println!("Runtime metrics: pass --runtime to execute and collect clone/move counters");
    }
    Ok(())
}

fn collect_runtime_metrics(
    path: &Path,
    program: &lk_core::stmt::Program,
    func: &lk_core::vm::Function,
) -> anyhow::Result<()> {
    rt::init_runtime().ok();
    let mut registry = ModuleRegistry::new();
    lk_stdlib::register_stdlib_globals(&mut registry);
    lk_stdlib::register_stdlib_modules(&mut registry)?;
    let mut resolver = ModuleResolver::with_registry(registry);
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        resolver.set_base_dir(parent.to_path_buf());
    }
    configure_package_resolver(&mut resolver, path)?;
    let resolver = Arc::new(resolver);
    let mut ctx = lk_core::vm::VmContext::new()
        .with_resolver(Arc::clone(&resolver))
        .with_type_checker(Some(TypeChecker::new_strict()));
    let imports = bundler::extract_import_statements(program);
    if !imports.is_empty() {
        execute_imports(&imports, resolver.as_ref(), &mut ctx)?;
    }

    vm_runtime_metrics_reset();
    let mut vm = lk_core::vm::Vm::new();
    let result = vm.exec_with(func, &mut ctx, None);
    rt::shutdown_runtime();
    result.map(|_| ())
}

fn print_vm_coverage(report: &VmCoverageReport) {
    let totals = &report.totals;
    println!(
        "VM totals: functions={} packed={}/{} ops={} code32_words={} calls={} named_calls={} closures={} unmaterialized_closures={}",
        totals.functions,
        totals.packed_functions,
        totals.functions,
        totals.instructions,
        totals.code32_words,
        totals.call_sites,
        totals.named_call_sites,
        totals.closure_sites,
        totals.unmaterialized_closures,
    );
    print_category_summary("  categories", &totals.category_counts);
    print_opcode_summary("  top opcodes", &totals.opcode_counts, 12);
    print_opcode_summary("  bc32 typed gate opcodes", &totals.bc32_typed_gate_counts, 16);
    if !totals.bc32_fallback_reasons.is_empty() {
        println!(
            "  bc32 fallback reasons: {}",
            format_pairs(&totals.bc32_fallback_reasons)
        );
    }
    if !totals.bc32_fallback_opcodes.is_empty() {
        println!(
            "  bc32 fallback opcodes: {}",
            format_pairs(&totals.bc32_fallback_opcodes)
        );
    }

    for function in &report.functions {
        print_function_coverage(function);
    }
}

fn print_function_coverage(function: &lk_core::vm::VmFunctionCoverage) {
    let indent = "  ".repeat(function.depth);
    if function.bc32_status.packed {
        println!(
            "{indent}- {}: packed ops={} words={} regs={} consts={} protos={} decoded={}",
            function.name,
            function.bc32_status.ops,
            function.bc32_status.words.unwrap_or(0),
            function.register_count,
            function.const_count,
            function.proto_count,
            function.has_decoded_bc32,
        );
    } else {
        println!(
            "{indent}- {}: unpacked ops={} reason={} opcode={} op_index={} detail={} regs={} consts={} protos={} decoded={}",
            function.name,
            function.bc32_status.ops,
            function.bc32_status.reason.as_deref().unwrap_or("unknown"),
            function.bc32_status.opcode.as_deref().unwrap_or("unknown"),
            function
                .bc32_status
                .op_index
                .map(|idx| idx.to_string())
                .unwrap_or_else(|| "n/a".to_string()),
            function.bc32_status.detail.as_deref().unwrap_or(""),
            function.register_count,
            function.const_count,
            function.proto_count,
            function.has_decoded_bc32,
        );
    }
    println!(
        "{indent}  sites: calls={} named_calls={} closures={} unmaterialized_closures={}",
        function.call_sites, function.named_call_sites, function.closure_sites, function.unmaterialized_closures
    );
    print_category_summary(&format!("{indent}  categories"), &function.category_counts);
    print_opcode_summary(&format!("{indent}  top opcodes"), &function.opcode_counts, 8);
}

fn print_category_summary(label: &str, categories: &[(lk_core::vm::VmOpcodeCategory, usize)]) {
    if !categories.is_empty() {
        let pairs = categories
            .iter()
            .map(|(category, count)| (category.label().to_string(), *count))
            .collect::<Vec<_>>();
        println!("{label}: {}", format_pairs(&pairs));
    }
}

fn print_opcode_summary(label: &str, opcodes: &[lk_core::vm::VmOpcodeCount], limit: usize) {
    if opcodes.is_empty() {
        return;
    }
    let mut sorted = opcodes.to_vec();
    sorted.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.opcode.cmp(b.opcode)));
    let pairs = sorted
        .into_iter()
        .take(limit)
        .map(|entry| (entry.opcode.to_string(), entry.count))
        .collect::<Vec<_>>();
    println!("{label}: {}", format_pairs(&pairs));
}

fn format_pairs(pairs: &[(String, usize)]) -> String {
    pairs
        .iter()
        .map(|(key, count)| format!("{key}={count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_runtime_metrics() {
    let metrics = vm_runtime_metrics_snapshot();
    println!(
        "Runtime metrics: opcode_steps={} branches={} typed_branches={} calls={} native_calls={} closure_calls={} exact_calls={} named_calls={} method_calls={} containers={} list_ops={} map_ops={} string_ops={} bc32_fallbacks={} bc32_build_misses={} bc32_stale_slots={} bc32_stale_misses={} bc32_sentinel_skips={} val_clones={} immediate_clones={} heap_clones={} register_writes={} return_value_moves={} quickening_hits={} quickening_build_attempts={} quickening_build_successes={} quickening_misses={} quickening_deopts={} quickening_sentinel_skips={}",
        metrics.opcode_steps,
        metrics.branch_ops,
        metrics.typed_branch_ops,
        metrics.call_ops,
        metrics.native_call_ops,
        metrics.closure_call_ops,
        metrics.exact_call_ops,
        metrics.named_call_ops,
        metrics.method_call_ops,
        metrics.container_ops,
        metrics.list_ops,
        metrics.map_ops,
        metrics.string_ops,
        metrics.bc32_fallback_ops,
        metrics.bc32_fallback_build_misses,
        metrics.bc32_hot_stale_slots,
        metrics.bc32_hot_stale_misses,
        metrics.bc32_hot_sentinel_skips,
        metrics.val_clones,
        metrics.immediate_val_clones,
        metrics.heap_val_clones,
        metrics.register_writes,
        metrics.return_value_moves,
        metrics.quickening_hits,
        metrics.quickening_build_attempts,
        metrics.quickening_build_successes,
        metrics.quickening_misses,
        metrics.quickening_deopts,
        metrics.quickening_sentinel_skips,
    );
}
