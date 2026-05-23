use std::path::{Path, PathBuf};
use std::sync::{Arc, Once};

static PERF_TRACE_INIT: Once = Once::new();
const DEFAULT_TRACE_FILTER: &str = "lk::vm::alloc=trace,lk::vm::slowpath=debug,lk_core=info,lk_cli=info";

use clap::{Parser, Subcommand, ValueEnum};
use lk_core::{
    module::ModuleRegistry,
    package::{PackageGraph, PackageModule},
    rt,
    stmt::{ModuleResolver, stmt_parser::StmtParser},
    token::Tokenizer,
    typ::TypeChecker,
    vm::VmContext,
};

use anyhow::Context;

mod coverage;
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
                    opt_level: _opt_level_cli,
                #[cfg(feature = "llvm")]
                    skip_opt: _skip_opt,
                #[cfg(feature = "llvm")]
                    target_triple: _target_triple,
                #[cfg(feature = "llvm")]
                    output: output_arg,
            } => {
                let (pos_target, safe) = split_compile_args(&positional)?;

                #[cfg(feature = "llvm")]
                let output = output_arg
                    .map(|p| {
                        sanitize_path(p.to_string_lossy().as_ref()).map_err(|e| {
                            eprintln!("Error: {}", e);
                            e
                        })
                    })
                    .transpose()?;

                let compile_mode = pos_target;

                #[cfg(feature = "llvm")]
                if compile_mode != Some(CompileMode::Exe) && output.is_some() {
                    anyhow::bail!("--output is only supported for `lk compile exe <FILE>`");
                }

                let src_path_str = safe.to_string_lossy().to_string();

                match compile_mode {
                    None => {
                        anyhow::bail!(
                            "compile output is disabled until the Instr32 module format replaces LKB: {}",
                            src_path_str
                        );
                    }
                    #[cfg(feature = "llvm")]
                    Some(CompileMode::Llvm) => {
                        anyhow::bail!(
                            "LLVM IR output is disabled during the Instr32 VM migration: {}",
                            src_path_str
                        );
                    }
                    #[cfg(feature = "llvm")]
                    Some(CompileMode::Exe) => {
                        anyhow::bail!(
                            "native executable output is disabled during the Instr32 VM migration: {}",
                            src_path_str
                        );
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

    // LKB execution is intentionally disabled while runtime execution is moved
    // to Instr32. Re-enable this through the new runtime module format, not by
    // reviving the legacy Vm path.
    if raw.starts_with(b"LKB") {
        anyhow::bail!(
            "LKB execution is disabled during the Instr32 VM migration: {}",
            src_path_str
        );
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

    let exec_result = program
        .execute32_with_ctx(&mut base_env)
        .with_context(|| "VM32 execution failed");

    // Shutdown runtime after execution
    rt::shutdown_runtime();

    let result = exec_result?;

    if !result.first_return_is_nil() {
        println!("{}", result.display_first_return());
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
        if resolver.resolve_runtime_module(&module.name).is_ok() {
            anyhow::bail!("Package module '{}' conflicts with a stdlib module", module.name);
        }
        resolver.register_package_module(module.name.clone(), module.root.clone());
    }
    Ok(())
}
