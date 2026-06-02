use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Once};

static PERF_TRACE_INIT: Once = Once::new();
const DEFAULT_TRACE_FILTER: &str = "lk::vm::alloc=trace,lk::vm::slowpath=debug,lk_core=info,lk_cli=info";

use clap::{Parser, Subcommand, ValueEnum};
use lk_core::{
    llvm::{LlvmBackendOptions, OptLevel},
    module::ModuleRegistry,
    package::{PackageGraph, PackageModule},
    rt,
    stmt::{ModuleResolver, import::collect_program_imports, stmt_parser::StmtParser},
    token::Tokenizer,
    typ::TypeChecker,
    vm::{
        Module32Artifact, VmContext, VmRuntimeMetrics, compile_program32_module_with_ctx,
        execute_module32_artifact_with_ctx, vm_runtime_metrics_reset, vm_runtime_metrics_snapshot,
    },
};

use anyhow::Context;

mod coverage;
mod diagnostic;
#[cfg(test)]
mod main_test;
mod paths;
mod pkg;
mod repl;

use coverage::run_coverage_report;
#[cfg(test)]
use paths::split_compile_args_with_cwd;
use paths::{parse_program_file, parse_sanitized_path, sanitize_path, split_compile_args};
use pkg::{init_package, run_pkg_command};

#[derive(Debug, Parser)]
#[command(
    name = "lk",
    author,
    version,
    about = "CLI for LK",
    long_about = None,
    after_help = "Compiler and runtime migration target the Instr32 VM path."
)]
struct CliArgs {
    /// Subcommands like `compile FILE`
    #[command(subcommand)]
    command: Option<Commands>,

