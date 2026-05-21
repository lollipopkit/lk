use std::collections::{BTreeMap, BTreeSet};
#[cfg(feature = "llvm")]
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Once};

static PERF_TRACE_INIT: Once = Once::new();
const DEFAULT_TRACE_FILTER: &str =
    "lk::vm::alloc=trace,lk::vm::bc32=info,lk::vm::slowpath=debug,lk_core=info,lk_cli=info";

use clap::{Parser, Subcommand, ValueEnum};
#[cfg(feature = "llvm")]
use lk_core::llvm::{LlvmBackendOptions, OptLevel, compile_function_to_llvm};
use lk_core::{
    module::ModuleRegistry,
    package::{PackageGraph, PackageModule},
    rt,
    stmt::{
        ImportSource, ImportStmt, ModuleResolver, Program, Stmt, deserialize_imports, execute_imports,
        serialize_imports, stmt_parser::StmtParser,
    },
    token::Tokenizer,
    typ::TypeChecker,
    val::Val,
    vm::{
        self, BundledModule, BytecodeModule, Compiler, ModuleFlags, ModuleMeta, Vm, VmContext, compile_program,
        vm_runtime_metrics_reset, vm_runtime_metrics_snapshot,
    },
};

use anyhow::Context;

mod bundler;
mod coverage;
#[cfg(test)]
mod main_test;
#[cfg(feature = "llvm")]
mod native_runtime;
mod paths;
mod pkg;
mod repl;

use bundler::ModuleBundler;
use coverage::run_coverage_report;
#[cfg(feature = "llvm")]
use native_runtime::{ensure_runtime_staticlib, resolve_llvm_tool};
#[cfg(test)]
use paths::split_compile_args_with_cwd;
use paths::{parse_program_file, parse_sanitized_path, sanitize_path, split_compile_args};
use pkg::{init_package, run_pkg_command};

#[cfg(feature = "llvm")]
struct EncodedBundledModule {
    path: String,
    bytes: Vec<u8>,
}

#[cfg(feature = "llvm")]
struct NativeModuleFunction {
    module: String,
    export: String,
    symbol: String,
    arity: usize,
}

#[cfg(feature = "llvm")]
#[derive(Default)]
struct NativeModuleIr {
    final_ir: String,
    unoptimised_ir: String,
    functions: Vec<NativeModuleFunction>,
}

#[cfg(feature = "llvm")]
type NativeModuleFunctionDecl<'a> = (&'a str, &'a [String], &'a [lk_core::stmt::NamedParamDecl], &'a Stmt);

#[cfg(feature = "llvm")]
#[derive(Default)]
struct RuntimeInitPlan {
    declarations: Vec<String>,
    globals: Vec<String>,
    body_lines: Vec<String>,
}

#[cfg(feature = "llvm")]
impl RuntimeInitPlan {}

#[derive(Debug, Parser)]
#[command(
    name = "lk",
    author,
    version,
    about = "CLI for LK",
    long_about = None,
    after_help = "BC32 compression guide: docs/bc32.md"
)]
struct CliArgs {
    /// Subcommands like `compile FILE`
    #[command(subcommand)]
    command: Option<Commands>,

    /// If no subcommand, treat as a source file to execute (statements only)
    #[arg(value_name = "FILE", value_parser = parse_sanitized_path)]
    file: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum EmitKind {
    Bytecode,
    #[cfg(feature = "llvm")]
    Llvm,
}

#[cfg(feature = "llvm")]
#[derive(Debug, Clone, Copy, ValueEnum)]
enum OptLevelCli {
    O0,
    O1,
    O2,
    O3,
}

#[cfg(feature = "llvm")]
impl From<OptLevelCli> for OptLevel {
    fn from(value: OptLevelCli) -> Self {
        match value {
            OptLevelCli::O0 => OptLevel::None,
            OptLevelCli::O1 => OptLevel::O1,
            OptLevelCli::O2 => OptLevel::O2,
            OptLevelCli::O3 => OptLevel::O3,
        }
    }
}

#[cfg(feature = "llvm")]
impl OptLevelCli {
    fn label(self) -> &'static str {
        match self {
            OptLevelCli::O0 => "O0",
            OptLevelCli::O1 => "O1",
            OptLevelCli::O2 => "O2",
            OptLevelCli::O3 => "O3",
        }
    }
}

#[cfg(feature = "llvm")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeProfile {
    Release,
}

#[cfg(feature = "llvm")]
impl RuntimeProfile {
    fn use_release(self) -> bool {
        matches!(self, RuntimeProfile::Release)
    }

    fn label(self) -> &'static str {
        match self {
            RuntimeProfile::Release => "release",
        }
    }
}

