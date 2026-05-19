use std::path::Path;

#[cfg(feature = "llvm")]
use lk_core::llvm::{LlvmBackendOptions, OptLevel, compile_function_to_llvm};
use lk_core::vm::{self, Function, compile_program};

use crate::llvm_symbol_fragment;
use crate::paths::parse_program_file;

pub(crate) fn run_coverage_report(path: &Path) -> anyhow::Result<()> {
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
    print_function_coverage("entry", &func, 0);
    Ok(())
}

fn print_function_coverage(name: &str, function: &Function, depth: usize) {
    let indent = "  ".repeat(depth);
    let status = vm::bc32_pack_status(function);
    if status.packed {
        println!(
            "{indent}- {name}: packed ops={} words={}",
            status.ops,
            status.words.unwrap_or(0)
        );
    } else {
        println!(
            "{indent}- {name}: unpacked ops={} reason={} opcode={} op_index={} detail={}",
            status.ops,
            status.reason.as_deref().unwrap_or("unknown"),
            status.opcode.as_deref().unwrap_or("unknown"),
            status
                .op_index
                .map(|idx| idx.to_string())
                .unwrap_or_else(|| "n/a".to_string()),
            status.detail.as_deref().unwrap_or("")
        );
    }

    for (idx, proto) in function.protos.iter().enumerate() {
        let proto_name = proto
            .self_name
            .as_deref()
            .map(|self_name| format!("closure[{idx}] {self_name}"))
            .unwrap_or_else(|| format!("closure[{idx}]"));
        if let Some(nested) = proto.func.as_ref() {
            print_function_coverage(&proto_name, nested.as_ref(), depth + 1);
        } else {
            println!("{indent}  - {proto_name}: not materialized");
        }
    }
}