    /// If no subcommand, treat as a source file to execute (statements only)
    #[arg(value_name = "FILE", value_parser = parse_sanitized_path)]
    file: Option<PathBuf>,
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
            OptLevelCli::O0 => Self::None,
            OptLevelCli::O1 => Self::O1,
            OptLevelCli::O2 => Self::O2,
            OptLevelCli::O3 => Self::O3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CompileMode {
    #[cfg(feature = "llvm")]
    Llvm,
    #[cfg(feature = "llvm")]
    Exe,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Compile sources into supported migration targets.
    Compile {
        /// 支持 `lk compile [TARGET] [FILE]`（省略 FILE 时自动查找当前目录入口）
        #[arg(value_name = "ARGS", num_args = 0..=2)]
        positional: Vec<String>,
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
    /// Report VM coverage for a source file.
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
        .map(|raw| env_toggle_enabled(&raw))
        .unwrap_or(false)
}

fn maybe_start_vm_profile(enabled: bool) {
    if enabled {
        vm_runtime_metrics_reset();
    }
}

fn maybe_print_vm_profile(enabled: bool) {
    if !enabled {
        return;
    }
    let metrics = vm_runtime_metrics_snapshot();
    eprintln!("{}", vm_profile_line(metrics));
}

fn vm_profile_line(metrics: VmRuntimeMetrics) -> String {
    format!(
        "VM profile: opcode_steps={} calls={} branches={} typed_branches={} containers={} list_ops={} map_ops={} string_ops={} val_clones={} heap_clones={} copy_policy_heap_clones={} register_copy_heap_clones={} local_copy_heap_clones={} local_load_heap_clones={} local_store_heap_clones={} const_load_heap_clones={} call_arg_heap_clones={} container_copy_heap_clones={}",
        metrics.opcode_steps,
        metrics.call_ops,
        metrics.branch_ops,
        metrics.typed_branch_ops,
        metrics.container_ops,
        metrics.list_ops,
        metrics.map_ops,
        metrics.string_ops,
        metrics.copy_policy_heap_clones,
        metrics.copy_policy_heap_clones,
        metrics.copy_policy_heap_clones,
        metrics.register_copy_heap_clones,
        metrics.local_copy_heap_clones,
        metrics.local_load_heap_clones,
        metrics.local_store_heap_clones,
        metrics.const_load_heap_clones,
        metrics.call_arg_heap_clones,
        metrics.container_copy_heap_clones
    )
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

                #[cfg(feature = "llvm")]
                let output = output_arg
                    .map(|p| {
                        sanitize_path(p.to_string_lossy().as_ref()).map_err(|e| {
                            diagnostic::error(&e);
                            e
                        })
                    })
                    .transpose()?;

                let compile_mode = pos_target;

                #[cfg(feature = "llvm")]
                if compile_mode != Some(CompileMode::Exe) && output.is_some() {
                    anyhow::bail!("--output is only supported for `lk compile exe <FILE>`");
                }

                match compile_mode {
                    None => {
                        compile_instr32_module(&safe)?;
                        return Ok(());
                    }
                    #[cfg(feature = "llvm")]
                    Some(CompileMode::Llvm) => {
                        let options = LlvmBackendOptions {
                            module_name: module_name_from_path(&safe),
                            target_triple,
                            run_optimizations: !skip_opt,
                            opt_level: opt_level_cli.into(),
                        };
                        compile_llvm_ir(&safe, options)?;
                        return Ok(());
                    }
                    #[cfg(feature = "llvm")]
                    Some(CompileMode::Exe) => {
                        let options = LlvmBackendOptions {
                            module_name: module_name_from_path(&safe),
                            target_triple,
                            run_optimizations: !skip_opt,
                            opt_level: opt_level_cli.into(),
                        };
                        compile_executable(&safe, output.as_deref(), options)?;
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
    // Otherwise: execute FILE as statements
    let file = file.expect("internal: file should be present when no subcommand");
    let safe = sanitize_path(file.to_string_lossy().as_ref()).map_err(|e| {
        diagnostic::error(&e);
        e
    })?;
    let src_path_str = safe.to_string_lossy().to_string();
    let raw = std::fs::read(&safe).map_err(|e| anyhow::anyhow!("Failed to read file '{}': {}", src_path_str, e))?;

    if safe.extension().and_then(|ext| ext.to_str()) == Some("lkm") {
        let input =
            String::from_utf8(raw).map_err(|e| anyhow::anyhow!("Input file is not valid UTF-8 LK module: {}", e))?;
        if let Err(e) = rt::init_runtime() {
            diagnostic::warning(format_args!("Failed to initialize runtime: {}", e));
        }
        let artifact = Module32Artifact::from_json_str(&input)
            .with_context(|| format!("decode Instr32 module {}", safe.display()))?;
        let mut base_env = build_vm_context(&safe)?;
        let profile_enabled = vm_profile_enabled();
        maybe_start_vm_profile(profile_enabled);
        let exec_result =
            execute_module32_artifact_with_ctx(artifact, &mut base_env).with_context(|| "VM32 module execution failed");
        rt::shutdown_runtime();
        let result = exec_result?;
        maybe_print_vm_profile(profile_enabled);
        if !result.first_return_is_nil() {
            println!("{}", result.display_first_return());
        }
        return Ok(());
    }

    let input =
        String::from_utf8(raw).map_err(|e| anyhow::anyhow!("Input file is not valid UTF-8 LK source: {}", e))?;

    // Initialize runtime for concurrency if enabled
    if let Err(e) = rt::init_runtime() {
        diagnostic::warning(format_args!("Failed to initialize runtime: {}", e));
    }

    // Parse and execute as statements
    let (tokens, spans) = match Tokenizer::tokenize_enhanced_with_spans(&input) {
        Ok((tokens, spans)) => (tokens, spans),
        Err(parse_err) => {
            diagnostic::parse_error(&parse_err, &input);
            std::process::exit(1);
        }
    };
    let mut parser = StmtParser::new_with_spans(&tokens, &spans);
    let program = match parser.parse_program_with_enhanced_errors(&input) {
        Ok(program) => program,
        Err(parse_err) => {
            diagnostic::parse_error(&parse_err, &input);
            std::process::exit(1);
        }
    };

    let mut base_env = build_vm_context(&safe)?;

    let profile_enabled = vm_profile_enabled();
    maybe_start_vm_profile(profile_enabled);
    let exec_result = program
        .execute32_with_ctx(&mut base_env)
        .with_context(|| "VM32 execution failed");

    // Shutdown runtime after execution
    rt::shutdown_runtime();

    let result = exec_result?;
    maybe_print_vm_profile(profile_enabled);

    if !result.first_return_is_nil() {
        println!("{}", result.display_first_return());
    }

    Ok(())
}

fn run_type_check(path: &Path) -> anyhow::Result<()> {
    let program = parse_program_file(path)?;
    let mut checker = TypeChecker::new_strict();
    if let Err(err) = program.type_check(&mut checker) {
        diagnostic::error(&err);
        std::process::exit(1);
    }
    Ok(())
}

fn compile_instr32_module(path: &Path) -> anyhow::Result<()> {
    let artifact = compile_instr32_artifact(path)?;
    let output = path.with_extension("lkm");
    std::fs::write(&output, artifact.to_json_string()?)
        .with_context(|| format!("write Instr32 module {}", output.display()))?;
    println!("{}", output.display());
    Ok(())
}

fn compile_instr32_artifact(path: &Path) -> anyhow::Result<Module32Artifact> {
    let program = parse_program_file(path)?;
    let mut ctx = build_vm_context(path)?;
    let module = compile_program32_module_with_ctx(&program, &mut ctx)
        .with_context(|| format!("compile Instr32 module for {}", path.display()))?;
    Module32Artifact::new(collect_program_imports(&program), &module)
}

#[cfg(feature = "llvm")]
fn compile_llvm_ir(path: &Path, options: LlvmBackendOptions) -> anyhow::Result<()> {
    let artifact = compile_instr32_artifact(path)?;
    let llvm = lk_core::llvm::compile_module32_artifact_to_llvm(&artifact, options)
        .with_context(|| format!("compile LLVM IR for {}", path.display()))?;
    let output = path.with_extension("ll");
    std::fs::write(&output, llvm.module.ir).with_context(|| format!("write LLVM IR {}", output.display()))?;
    println!("{}", output.display());
    Ok(())
}

#[cfg(feature = "llvm")]
fn compile_executable(path: &Path, output: Option<&Path>, options: LlvmBackendOptions) -> anyhow::Result<()> {
    let artifact = compile_instr32_artifact(path)?;
    let output = output.map(Path::to_path_buf).unwrap_or_else(|| path.with_extension(""));
    let llvm = lk_core::llvm::compile_module32_artifact_to_llvm(&artifact, options)
        .with_context(|| format!("compile native executable LLVM IR for {}", path.display()))?;
    compile_native_executable_from_llvm(path, &output, &llvm.module.ir)?;
    println!("{}", output.display());
    Ok(())
}

#[cfg(feature = "llvm")]
fn compile_native_executable_from_llvm(path: &Path, output: &Path, ir: &str) -> anyhow::Result<()> {
    let _ = lkrt::link_anchor();
    let source_path = temp_llvm_source_path(path)?;
    std::fs::write(&source_path, ir).with_context(|| format!("write native LLVM IR {}", source_path.display()))?;
    let clang = clang_command();
    let mut command = Command::new(&clang);
    command.arg(&source_path).arg("-o").arg(output);
    if let Some(staticlib) = lkrt_staticlib_path() {
        add_force_load_staticlib(&mut command, &staticlib);
    }
    let output_status = command
        .output()
        .with_context(|| format!("spawn clang to build native executable {}", output.display()))?;
    let _ = std::fs::remove_file(&source_path);
    if !output_status.status.success() {
        anyhow::bail!(
            "native executable build failed for {}:\n{}",
            path.display(),
            String::from_utf8_lossy(&output_status.stderr)
        );
    }
    Ok(())
}

#[cfg(feature = "llvm")]
fn add_force_load_staticlib(command: &mut Command, staticlib: &Path) {
    if cfg!(target_os = "macos") {
        command.arg("-Wl,-force_load").arg(staticlib);
    } else {
        command
            .arg("-Wl,--whole-archive")
            .arg(staticlib)
            .arg("-Wl,--no-whole-archive");
    }
}

#[cfg(feature = "llvm")]
fn lkrt_staticlib_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("LKRT_STATICLIB") {
        return Some(PathBuf::from(path));
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let file = if cfg!(target_os = "windows") {
        "lkrt.lib"
    } else {
        "liblkrt.a"
    };
    let candidate = dir.join(file);
    if candidate.exists() {
        return Some(candidate);
    }
    latest_lkrt_staticlib_in_deps(&dir.join("deps"))
}

#[cfg(feature = "llvm")]
fn latest_lkrt_staticlib_in_deps(deps_dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(deps_dir).ok()?;
    let prefix = if cfg!(target_os = "windows") {
        "lkrt-"
    } else {
        "liblkrt-"
    };
    let suffix = if cfg!(target_os = "windows") { ".lib" } else { ".a" };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            if !name.starts_with(prefix) || !name.ends_with(suffix) {
                return None;
            }
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, path))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

#[cfg(feature = "llvm")]
fn clang_command() -> std::ffi::OsString {
    std::env::var_os("LK_CLANG")
        .or_else(|| std::env::var_os("CLANG"))
        .or_else(|| std::env::var_os("CC"))
        .unwrap_or_else(|| {
            let homebrew_llvm = Path::new("/opt/homebrew/opt/llvm/bin/clang");
            if homebrew_llvm.exists() {
                homebrew_llvm.as_os_str().to_os_string()
            } else {
                "clang".into()
            }
        })
}

#[cfg(feature = "llvm")]
fn temp_llvm_source_path(path: &Path) -> anyhow::Result<PathBuf> {
    let stem = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or("lk");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    Ok(std::env::temp_dir().join(format!("lk-{stem}-{}-{nanos}.ll", std::process::id())))
}

#[cfg(feature = "llvm")]
fn module_name_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| {
            stem.chars()
                .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
                .collect::<String>()
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "lk_module".to_string())
}

pub(crate) fn build_vm_context(path: &Path) -> anyhow::Result<VmContext> {
    let mut registry = ModuleRegistry::new();
    lk_stdlib::register_stdlib_globals(&mut registry);
    lk_stdlib::register_stdlib_modules(&mut registry)?;
    let mut resolver = ModuleResolver::with_registry(registry);
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        resolver.set_base_dir(parent.to_path_buf());
    }
    configure_package_resolver(&mut resolver, path)?;
    lk_stdlib::register_stdlib_lk_modules(&mut resolver)?;
    let resolver = Arc::new(resolver);
    Ok(VmContext::new()
        .with_resolver(Arc::clone(&resolver))
        .with_type_checker(Some(TypeChecker::new_strict())))
}

pub(crate) fn configure_package_resolver(
    resolver: &mut ModuleResolver,
    path: &Path,
) -> anyhow::Result<Option<PackageGraph>> {
    let Some(graph) = PackageGraph::discover(path)? else {
        return Ok(None);
    };
    register_package_modules(resolver, &graph.modules)?;
    Ok(Some(graph))
}

fn register_package_modules(resolver: &ModuleResolver, modules: &[PackageModule]) -> anyhow::Result<()> {
    for module in modules {
        if resolver.resolve_runtime_module(&module.name).is_ok() {
            anyhow::bail!("Package module '{}' conflicts with a stdlib module", module.name);
        }
        resolver.register_package_module(module.name.clone(), module.root.clone());
    }
    Ok(())
}