impl From<EmitKind> for CompileMode {
    fn from(value: EmitKind) -> Self {
        match value {
            EmitKind::Bytecode => CompileMode::Lkb,
            #[cfg(feature = "llvm")]
            EmitKind::Llvm => CompileMode::Llvm,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CompileMode {
    #[value(name = "lkb", alias = "bytecode")]
    Lkb,
    #[cfg(feature = "llvm")]
    Llvm,
    #[cfg(feature = "llvm")]
    Exe,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Compile sources into bytecode / (optional) LLVM IR or native executables.
    Compile {
        /// 支持 `lk compile [TARGET] [FILE]`（默认为 `lkb`；省略 FILE 时自动查找当前目录入口）
        #[arg(value_name = "ARGS", num_args = 0..=2)]
        positional: Vec<String>,
        /// Emit format when未指定 `exe`（向后兼容，推荐使用位置参数）
        #[arg(long, value_enum, hide = true)]
        emit: Option<EmitKind>,
        #[cfg(feature = "llvm")]
        /// Optimisation level for LLVM backend
        #[cfg(feature = "llvm")]
        #[arg(long, value_enum, default_value_t = OptLevelCli::O2)]
        opt_level: OptLevelCli,
        #[cfg(feature = "llvm")]
        /// Skip running LLVM optimisation passes even if opt is available
        #[cfg(feature = "llvm")]
        #[arg(long)]
        skip_opt: bool,
        #[cfg(feature = "llvm")]
        /// Overrides LLVM target triple（默认自动推断）
        #[cfg(feature = "llvm")]
        #[arg(long)]
        target_triple: Option<String>,
        #[cfg(feature = "llvm")]
        /// 输出文件路径（针对 `exe` 目标指定最终 ELF 路径）
        #[cfg(feature = "llvm")]
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Type-check a source file without executing it.
    Check {
        /// Source file to type-check
        #[arg(value_name = "FILE", value_parser = parse_sanitized_path)]
        file: PathBuf,
    },
    /// Report benchmark-critical packed/AOT coverage for a source file.
    Coverage {
        /// Source file to inspect
        #[arg(value_name = "FILE", value_parser = parse_sanitized_path)]
        file: PathBuf,
        /// Execute after static coverage to collect clone/move runtime metrics
        #[arg(long)]
        runtime: bool,
    },
    /// Create and manage LK packages.
    Init {
        /// Package name. Defaults to the current directory name.
        name: Option<String>,
    },
    /// Package manager commands.
    Pkg {
        #[command(subcommand)]
        command: PkgCommand,
    },
}

#[derive(Debug, Subcommand)]
enum PkgCommand {
    /// Add a GitHub dependency to Lk.toml.
    Add {
        name: String,
        source: String,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        tag: Option<String>,
        #[arg(long)]
        rev: Option<String>,
    },
    /// Fetch dependencies and update Lk.lock.
    Fetch,
    /// Update one dependency or all dependencies.
    Update { name: Option<String> },
    /// Print the resolved dependency tree.
    Tree,
}

fn env_toggle_enabled(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return false;
    }
    !(trimmed.eq_ignore_ascii_case("0") || trimmed.eq_ignore_ascii_case("false") || trimmed.eq_ignore_ascii_case("off"))
}

fn filter_expr_from(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("1")
        || trimmed.eq_ignore_ascii_case("true")
        || trimmed.eq_ignore_ascii_case("on")
    {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn maybe_init_perf_tracing() {
    let raw = match std::env::var("LK_TRACE") {
        Ok(value) => value,
        Err(_) => return,
    };

    if !env_toggle_enabled(&raw) {
        return;
    }

    PERF_TRACE_INIT.call_once(|| {
        use tracing_subscriber::EnvFilter;
        use tracing_subscriber::fmt;

        let filter_expr = filter_expr_from(&raw).or_else(|| std::env::var("RUST_LOG").ok());

        let builder = fmt().with_writer(std::io::stderr);

        let builder = match filter_expr.and_then(|expr| EnvFilter::try_new(expr).ok()) {
            Some(filter) => builder.with_env_filter(filter),
            None => builder.with_env_filter(DEFAULT_TRACE_FILTER),
        };

        let _ = builder.try_init();
    });
}

fn vm_profile_enabled() -> bool {
    std::env::var("LK_VM_PROFILE")
        .ok()
        .is_some_and(|value| env_toggle_enabled(&value))
}

fn vm_profile_begin() -> bool {
    let enabled = vm_profile_enabled();
    if enabled {
        vm_runtime_metrics_reset();
    }
    enabled
}

fn print_vm_profile_metrics() {
    let metrics = vm_runtime_metrics_snapshot();
    eprintln!(
        "VM profile: opcode_steps={} branches={} typed_branches={} calls={} native_calls={} closure_calls={} exact_calls={} named_calls={} method_calls={} containers={} list_ops={} map_ops={} string_ops={} bc32_fallbacks={} bc32_build_misses={} bc32_stale_slots={} bc32_stale_misses={} bc32_sentinel_skips={} val_clones={} immediate_clones={} heap_clones={} copy_policy_heap_clones={} register_copy_heap_clones={} local_copy_heap_clones={} local_load_heap_clones={} local_store_heap_clones={} const_load_heap_clones={} call_arg_heap_clones={} container_copy_heap_clones={} register_writes={} return_value_moves={} quickening_hits={} quickening_build_attempts={} quickening_build_successes={} quickening_misses={} quickening_deopts={} quickening_sentinel_skips={}",
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
        metrics.copy_policy_heap_clones,
        metrics.register_copy_heap_clones,
        metrics.local_copy_heap_clones,
        metrics.local_load_heap_clones,
        metrics.local_store_heap_clones,
        metrics.const_load_heap_clones,
        metrics.call_arg_heap_clones,
        metrics.container_copy_heap_clones,
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

#[cfg(feature = "llvm")]
fn default_runtime_profile_for_exe() -> RuntimeProfile {
    RuntimeProfile::Release
}

#[cfg(feature = "llvm")]
fn default_executable_path(source: &Path, target_triple: Option<&str>) -> PathBuf {
    match default_executable_extension(target_triple) {
        Some(ext) => source.with_extension(ext),
        None => source.with_extension(""),
    }
}

#[cfg(feature = "llvm")]
fn default_executable_extension(target_triple: Option<&str>) -> Option<&'static str> {
    if let Some(triple) = target_triple {
        if triple.contains("windows") {
            return Some("exe");
        }
        if triple.contains("apple") {
            return None;
        }
        if triple.contains("linux") || triple.contains("elf") {
            return Some("elf");
        }
        return Some("out");
    }

    if cfg!(windows) {
        Some("exe")
    } else if cfg!(target_os = "macos") {
        None
    } else if cfg!(unix) {
        Some("elf")
    } else {
        Some("out")
    }
}

#[cfg(feature = "llvm")]
fn should_strip_executable(target_triple: Option<&str>) -> bool {
    target_triple.is_none()
}

#[cfg(feature = "llvm")]
fn strip_executable_if_needed(exe_path: &Path, target_triple: Option<&str>) -> anyhow::Result<bool> {
    if !should_strip_executable(target_triple) {
        return Ok(false);
    }

    let strip = std::env::var("LK_STRIP").unwrap_or_else(|_| "strip".to_string());
    let status = Command::new(&strip)
        .arg(exe_path)
        .status()
        .with_context(|| format!("failed to spawn strip tool `{strip}` for {}", exe_path.display()))?;
    if !status.success() {
        anyhow::bail!(
            "strip tool `{strip}` failed with status {status} for {}",
            exe_path.display()
        );
    }
    Ok(true)
}

#[cfg(feature = "llvm")]
fn bytecode_trampoline_c(bytecode: &[u8]) -> String {
    let mut out = String::new();
    out.push_str("#include <stddef.h>\nextern int lk_rt_run_bytecode(const unsigned char*, long long);\n");
    out.push_str("static const unsigned char LK_ENTRY_BYTECODE[] = {\n");
    for chunk in bytecode.chunks(16) {
        out.push_str("  ");
        for byte in chunk {
            let _ = write!(out, "0x{byte:02x}, ");
        }
        out.push('\n');
    }
    out.push_str(
        "};\nint main(void) { return lk_rt_run_bytecode(LK_ENTRY_BYTECODE, (long long)sizeof(LK_ENTRY_BYTECODE)); }\n",
    );
    out
}

fn bytecode_trampoline_ir(module_name: &str, bytecode: &[u8]) -> String {
    let len = bytecode.len();
    let literal = llvm_bytes_literal(bytecode);
    let mut out = String::new();
    out.push_str(&format!("; ModuleID = '{}_bytecode_trampoline'\n", module_name));
    out.push_str(&format!(
        "source_filename = \"{}_bytecode_trampoline\"\n\n",
        module_name
    ));
    out.push_str("declare i32 @lk_rt_run_bytecode(i8*, i64)\n\n");
    out.push_str(&format!(
        "@.lk_entry_bytecode = private unnamed_addr constant [{len} x i8] {literal}, align 1\n\n"
    ));
    out.push_str("define i32 @main() {\n");
    out.push_str(&format!(
        "  %status = call i32 @lk_rt_run_bytecode(i8* getelementptr inbounds ([{len} x i8], [{len} x i8]* @.lk_entry_bytecode, i64 0, i64 0), i64 {len})\n"
    ));
    out.push_str("  ret i32 %status\n");
    out.push_str("}\n");
    out
}

fn append_main_stub(ir: &str, entry: &str, init: &RuntimeInitPlan) -> String {
    let mut out = String::with_capacity(ir.len() + 256 + init.globals.iter().map(String::len).sum::<usize>());
    out.push_str(ir);
    if !ir.ends_with('\n') {
        out.push('\n');
    }

    if !init.declarations.is_empty() {
        for decl in &init.declarations {
            if !ir.contains(decl) {
                out.push_str(decl);
                if !decl.ends_with('\n') {
                    out.push('\n');
                }
            }
        }
    }

    if !init.globals.is_empty() {
        for global in &init.globals {
            out.push_str(global);
            if !global.ends_with('\n') {
                out.push('\n');
            }
        }
    }

    out.push_str("; --- auto-generated main stub ---\n");
    out.push_str("define i32 @main() {\n");
    for line in &init.body_lines {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&format!("  %result = call i64 @{entry}()\n"));
    out.push_str("  ret i32 0\n");
    out.push_str("}\n");
    out
}

#[cfg(feature = "llvm")]
fn llvm_bytes_literal(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 4 + 3);
    out.push('c');
    out.push('"');
    for &b in bytes {
        match b {
            b'"' => out.push_str("\\22"),
            b'\\' => out.push_str("\\5C"),
            b'\n' => out.push_str("\\0A"),
            b'\r' => out.push_str("\\0D"),
            b'\t' => out.push_str("\\09"),
            0x20..=0x7E => out.push(b as char),
            _ => {
                out.push('\\');
                let _ = write!(out, "{:02X}", b);
            }
        }
    }
    out.push('"');
    out
}

#[cfg(feature = "llvm")]
fn build_runtime_init_plan(
    module_ir: &str,
    search_paths: &[String],
    imports_json: Option<&str>,
    package_modules_json: Option<&str>,
    modules: &[EncodedBundledModule],
    native_functions: &[NativeModuleFunction],
    native_imports_only: bool,
) -> RuntimeInitPlan {
    let mut plan = RuntimeInitPlan::default();

    let decls = [
        "declare void @lk_rt_begin_session()",
        "declare void @lk_rt_register_search_path(i8*, i64)",
        "declare i32 @lk_rt_register_bundled_module(i8*, i64, i8*, i64)",
        "declare i32 @lk_rt_register_native_module_function(i8*, i64, i8*, i64, i8*, i64)",
        "declare i32 @lk_rt_register_imports(i8*, i64)",
        "declare i32 @lk_rt_register_package_modules(i8*, i64)",
        "declare i32 @lk_rt_apply_imports()",
        "declare i32 @lk_rt_apply_native_imports()",
    ];
    for decl in decls {
        if !module_ir.contains(decl) {
            plan.declarations.push(decl.to_string());
        }
    }

    plan.body_lines.push("call void @lk_rt_begin_session()".to_string());

    if let Some(imports) = imports_json.and_then(|raw| deserialize_imports(raw).ok()) {
        for module in stdlib_module_names_from_imports(&imports) {
            if let Some(symbol) = stdlib_require_symbol(&module) {
                let decl = format!("declare void @{symbol}()");
                if !module_ir.contains(&decl) && !plan.declarations.iter().any(|existing| existing == &decl) {
                    plan.declarations.push(decl);
                }
                plan.body_lines.push(format!("call void @{symbol}()"));
            }
        }
    }

    for (idx, path) in search_paths.iter().enumerate() {
        let bytes = path.as_bytes();
        if bytes.is_empty() {
            continue;
        }
        let len = bytes.len();
        let global_name = format!("@.lk_path.{}", idx);
        let literal = llvm_bytes_literal(bytes);
        plan.globals.push(format!(
            "{global_name} = private unnamed_addr constant [{len} x i8] {literal}, align 1"
        ));
        plan.body_lines.push(format!(
            "call void @lk_rt_register_search_path(i8* getelementptr inbounds ([{len} x i8], [{len} x i8]* {global_name}, i64 0, i64 0), i64 {len})"
        ));
    }

    for (idx, module) in modules.iter().enumerate() {
        let path_bytes = module.path.as_bytes();
        if path_bytes.is_empty() {
            continue;
        }
        let path_len = path_bytes.len();
        let path_name = format!("@.lk_mod_path.{}", idx);
        let path_literal = llvm_bytes_literal(path_bytes);
        plan.globals.push(format!(
            "{path_name} = private unnamed_addr constant [{path_len} x i8] {path_literal}, align 1"
        ));

        let blob_len = module.bytes.len();
        let blob_name = format!("@.lk_mod_blob.{}", idx);
        let blob_literal = llvm_bytes_literal(&module.bytes);
        plan.globals.push(format!(
            "{blob_name} = private unnamed_addr constant [{blob_len} x i8] {blob_literal}, align 1"
        ));

        plan.body_lines.push(format!(
            "call i32 @lk_rt_register_bundled_module(i8* getelementptr inbounds ([{path_len} x i8], [{path_len} x i8]* {path_name}, i64 0, i64 0), i64 {path_len}, i8* getelementptr inbounds ([{blob_len} x i8], [{blob_len} x i8]* {blob_name}, i64 0, i64 0), i64 {blob_len})"
        ));
    }

    for (idx, function) in native_functions.iter().enumerate() {
        let module_bytes = function.module.as_bytes();
        let export_bytes = function.export.as_bytes();
        if module_bytes.is_empty() || export_bytes.is_empty() {
            continue;
        }
        let module_len = module_bytes.len();
        let export_len = export_bytes.len();
        let module_name = format!("@.lk_native_module.{}", idx);
        let export_name = format!("@.lk_native_export.{}", idx);
        let module_literal = llvm_bytes_literal(module_bytes);
        let export_literal = llvm_bytes_literal(export_bytes);
        plan.globals.push(format!(
            "{module_name} = private unnamed_addr constant [{module_len} x i8] {module_literal}, align 1"
        ));
        plan.globals.push(format!(
            "{export_name} = private unnamed_addr constant [{export_len} x i8] {export_literal}, align 1"
        ));
        plan.body_lines.push(format!(
            "call i32 @lk_rt_register_native_module_function(i8* getelementptr inbounds ([{module_len} x i8], [{module_len} x i8]* {module_name}, i64 0, i64 0), i64 {module_len}, i8* getelementptr inbounds ([{export_len} x i8], [{export_len} x i8]* {export_name}, i64 0, i64 0), i64 {export_len}, i8* bitcast (i64 ({params})* @{symbol} to i8*), i64 {arity})",
            params = std::iter::repeat_n("i64", function.arity).collect::<Vec<_>>().join(", "),
            symbol = function.symbol,
            arity = function.arity
        ));
    }

    if let Some(imports) = imports_json {
        let bytes = imports.as_bytes();
        if !bytes.is_empty() {
            let len = bytes.len();
            let global_name = "@.lk_imports";
            let literal = llvm_bytes_literal(bytes);
            plan.globals.push(format!(
                "{global_name} = private unnamed_addr constant [{len} x i8] {literal}, align 1"
            ));
            plan.body_lines.push(format!(
                "call i32 @lk_rt_register_imports(i8* getelementptr inbounds ([{len} x i8], [{len} x i8]* {global_name}, i64 0, i64 0), i64 {len})"
            ));
        }
    }

    if let Some(package_modules) = package_modules_json {
        let bytes = package_modules.as_bytes();
        if !bytes.is_empty() {
            let len = bytes.len();
            let global_name = "@.lk_package_modules";
            let literal = llvm_bytes_literal(bytes);
            plan.globals.push(format!(
                "{global_name} = private unnamed_addr constant [{len} x i8] {literal}, align 1"
            ));
            plan.body_lines.push(format!(
                "call i32 @lk_rt_register_package_modules(i8* getelementptr inbounds ([{len} x i8], [{len} x i8]* {global_name}, i64 0, i64 0), i64 {len})"
            ));
        }
    }

    if native_imports_only {
        plan.body_lines
            .push("call i32 @lk_rt_apply_native_imports()".to_string());
    } else {
        plan.body_lines.push("call i32 @lk_rt_apply_imports()".to_string());
    }

    plan
}

#[cfg(feature = "llvm")]
fn stdlib_module_names_from_imports(imports: &[ImportStmt]) -> Vec<String> {
    let mut names = Vec::new();
    for import in imports {
        match import {
            ImportStmt::Module { module } | ImportStmt::ModuleAlias { module, .. } => {
                push_unique_stdlib_module(&mut names, module);
            }
            ImportStmt::Items {
                source: ImportSource::Module(module),
                ..
            }
            | ImportStmt::Namespace {
                source: ImportSource::Module(module),
                ..
            } => {
                push_unique_stdlib_module(&mut names, module);
            }
            ImportStmt::File { .. }
            | ImportStmt::Items {
                source: ImportSource::File(_),
                ..
            }
            | ImportStmt::Namespace {
                source: ImportSource::File(_),
                ..
            } => {}
        }
    }
    names
}

#[cfg(feature = "llvm")]
fn push_unique_stdlib_module(names: &mut Vec<String>, candidate: &str) {
    if stdlib_require_symbol(candidate).is_some() && !names.iter().any(|name| name == candidate) {
        names.push(candidate.to_string());
    }
}

#[cfg(feature = "llvm")]
fn stdlib_require_symbol(module: &str) -> Option<&'static str> {
    match module {
        "io" => Some("lk_rt_require_stdlib_io"),
        "json" => Some("lk_rt_require_stdlib_json"),
        "yaml" => Some("lk_rt_require_stdlib_yaml"),
        "toml" => Some("lk_rt_require_stdlib_toml"),
        "iter" => Some("lk_rt_require_stdlib_iter"),
        "math" => Some("lk_rt_require_stdlib_math"),
        "string" => Some("lk_rt_require_stdlib_string"),
        "list" => Some("lk_rt_require_stdlib_list"),
        "map" => Some("lk_rt_require_stdlib_map"),
        "datetime" => Some("lk_rt_require_stdlib_datetime"),
        "os" => Some("lk_rt_require_stdlib_os"),
        "tcp" => Some("lk_rt_require_stdlib_tcp"),
        "stream" => Some("lk_rt_require_stdlib_stream"),
        "task" => Some("lk_rt_require_stdlib_task"),
        "chan" => Some("lk_rt_require_stdlib_chan"),
        "time" => Some("lk_rt_require_stdlib_time"),
        _ => None,
    }
}

#[cfg(feature = "llvm")]
fn compile_native_package_modules(
    graph: Option<&PackageGraph>,
    imports: &[ImportStmt],
    options: &LlvmBackendOptions,
) -> anyhow::Result<Option<NativeModuleIr>> {
    let Some(graph) = graph else {
        return Ok(None);
    };
    let package_roots: BTreeMap<&str, &Path> = graph
        .modules
        .iter()
        .map(|module| (module.name.as_str(), module.root.as_path()))
        .collect();
    let mut requested = BTreeSet::new();
    for import in imports {
        match import {
            ImportStmt::Module { module } | ImportStmt::ModuleAlias { module, .. } => {
                if package_roots.contains_key(module.as_str()) {
                    requested.insert(module.clone());
                }
            }
            ImportStmt::Items {
                source: ImportSource::Module(module),
                ..
            }
            | ImportStmt::Namespace {
                source: ImportSource::Module(module),
                ..
            } => {
                if package_roots.contains_key(module.as_str()) {
                    requested.insert(module.clone());
                }
            }
            _ => {}
        }
    }
    if requested.is_empty() {
        return Ok(None);
    }

    let mut out = NativeModuleIr::default();
    for module in requested {
        let root = package_roots
            .get(module.as_str())
            .copied()
            .ok_or_else(|| anyhow::anyhow!("package module '{module}' not found"))?;
        let program = parse_program_file(root)?;
        let functions = match native_module_functions(&program) {
            Ok(functions) => functions,
            Err(_) => return Ok(None),
        };
        if functions.is_empty() {
            return Ok(None);
        }
        for (export, params, named_params, body) in functions {
            if !named_params.is_empty() {
                return Ok(None);
            }
            let function = Compiler::new().compile_function(params, named_params, body);
            let symbol = format!(
                "lk_mod_{}_{}",
                llvm_symbol_fragment(module.as_str()),
                llvm_symbol_fragment(export)
            );
            let mut function_options = options.clone();
            function_options.module_name = symbol.clone();
            let artifact = compile_function_to_llvm(&function, &symbol, function_options)
                .with_context(|| format!("compile native package function {module}.{export}"))?;
            out.final_ir.push_str(&strip_llvm_module_header(
                artifact.optimised_ir.as_deref().unwrap_or(&artifact.module.ir),
            ));
            out.final_ir.push('\n');
            out.unoptimised_ir
                .push_str(&strip_llvm_module_header(&artifact.module.ir));
            out.unoptimised_ir.push('\n');
            out.functions.push(NativeModuleFunction {
                module: module.clone(),
                export: export.to_string(),
                symbol,
                arity: params.len(),
            });
        }
    }
    Ok(Some(out))
}

#[cfg(feature = "llvm")]
fn native_module_functions(program: &Program) -> anyhow::Result<Vec<NativeModuleFunctionDecl<'_>>> {
    let mut functions = Vec::new();
    for stmt in &program.statements {
        match stmt.as_ref() {
            Stmt::Function {
                name,
                params,
                named_params,
                body,
                ..
            } => functions.push((name.as_str(), params.as_slice(), named_params.as_slice(), body.as_ref())),
            Stmt::Import(_) => {}
            other => {
                return Err(anyhow::anyhow!(
                    "native package module lowering only supports top-level functions, found {other:?}"
                ));
            }
        }
    }
    Ok(functions)
}

#[cfg(feature = "llvm")]
fn strip_llvm_module_header(ir: &str) -> String {
    ir.lines()
        .filter(|line| {
            !line.starts_with("; ModuleID")
                && !line.starts_with("source_filename")
                && !line.starts_with("target triple")
                && !line.starts_with("declare ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(feature = "llvm")]
fn llvm_symbol_fragment(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() { "module".to_string() } else { out }
}

fn main() -> anyhow::Result<()> {
    maybe_init_perf_tracing();

    let CliArgs { command, file } = CliArgs::parse();

    // No args: enter REPL
    if command.is_none() && file.is_none() {
        return repl::run(true);
    }

    if let Some(cmd) = command {
        match cmd {
            Commands::Compile {
                positional,
                emit,
                #[cfg(feature = "llvm")]
                    opt_level: opt_level_cli,
                #[cfg(feature = "llvm")]
                skip_opt,
                #[cfg(feature = "llvm")]
                target_triple,
                #[cfg(feature = "llvm")]
                    output: output_arg,
            } => {
                let (pos_target, safe) = split_compile_args(&positional)?;

                if pos_target.is_some() && emit.is_some() {
                    anyhow::bail!("--emit conflicts with positional target argument");
                }

                #[cfg(feature = "llvm")]
                let output = output_arg
                    .map(|p| {
                        sanitize_path(p.to_string_lossy().as_ref()).map_err(|e| {
                            eprintln!("Error: {}", e);
                            e
                        })
                    })
                    .transpose()?;

                let compile_mode = pos_target
                    .or_else(|| emit.map(CompileMode::from))
                    .unwrap_or(CompileMode::Lkb);

                #[cfg(feature = "llvm")]
                if compile_mode != CompileMode::Exe && output.is_some() {
                    anyhow::bail!("--output is only supported for `lk compile exe <FILE>`");
                }

                let src_path_str = safe.to_string_lossy().to_string();
                let program = parse_program_file(&safe)?;
                let package_graph = PackageGraph::discover(&safe)?;
                let func = compile_program(&program);
                if std::env::var_os("LK_DEBUG_BYTECODE").is_some() {
                    eprintln!("-- bytecode for {} --", src_path_str);
                    for (idx, op) in func.code.iter().enumerate() {
                        eprintln!("op[{idx}]: {op:?}");
                    }
                }

                match compile_mode {
                    CompileMode::Lkb => {
                        let mut module = BytecodeModule::new(func.clone());
                        module.flags.insert(ModuleFlags::CONST_FOLDED);
                        let mut meta = ModuleMeta {
                            source: Some(src_path_str.clone()),
                            ..Default::default()
                        };
                        meta.tags.insert("entry_kind".to_string(), "stmt".to_string());
                        if !meta.is_empty() {
                            module.meta = Some(meta);
                        }

                        let import_stmts = bundler::extract_import_statements(&program);
                        if !import_stmts.is_empty() {
                            let json = serialize_imports(&import_stmts).context("serialize entry imports")?;
                            module
                                .meta
                                .get_or_insert_with(Default::default)
                                .tags
                                .insert("imports".to_string(), json);
                        }

                        let parent_dir = safe.parent().filter(|p| !p.as_os_str().is_empty());
                        let mut bundler = ModuleBundler::new(parent_dir);
                        if let Some(graph) = package_graph.as_ref() {
                            bundler.register_package_modules(&graph.modules);
                        }
                        bundler.bundle_program(&program)?;
                        let package_modules_json = bundler.package_modules_json()?;
                        let bundled_modules = bundler.into_bundled();
                        if let Some(json) = package_modules_json {
                            module
                                .meta
                                .get_or_insert_with(Default::default)
                                .tags
                                .insert("package_modules".to_string(), json);
                        }
                        if !bundled_modules.is_empty() {
                            module.bundled_modules = bundled_modules;
                        }

                        let out_path = safe.with_extension("lkb");

                        let bytes = vm::encode_module(&module)?;
                        if let Some(parent) = out_path.parent()
                            && !parent.as_os_str().is_empty()
                        {
                            std::fs::create_dir_all(parent).with_context(|| {
                                format!("Failed to create parent directory for {}", out_path.display())
                            })?;
                        }
                        std::fs::write(&out_path, &bytes)
                            .with_context(|| format!("Failed to write bytecode to {}", out_path.display()))?;
                        eprintln!("Emitted bytecode to {} ({} bytes)", out_path.display(), bytes.len());
                        return Ok(());
                    }
                    #[cfg(feature = "llvm")]
                    CompileMode::Llvm => {
                        let module_name = safe
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| "lk_module".to_string());
                        let options = LlvmBackendOptions {
                            module_name,
                            target_triple: target_triple.clone(),
                            run_optimizations: !skip_opt,
                            opt_level: opt_level_cli.into(),
                        };
                        let artifact = compile_function_to_llvm(&func, "lk_entry", options).context("LLVM backend")?;

                        let out_path = safe.with_extension("ll");
                        if let Some(parent) = out_path.parent()
                            && !parent.as_os_str().is_empty()
                        {
                            std::fs::create_dir_all(parent).with_context(|| {
                                format!("Failed to create parent directory for {}", out_path.display())
                            })?;
                        }

                        let final_ir = artifact.optimised_ir.as_deref().unwrap_or(&artifact.module.ir);
                        std::fs::write(&out_path, final_ir)
                            .with_context(|| format!("Failed to write LLVM IR to {}", out_path.display()))?;

                        if artifact.optimised_ir.is_some() {
                            let mut unopt_path = out_path.clone();
                            unopt_path.set_extension("unopt.ll");
                            std::fs::write(&unopt_path, &artifact.module.ir).with_context(|| {
                                format!("Failed to write unoptimised LLVM IR to {}", out_path.display())
                            })?;
                            eprintln!(
                                "Emitted LLVM IR to {} (optimised, opt-level {})",
                                out_path.display(),
                                opt_level_cli.label()
                            );
                            eprintln!("Preserved unoptimised IR at {}", unopt_path.display());
                        } else {
                            eprintln!("Emitted LLVM IR to {} (unoptimised)", out_path.display());
                        }
                        return Ok(());
                    }
                    #[cfg(feature = "llvm")]
                    CompileMode::Exe => {
                        let import_stmts = bundler::extract_import_statements(&program);
                        let imports_serialized = if import_stmts.is_empty() {
                            None
                        } else {
                            Some(serialize_imports(&import_stmts).context("serialize entry imports")?)
                        };

                        let parent_dir_owned = safe
                            .parent()
                            .filter(|p| !p.as_os_str().is_empty())
                            .map(Path::to_path_buf);
                        let mut search_paths = Vec::new();
                        if let Some(parent) = &parent_dir_owned {
                            let as_str = parent.to_string_lossy().to_string();
                            if !as_str.is_empty() {
                                search_paths.push(as_str);
                            }
                        }

                        let mut bundler = ModuleBundler::new(parent_dir_owned.as_deref());
                        if let Some(graph) = package_graph.as_ref() {
                            bundler.register_package_modules(&graph.modules);
                        }
                        bundler.bundle_program(&program)?;
                        let package_modules_json = bundler.package_modules_json()?;
                        let bundled_modules = bundler.into_bundled();
                        let mut encoded_modules = Vec::new();
                        for bundled in &bundled_modules {
                            let bytes = vm::encode_module(&bundled.module)
                                .with_context(|| format!("encode bundled module {}", bundled.path))?;
                            encoded_modules.push(EncodedBundledModule {
                                path: bundled.path.clone(),
                                bytes,
                            });
                        }

                        let module_name = safe
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| "lk_module".to_string());
                        let options = LlvmBackendOptions {
                            module_name: module_name.clone(),
                            target_triple: target_triple.clone(),
                            run_optimizations: !skip_opt,
                            opt_level: opt_level_cli.into(),
                        };
                        let native_modules =
                            compile_native_package_modules(package_graph.as_ref(), &import_stmts, &options)?;
                        let native_imports_only = native_modules
                            .as_ref()
                            .is_some_and(|native| !native.functions.is_empty());
                        let active_package_modules_json = if native_imports_only {
                            None
                        } else {
                            package_modules_json.as_deref()
                        };
                        let active_encoded_modules: &[EncodedBundledModule] =
                            if native_imports_only { &[] } else { &encoded_modules };
                        let llvm_artifact = compile_function_to_llvm(&func, "lk_entry", options);
                        let (ll_with_main, unopt_with_main, bytecode_trampoline_c_src) = match llvm_artifact {
                            Ok(artifact) => {
                                let final_ir = artifact.optimised_ir.as_deref().unwrap_or(&artifact.module.ir);
                                let combined_final_ir = if let Some(native) = native_modules.as_ref() {
                                    format!("{final_ir}\n{}", native.final_ir)
                                } else {
                                    final_ir.to_string()
                                };
                                let runtime_plan = build_runtime_init_plan(
                                    &combined_final_ir,
                                    &search_paths,
                                    imports_serialized.as_deref(),
                                    active_package_modules_json,
                                    active_encoded_modules,
                                    native_modules
                                        .as_ref()
                                        .map(|native| native.functions.as_slice())
                                        .unwrap_or(&[]),
                                    native_imports_only,
                                );
                                let ll_with_main = append_main_stub(&combined_final_ir, "lk_entry", &runtime_plan);
                                let combined_unopt_ir = if let Some(native) = native_modules.as_ref() {
                                    format!("{}\n{}", artifact.module.ir, native.unoptimised_ir)
                                } else {
                                    artifact.module.ir.clone()
                                };
                                let unopt_plan = build_runtime_init_plan(
                                    &combined_unopt_ir,
                                    &search_paths,
                                    imports_serialized.as_deref(),
                                    active_package_modules_json,
                                    active_encoded_modules,
                                    native_modules
                                        .as_ref()
                                        .map(|native| native.functions.as_slice())
                                        .unwrap_or(&[]),
                                    native_imports_only,
                                );
                                let unopt_with_main = append_main_stub(&combined_unopt_ir, "lk_entry", &unopt_plan);
                                (ll_with_main, Some(unopt_with_main), None)
                            }
                            Err(err) => {
                                eprintln!(
                                    "LLVM backend could not lower this program ({err}); emitting VM bytecode trampoline executable"
                                );
                                let mut module = BytecodeModule::new(func.clone());
                                module.flags.insert(ModuleFlags::CONST_FOLDED);
                                let mut meta = ModuleMeta {
                                    source: Some(src_path_str.clone()),
                                    ..Default::default()
                                };
                                meta.tags.insert("entry_kind".to_string(), "stmt".to_string());
                                if let Some(imports) = imports_serialized.as_ref() {
                                    meta.tags.insert("imports".to_string(), imports.clone());
                                }
                                if let Some(package_modules) = package_modules_json.as_ref() {
                                    meta.tags.insert("package_modules".to_string(), package_modules.clone());
                                }
                                module.meta = Some(meta);
                                module.bundled_modules = bundled_modules;
                                let bytes = vm::encode_module(&module).context("encode bytecode trampoline payload")?;
                                let c_src = bytecode_trampoline_c(&bytes);
                                (bytecode_trampoline_ir(&module_name, &bytes), None, Some(c_src))
                            }
                        };

                        let ll_path = safe.with_extension("ll");
                        if let Some(parent) = ll_path.parent()
                            && !parent.as_os_str().is_empty()
                        {
                            std::fs::create_dir_all(parent).with_context(|| {
                                format!("Failed to create parent directory for {}", ll_path.display())
                            })?;
                        }
                        std::fs::write(&ll_path, &ll_with_main)
                            .with_context(|| format!("Failed to write LLVM IR to {}", ll_path.display()))?;

                        if let Some(unopt_with_main) = &unopt_with_main {
                            let mut unopt_path = ll_path.clone();
                            unopt_path.set_extension("unopt.ll");
                            std::fs::write(&unopt_path, unopt_with_main).with_context(|| {
                                format!("Failed to write unoptimised LLVM IR to {}", ll_path.display())
                            })?;
                        }

                        let runtime_profile = default_runtime_profile_for_exe();
                        let runtime_staticlibs =
                            ensure_runtime_staticlib(target_triple.as_deref(), runtime_profile.use_release())
                                .with_context(|| "failed to produce LLVM runtime static library")?;

                        let exe_path = output
                            .clone()
                            .unwrap_or_else(|| default_executable_path(&safe, target_triple.as_deref()));
                        let cc = std::env::var("LK_CC")
                            .or_else(|_| std::env::var("CC"))
                            .unwrap_or_else(|_| "cc".to_string());

                        let obj_path = safe.with_extension("o");
                        let mut cc_input = obj_path.clone();
                        if let Some(llc_path) = resolve_llvm_tool("llc", "LK_LLVM_LLC") {
                            let mut llc_cmd = Command::new(&llc_path);
                            llc_cmd.arg("-filetype=obj").arg(&ll_path).arg("-o").arg(&obj_path);
                            if let Some(triple) = &target_triple {
                                llc_cmd.arg("-mtriple").arg(triple);
                            }
                            let llc_status = llc_cmd
                                .status()
                                .with_context(|| format!("failed to spawn llc at {}", llc_path.display()))?;
                            if !llc_status.success() {
                                anyhow::bail!("llc failed with status {}", llc_status);
                            }
                        } else if let Some(c_src) = &bytecode_trampoline_c_src {
                            let c_path = safe.with_extension("trampoline.c");
                            std::fs::write(&c_path, c_src)
                                .with_context(|| format!("Failed to write C trampoline to {}", c_path.display()))?;
                            cc_input = c_path;
                        } else {
                            anyhow::bail!("llc tool not found");
                        }

                        let mut cc_cmd = Command::new(&cc);
                        cc_cmd.arg(&cc_input);
                        for lib in &runtime_staticlibs {
                            cc_cmd.arg(lib);
                        }
                        cc_cmd.arg("-o").arg(&exe_path);
                        if let Some(triple) = &target_triple {
                            cc_cmd.arg(format!("--target={}", triple));
                        }
                        if !cfg!(target_os = "macos") && !cfg!(target_os = "windows") {
                            cc_cmd.arg("-lm");
                        }
                        let target_is_apple = target_triple
                            .as_deref()
                            .map(|triple| triple.contains("apple"))
                            .unwrap_or(cfg!(target_os = "macos"));
                        if target_is_apple {
                            cc_cmd.arg("-Wl,-dead_strip");
                            cc_cmd.arg("-framework").arg("CoreFoundation");
                            cc_cmd.arg("-framework").arg("CoreServices");
                        }
                        let cc_status = cc_cmd
                            .status()
                            .with_context(|| format!("failed to spawn linker {}", cc))?;
                        if !cc_status.success() {
                            anyhow::bail!("linker {} failed with status {}", cc, cc_status);
                        }
                        let stripped = strip_executable_if_needed(&exe_path, target_triple.as_deref())?;

                        let backend_label = if bytecode_trampoline_c_src.is_some() {
                            "VM bytecode trampoline"
                        } else {
                            opt_level_cli.label()
                        };
                        eprintln!(
                            "Emitted native executable to {} (backend {}, runtime {}, {}, LLVM IR at {})",
                            exe_path.display(),
                            backend_label,
                            runtime_profile.label(),
                            if stripped { "stripped" } else { "not stripped" },
                            ll_path.display()
                        );
                        return Ok(());
                    }
                }
            }
            Commands::Check { file } => {
                run_type_check(&file)?;
                return Ok(());
            }
            Commands::Coverage { file, runtime } => {
                run_coverage_report(&file, runtime)?;
                return Ok(());
            }
            Commands::Init { name } => {
                init_package(name)?;
                return Ok(());
            }
            Commands::Pkg { command } => {
                run_pkg_command(command)?;
                return Ok(());
            }
        }
    }
    // No separate subcommand to run bytecode; handled below by auto-detecting LKB magic

    // Otherwise: execute FILE as statements
    let file = file.expect("internal: file should be present when no subcommand");
    let safe = sanitize_path(file.to_string_lossy().as_ref()).map_err(|e| {
        eprintln!("Error: {}", e);
        e
    })?;
    let src_path_str = safe.to_string_lossy().to_string();
    // Read raw bytes first to auto-detect LKB magic
    let raw = std::fs::read(&safe).map_err(|e| anyhow::anyhow!("Failed to read file '{}': {}", src_path_str, e))?;

    // If LKB magic present, decode and execute via VM
    if raw.starts_with(b"LKB") {
        let module = vm::decode_module(&raw).with_context(|| format!("Failed to decode LKB from {}", src_path_str))?;

        // Initialize runtime for concurrency if enabled
        if let Err(e) = rt::init_runtime() {
            eprintln!("Warning: Failed to initialize runtime: {}", e);
        }

        // Prepare environment with stdlib
        let mut registry = ModuleRegistry::new();
        lk_stdlib::register_stdlib_globals(&mut registry);
        lk_stdlib::register_stdlib_modules(&mut registry)?;
        let mut resolver = ModuleResolver::with_registry(registry);
        if let Some(parent) = safe.parent().filter(|p| !p.as_os_str().is_empty()) {
            resolver.set_base_dir(parent.to_path_buf());
        }
        let resolver = Arc::new(resolver);
        register_embedded_modules(&resolver, &module.bundled_modules);
        register_package_modules_from_meta(&resolver, module.meta.as_ref())?;
        let mut base_env = VmContext::new()
            .with_resolver(Arc::clone(&resolver))
            .with_type_checker(Some(TypeChecker::new_strict()));

        if let Some(meta) = module.meta.as_ref()
            && let Some(imports_json) = meta.tags.get("imports")
        {
            let imports = deserialize_imports(imports_json)
                .with_context(|| format!("Failed to parse serialized imports for {}", src_path_str))?;
            execute_imports(&imports, resolver.as_ref(), &mut base_env)
                .with_context(|| format!("Failed to replay imports for {}", src_path_str))?;
        }

        let profile = vm_profile_begin();
        let mut vm = vm::Vm::new();
        let result = vm.exec_with(&module.entry, &mut base_env, None);
        if profile {
            print_vm_profile_metrics();
        }

        rt::shutdown_runtime();

        match result {
            Ok(res) => {
                if !matches!(res, Val::Nil) {
                    println!("{}", res.display_string(Some(&base_env)));
                }
                return Ok(());
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
    }

    // Otherwise: treat as UTF-8 LK source and execute statements
    let input = String::from_utf8(raw)
        .map_err(|e| anyhow::anyhow!("Input file is neither LKB bytecode nor valid UTF-8 source: {}", e))?;

    // Initialize runtime for concurrency if enabled
    if let Err(e) = rt::init_runtime() {
        eprintln!("Warning: Failed to initialize runtime: {}", e);
    }

    // Parse and execute as statements
    let (tokens, spans) = match Tokenizer::tokenize_enhanced_with_spans(&input) {
        Ok((tokens, spans)) => (tokens, spans),
        Err(parse_err) => {
            eprintln!("Error: {}", parse_err);
            std::process::exit(1);
        }
    };
    let mut parser = StmtParser::new_with_spans(&tokens, &spans);
    let program = match parser.parse_program_with_enhanced_errors(&input) {
        Ok(program) => program,
        Err(parse_err) => {
            eprintln!("Error: {}", parse_err);
            std::process::exit(1);
        }
    };

    let mut registry = ModuleRegistry::new();
    lk_stdlib::register_stdlib_globals(&mut registry);
    lk_stdlib::register_stdlib_modules(&mut registry)?;
    let mut resolver = ModuleResolver::with_registry(registry);
    if let Some(parent) = safe.parent().filter(|p| !p.as_os_str().is_empty()) {
        resolver.set_base_dir(parent.to_path_buf());
    }
    configure_package_resolver(&mut resolver, &safe)?;
    let resolver = Arc::new(resolver);
    let mut base_env = VmContext::new()
        .with_resolver(Arc::clone(&resolver))
        .with_type_checker(Some(TypeChecker::new_strict()));

    let import_stmts = bundler::extract_import_statements(&program);
    if !import_stmts.is_empty() {
        execute_imports(&import_stmts, resolver.as_ref(), &mut base_env)
            .with_context(|| format!("Failed to execute imports for {}", src_path_str))?;
    }

    let exec_result: anyhow::Result<(Val, VmContext)> = {
        let compiled = compile_program(&program);
        let profile = vm_profile_begin();
        let mut vm = Vm::new();
        let val = vm
            .exec_with(&compiled, &mut base_env, None)
            .with_context(|| "VM execution failed")?;
        if profile {
            print_vm_profile_metrics();
        }
        let env_after = base_env.snapshot();
        Ok((val, env_after))
    };

    // Shutdown runtime after execution
    rt::shutdown_runtime();

    let (result, env) = exec_result?;

    if !matches!(result, Val::Nil) {
        println!("{}", result.display_string(Some(&env)));
    }

    Ok(())
}

fn run_type_check(path: &Path) -> anyhow::Result<()> {
    let program = parse_program_file(path)?;
    let mut checker = TypeChecker::new_strict();
    if let Err(err) = program.type_check(&mut checker) {
        eprintln!("Error: {}", err);
        std::process::exit(1);
    }
    Ok(())
}

fn configure_package_resolver(resolver: &mut ModuleResolver, path: &Path) -> anyhow::Result<Option<PackageGraph>> {
    let Some(graph) = PackageGraph::discover(path)? else {
        return Ok(None);
    };
    register_package_modules(resolver, &graph.modules)?;
    Ok(Some(graph))
}

fn register_package_modules(resolver: &ModuleResolver, modules: &[PackageModule]) -> anyhow::Result<()> {
    for module in modules {
        if resolver.resolve_module(&module.name).is_ok() {
            anyhow::bail!("Package module '{}' conflicts with a stdlib module", module.name);
        }
        resolver.register_package_module(module.name.clone(), module.root.clone());
    }
    Ok(())
}

fn register_package_modules_from_meta(resolver: &Arc<ModuleResolver>, meta: Option<&ModuleMeta>) -> anyhow::Result<()> {
    let Some(raw) = meta.and_then(|meta| meta.tags.get("package_modules")) else {
        return Ok(());
    };
    let modules: BTreeMap<String, String> = serde_json::from_str(raw).context("parse package module metadata")?;
    for (name, path) in modules {
        resolver.register_package_module(name, PathBuf::from(path));
    }
    Ok(())
}

fn register_embedded_modules(resolver: &Arc<ModuleResolver>, modules: &[BundledModule]) {
    for bundled in modules {
        let path = PathBuf::from(&bundled.path);
        resolver.register_embedded_module(path, bundled.module.clone());
        if !bundled.module.bundled_modules.is_empty() {
            register_embedded_modules(resolver, &bundled.module.bundled_modules);
        }
    }
}
