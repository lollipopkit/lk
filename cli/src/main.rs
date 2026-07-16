use std::path::{Path, PathBuf};
#[cfg(feature = "llvm")]
use std::process::Command;
use std::sync::{Arc, Once};

static PERF_TRACE_INIT: Once = Once::new();
const DEFAULT_TRACE_FILTER: &str = "lk::vm::alloc=trace,lk::vm::slowpath=debug,lk_core=info,lk_cli=info";

use clap::{Parser, Subcommand, ValueEnum};
#[cfg(feature = "llvm")]
use lk_core::macro_system::{ProcMacroDependencyFingerprint, fingerprint_proc_macro_dependencies};
use lk_core::{
    macro_system::{AstMacroOrigin, MacroTokenOrigin, ProcMacroDependency},
    module::ModuleRegistry,
    package::{PackageGraph, PackageModule},
    stmt::{ModuleResolver, import::collect_program_imports},
    syntax::{expand_program_source, macro_origin_note_for_span, render_program, render_tokens, type_error_span},
    typ::TypeChecker,
    vm::{
        ModuleArtifact, Opcode, VM_INDEX_KEY_METRIC_NAMES, VM_REGISTER_WRITE_SOURCE_NAMES, VmContext, VmRuntimeMetrics,
        compile_program_module_with_ctx, execute_compiled_module_with_ctx, execute_module_artifact_with_ctx,
        execute_program_with_ctx_and_limits, vm_runtime_metrics_reset, vm_runtime_metrics_snapshot,
    },
};
#[cfg(feature = "llvm")]
use lk_llvm::{LlvmBackendOptions, OptLevel};

use anyhow::Context;

mod bytecode_cache;
mod coverage;
mod diagnostic;
#[cfg(test)]
mod main_test;
mod native_compile;
mod paths;
mod pkg;
mod repl;
mod repl_completion;
mod repl_tui;
mod startup_trace;
use self::native_compile::*;

use coverage::run_coverage_report;
#[cfg(test)]
use paths::split_compile_args_with_cwd;
use paths::{expand_program_file, parse_options_for_file, parse_sanitized_path, sanitize_path, split_compile_args};
use pkg::run_pkg_command;

#[derive(Debug, Parser)]
#[command(
    name = "lk",
    author,
    version,
    about = "CLI for LK",
    long_about = None,
    after_help = "Direct source execution uses the bytecode VM; `lk compile` emits a native executable by default."
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
    /// Emit a `.lkm` bytecode module. This is an INTERNAL artifact (version-locked
    /// to this build, like Python's `.pyc`), not a distribution format — ship
    /// source or a native executable instead.
    Bytecode,
    /// Emit LLVM IR (`.ll`).
    Llvm,
    /// Emit a native executable (default).
    Exe,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Compile sources into supported migration targets.
    Compile {
        /// 支持 `lk compile [TARGET] [FILE]`（默认编译 exe；省略 FILE 时自动查找当前目录入口）
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
        /// 输出文件路径（针对默认 exe 目标指定最终可执行文件路径）
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
    /// Format a source file in place (4-space indent). `--check` reports without writing.
    Fmt {
        /// Source file to format
        #[arg(value_name = "FILE", value_parser = parse_sanitized_path)]
        file: PathBuf,
        /// Do not write; exit non-zero if the file is not already formatted.
        #[arg(long)]
        check: bool,
    },
    /// AOT Tier 0: bundle a source file into a self-contained native executable
    /// that embeds the program and the VM (100% coverage; runs the VM at launch).
    Bundle {
        /// Source file to bundle
        #[arg(value_name = "FILE", value_parser = parse_sanitized_path)]
        file: PathBuf,
        /// Output executable path
        #[arg(short, long, value_name = "OUT", value_parser = parse_sanitized_path)]
        output: PathBuf,
    },
    /// Report VM coverage for a source file.
    Coverage {
        /// Source file to inspect
        #[arg(value_name = "FILE", value_parser = parse_sanitized_path)]
        file: PathBuf,
        /// Print disassembled VM functions after static coverage
        #[arg(long)]
        disassemble: bool,
        /// Execute after static coverage to collect clone/move runtime metrics
        #[arg(long)]
        runtime: bool,
    },
    /// Inspect macro expansion.
    Macro {
        #[command(subcommand)]
        command: MacroCommand,
    },
    /// Package manager commands.
    Pkg {
        #[command(subcommand)]
        command: PkgCommand,
    },
}

#[derive(Debug, Subcommand)]
enum MacroCommand {
    /// Expand macros in a source file and print the resulting LK token stream.
    Expand {
        /// Source file to expand
        #[arg(value_name = "FILE", value_parser = parse_sanitized_path)]
        file: PathBuf,
        /// Print expansion trace entries before expanded source
        #[arg(long)]
        trace: bool,
        /// Print procedural macro dependency metadata after expansion
        #[arg(long)]
        deps: bool,
        /// Print token-level macro origin metadata after expansion
        #[arg(long)]
        origins: bool,
        /// Enable a compile-time macro feature for cfg predicates; repeat for multiple features
        #[arg(long = "feature", value_name = "NAME")]
        features: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
enum PkgCommand {
    /// Create a package.
    Init {
        /// Package name. Defaults to the current directory name.
        name: Option<String>,
    },
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
    /// Validate package graph and macro provider distribution metadata.
    Check,
    /// Print the resolved dependency tree.
    Tree,
}

/// Unwrap an execution result, printing the VM call-stack traceback to stderr
/// first when it failed (plan M2.2). The traceback is only populated while an
/// error unwinds, so successful runs pay nothing.
fn unwrap_with_traceback<T>(result: anyhow::Result<T>, ctx: &VmContext) -> anyhow::Result<T> {
    if result.is_err()
        && let Some(report) = ctx.call_stack_report()
    {
        eprintln!("{report}");
    }
    result
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
    let heap_clones = metrics.copy_policy_heap_clones;
    let val_clones = heap_clones;
    format!(
        "VM profile: opcode_steps={} top_opcodes={} write_sources={} index_keys={} calls={} branches={} typed_branches={} containers={} list_ops={} map_ops={} string_ops={} val_clones={} heap_clones={} copy_policy_heap_clones={} register_copy_heap_clones={} local_copy_heap_clones={} local_load_heap_clones={} local_store_heap_clones={} const_load_heap_clones={} call_arg_heap_clones={} container_copy_heap_clones={}",
        metrics.opcode_steps,
        top_opcode_profile(&metrics),
        top_register_write_source_profile(&metrics),
        top_index_key_profile(&metrics),
        metrics.call_ops,
        metrics.branch_ops,
        metrics.typed_branch_ops,
        metrics.container_ops,
        metrics.list_ops,
        metrics.map_ops,
        metrics.string_ops,
        val_clones,
        heap_clones,
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

fn top_index_key_profile(metrics: &VmRuntimeMetrics) -> String {
    let mut pairs = Vec::new();
    for (name, count) in VM_INDEX_KEY_METRIC_NAMES.iter().zip(metrics.index_key_metrics.iter()) {
        if *count != 0 {
            pairs.push((*count, *name));
        }
    }
    pairs.sort_by(|(left_count, left_name), (right_count, right_name)| {
        right_count.cmp(left_count).then_with(|| left_name.cmp(right_name))
    });

    if pairs.is_empty() {
        return "none".to_string();
    }

    pairs
        .into_iter()
        .take(6)
        .map(|(count, name)| format!("{name}:{count}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn top_register_write_source_profile(metrics: &VmRuntimeMetrics) -> String {
    let mut pairs = Vec::new();
    for (name, count) in VM_REGISTER_WRITE_SOURCE_NAMES
        .iter()
        .zip(metrics.register_write_sources.iter())
    {
        if *count != 0 {
            pairs.push((*count, *name));
        }
    }
    pairs.sort_by(|(left_count, left_name), (right_count, right_name)| {
        right_count.cmp(left_count).then_with(|| left_name.cmp(right_name))
    });

    if pairs.is_empty() {
        return "none".to_string();
    }

    pairs
        .into_iter()
        .take(6)
        .map(|(count, name)| format!("{name}:{count}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn top_opcode_profile(metrics: &VmRuntimeMetrics) -> String {
    let mut pairs = Vec::new();
    for bits in 0..Opcode::COUNT {
        let count = metrics.opcode_histogram[bits as usize];
        if count == 0 {
            continue;
        }
        let opcode = Opcode::from_bits(bits).expect("valid opcode histogram slot");
        pairs.push((count, format!("{opcode:?}")));
    }
    pairs.sort_by(|(left_count, left_name), (right_count, right_name)| {
        right_count.cmp(left_count).then_with(|| left_name.cmp(right_name))
    });

    if pairs.is_empty() {
        return "none".to_string();
    }

    pairs
        .into_iter()
        .take(6)
        .map(|(count, name)| format!("{name}:{count}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn main() -> anyhow::Result<()> {
    let mut startup = startup_trace::StartupTrace::new("main");
    maybe_init_perf_tracing();
    startup.step("perf tracing checked");

    let CliArgs { command, file } = CliArgs::parse();
    startup.step("cli args parsed");

    // No args: enter REPL
    if command.is_none() && file.is_none() {
        startup.step("enter repl");
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
                        sanitize_path(p.to_string_lossy().as_ref()).inspect_err(|e| {
                            diagnostic::error(e);
                        })
                    })
                    .transpose()?;

                let compile_mode = pos_target;

                #[cfg(feature = "llvm")]
                if compile_mode != CompileMode::Exe && output.is_some() {
                    anyhow::bail!("--output is only supported for `lk compile <FILE>`");
                }

                match compile_mode {
                    CompileMode::Bytecode => {
                        compile_instr_module(&safe)?;
                        return Ok(());
                    }
                    CompileMode::Llvm => {
                        #[cfg(not(feature = "llvm"))]
                        anyhow::bail!(
                            "LLVM backend disabled at build time; rebuild with `--features llvm` to use `llvm` target"
                        );
                        #[cfg(feature = "llvm")]
                        {
                            let options = LlvmBackendOptions {
                                module_name: module_name_from_path(&safe),
                                target_triple,
                                run_optimizations: !skip_opt,
                                opt_level: opt_level_cli.into(),
                            };
                            compile_llvm_ir(&safe, options)?;
                            return Ok(());
                        }
                    }
                    CompileMode::Exe => {
                        #[cfg(not(feature = "llvm"))]
                        anyhow::bail!(
                            "LLVM backend disabled at build time; rebuild with `--features llvm` to compile native executables"
                        );
                        #[cfg(feature = "llvm")]
                        {
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
            }
            Commands::Check { file } => {
                run_type_check(&file)?;
                return Ok(());
            }
            Commands::Fmt { file, check } => {
                run_fmt(&file, check)?;
                return Ok(());
            }
            Commands::Bundle { file, output } => {
                run_bundle(&file, &output)?;
                return Ok(());
            }
            Commands::Coverage {
                file,
                disassemble,
                runtime,
            } => {
                run_coverage_report(&file, disassemble, runtime)?;
                return Ok(());
            }
            Commands::Macro { command } => {
                run_macro_command(command)?;
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
    let safe = sanitize_path(file.to_string_lossy().as_ref()).inspect_err(|e| {
        diagnostic::error(e);
    })?;
    let src_path_str = safe.to_string_lossy().to_string();
    let raw = std::fs::read(&safe).map_err(|e| anyhow::anyhow!("Failed to read file '{}': {}", src_path_str, e))?;

    if safe.extension().and_then(|ext| ext.to_str()) == Some("lkm") {
        let input =
            String::from_utf8(raw).map_err(|e| anyhow::anyhow!("Input file is not valid UTF-8 LK module: {}", e))?;
        let artifact =
            ModuleArtifact::from_json_str(&input).with_context(|| format!("decode Instr module {}", safe.display()))?;
        let mut base_env = build_vm_context(&safe)?;
        let profile_enabled = vm_profile_enabled();
        maybe_start_vm_profile(profile_enabled);
        let exec_result =
            execute_module_artifact_with_ctx(artifact, &mut base_env).with_context(|| "VM module execution failed");
        base_env.shutdown_async_runtime();
        let result = unwrap_with_traceback(exec_result, &base_env)?;
        maybe_print_vm_profile(profile_enabled);
        if !result.first_return_is_nil() {
            println!("{}", result.display_first_return());
        }
        return Ok(());
    }

    let input =
        String::from_utf8(raw).map_err(|e| anyhow::anyhow!("Input file is not valid UTF-8 LK source: {}", e))?;

    #[cfg(feature = "llvm")]
    if try_execute_cached_native(&safe, input.as_bytes())? {
        return Ok(());
    }

    // Optional bytecode cache (plan M1.3): with `LK_CACHE=1`, an unchanged
    // macro-free source skips parsing/compilation and runs its cached `.lkm`.
    // Sandboxed (fuel/heap-limited) runs bypass the cache — the limits are a
    // per-run policy, not part of the cached artifact.
    let cache_file = if fuel_budget_from_env().is_none() && heap_object_limit_from_env().is_none() {
        bytecode_cache::cache_path(&safe, input.as_bytes())
    } else {
        None
    };
    if let Some(cache_file) = cache_file.as_ref()
        && let Some(artifact) = bytecode_cache::load(cache_file)
    {
        let mut base_env = build_vm_context(&safe)?;
        let profile_enabled = vm_profile_enabled();
        maybe_start_vm_profile(profile_enabled);
        let exec_result = execute_module_artifact_with_ctx(artifact, &mut base_env)
            .with_context(|| "VM cached-module execution failed");
        base_env.shutdown_async_runtime();
        let result = unwrap_with_traceback(exec_result, &base_env)?;
        maybe_print_vm_profile(profile_enabled);
        if !result.first_return_is_nil() {
            println!("{}", result.display_first_return());
        }
        return Ok(());
    }

    // Parse, expand macros, and execute as statements.
    // NOTE: Direct `.lk` execution does not check proc-macro dependency
    // freshness against cached native binaries. Proc macros are always
    // re-expanded through the macro system when running in VM mode.
    let expansion = match expand_program_source(&input, parse_options_for_file(&safe)?) {
        Ok(expansion) => expansion,
        Err(parse_err) => {
            diagnostic::parse_error(&parse_err, &input);
            std::process::exit(1);
        }
    };
    // Only macro-free programs are cacheable: their bytecode is a pure function
    // of the source bytes (external proc-macro output is not).
    let macro_free = expansion.proc_macro_dependencies.is_empty();
    let program = expansion.program;

    let mut base_env = build_vm_context(&safe)?;

    let profile_enabled = vm_profile_enabled();
    maybe_start_vm_profile(profile_enabled);
    let fuel = fuel_budget_from_env();
    let heap_limit = heap_object_limit_from_env();
    let exec_result = if fuel.is_some() || heap_limit.is_some() {
        // Sandboxed run: fuel and/or heap-object cap. Skips the bytecode cache
        // (the limits are a per-run policy, not part of the cached artifact).
        execute_program_with_ctx_and_limits(&program, &mut base_env, fuel, heap_limit)
    } else {
        match cache_file.as_ref().filter(|_| macro_free) {
            // Compile once so the module can be both cached and executed.
            Some(cache_file) => match compile_program_module_with_ctx(&program, &mut base_env) {
                Ok(module) => {
                    bytecode_cache::store(cache_file, &program, &module);
                    execute_compiled_module_with_ctx(module, &mut base_env)
                }
                Err(err) => Err(err),
            },
            None => program.execute_with_ctx(&mut base_env),
        }
    }
    .with_context(|| "VM execution failed");

    // Shutdown runtime after execution
    base_env.shutdown_async_runtime();

    let result = unwrap_with_traceback(exec_result, &base_env)?;
    maybe_print_vm_profile(profile_enabled);

    if !result.first_return_is_nil() {
        println!("{}", result.display_first_return());
    }

    Ok(())
}

fn run_macro_command(command: MacroCommand) -> anyhow::Result<()> {
    match command {
        MacroCommand::Expand {
            file,
            trace,
            deps,
            origins,
            features,
        } => expand_macro_file(&file, trace, deps, origins, features),
    }
}

fn expand_macro_file(path: &Path, trace: bool, deps: bool, origins: bool, features: Vec<String>) -> anyhow::Result<()> {
    let input = std::fs::read_to_string(path).with_context(|| format!("read LK source {}", path.display()))?;
    let mut options = parse_options_for_file(path)?;
    options.macro_trace = trace;
    // Deduplicate features preserving first-occurrence order.
    let mut seen = std::collections::HashSet::new();
    options.macro_features = features.into_iter().filter(|f| seen.insert(f.clone())).collect();
    let expanded = expand_program_source(&input, options).map_err(|parse_err| {
        diagnostic::parse_error(&parse_err, &input);
        anyhow::anyhow!(parse_err.to_string())
    })?;
    if trace {
        for step in &expanded.source.trace {
            println!(
                "# macro {} at {}:{} -> {} tokens",
                step.macro_name, step.call_span.start.line, step.call_span.start.column, step.output_len
            );
        }
    }
    let token_output = render_tokens(&expanded.source.tokens);
    if expanded.ast_expanded {
        println!("# token macro expansion");
        println!("{token_output}");
        println!("# ast macro expansion");
        println!("{}", render_program(&expanded.program));
    } else {
        println!("{token_output}");
    }
    if deps {
        println!("# proc macro dependencies");
        println!("{}", serde_json::to_string_pretty(&expanded.proc_macro_dependencies)?);
    }
    if origins {
        println!("# macro token origins");
        println!(
            "{}",
            serde_json::to_string_pretty(&json_macro_origins(&expanded.source.origins))?
        );
        println!("# ast macro origins");
        println!(
            "{}",
            serde_json::to_string_pretty(&json_ast_macro_origins(&expanded.ast_macro_origins))?
        );
    }
    Ok(())
}

#[derive(serde::Serialize)]
struct JsonMacroTokenOrigin<'a> {
    token_index: usize,
    lexeme: &'a str,
    span: JsonSpan,
    frames: Vec<JsonMacroOriginFrame<'a>>,
}

#[derive(serde::Serialize)]
struct JsonMacroOriginFrame<'a> {
    macro_name: &'a str,
    kind: &'a str,
    call_span: JsonSpan,
}

#[derive(serde::Serialize)]
struct JsonAstMacroOrigin<'a> {
    macro_name: &'a str,
    kind: &'a str,
    input_span: Option<JsonSpan>,
    generated_items: usize,
    generated_item_labels: &'a [String],
    generated_item_origins: Vec<JsonAstGeneratedItemOrigin<'a>>,
}

#[derive(serde::Serialize)]
struct JsonAstGeneratedItemOrigin<'a> {
    label: &'a str,
    span: Option<JsonSpan>,
    generated_member_origins: Vec<JsonAstGeneratedMemberOrigin<'a>>,
}

#[derive(serde::Serialize)]
struct JsonAstGeneratedMemberOrigin<'a> {
    label: &'a str,
    span: Option<JsonSpan>,
}

#[derive(serde::Serialize)]
struct JsonSpan {
    start_line: u32,
    start_column: u32,
    start_offset: usize,
    end_line: u32,
    end_column: u32,
    end_offset: usize,
}

fn json_macro_origins(origins: &[MacroTokenOrigin]) -> Vec<JsonMacroTokenOrigin<'_>> {
    origins
        .iter()
        .map(|origin| JsonMacroTokenOrigin {
            token_index: origin.token_index,
            lexeme: &origin.lexeme,
            span: json_span(&origin.span),
            frames: origin
                .frames
                .iter()
                .map(|frame| JsonMacroOriginFrame {
                    macro_name: &frame.macro_name,
                    kind: frame.kind.as_str(),
                    call_span: json_span(&frame.call_span),
                })
                .collect(),
        })
        .collect()
}

fn json_ast_macro_origins(origins: &[AstMacroOrigin]) -> Vec<JsonAstMacroOrigin<'_>> {
    origins
        .iter()
        .map(|origin| JsonAstMacroOrigin {
            macro_name: &origin.macro_name,
            kind: origin.kind.as_str(),
            input_span: origin.input_span.as_ref().map(json_span),
            generated_items: origin.generated_items,
            generated_item_labels: &origin.generated_item_labels,
            generated_item_origins: origin
                .generated_item_origins
                .iter()
                .map(|item| JsonAstGeneratedItemOrigin {
                    label: &item.label,
                    span: item.span.as_ref().map(json_span),
                    generated_member_origins: item
                        .generated_member_origins
                        .iter()
                        .map(|member| JsonAstGeneratedMemberOrigin {
                            label: &member.label,
                            span: member.span.as_ref().map(json_span),
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect()
}

fn json_span(span: &lk_core::token::Span) -> JsonSpan {
    JsonSpan {
        start_line: span.start.line,
        start_column: span.start.column,
        start_offset: span.start.offset,
        end_line: span.end.line,
        end_column: span.end.column,
        end_offset: span.end.offset,
    }
}

fn run_type_check(path: &Path) -> anyhow::Result<()> {
    let input = std::fs::read_to_string(path).with_context(|| format!("read LK source {}", path.display()))?;
    let options = parse_options_for_file(path)?;
    let expanded = expand_program_source(&input, options).map_err(|parse_err| {
        diagnostic::parse_error(&parse_err, &input);
        anyhow::anyhow!(parse_err.to_string())
    })?;
    let mut checker = TypeChecker::new_strict();
    if let Err(err) = expanded.program.type_check(&mut checker) {
        let mut message = err.to_string();
        if let Some(span) = type_error_span(&err, &expanded.source.tokens, &expanded.source.spans)
            && let Some(note) = macro_origin_note_for_span(&expanded.source.origins, &span)
        {
            message.push('\n');
            message.push_str(&note);
        }
        diagnostic::error(anyhow::anyhow!(message));
        std::process::exit(1);
    }
    Ok(())
}

/// Optional instruction budget (fuel) for sandboxed execution, read from the
/// `LK_FUEL` environment variable. When set to a positive integer the VM aborts
/// after that many instructions instead of running unbounded — the fuel knob of
/// the sandbox model (plan M2.6). Absent/0/invalid means unlimited.
fn fuel_budget_from_env() -> Option<u64> {
    std::env::var("LK_FUEL")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|&budget| budget > 0)
}

/// Optional cap on live heap objects, read from `LK_MAX_HEAP_OBJECTS`. When set
/// to a positive integer, allocation beyond it aborts with a catchable
/// heap-limit error instead of growing unbounded — the memory knob of the
/// sandbox model (plan M2.6). This bounds the *count* of live objects (a coarse
/// memory proxy), not bytes; pair with `LK_FUEL` to bound total work/allocation.
/// Absent/0/invalid means unlimited.
fn heap_object_limit_from_env() -> Option<usize> {
    std::env::var("LK_MAX_HEAP_OBJECTS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|&limit| limit > 0)
}

/// `lk fmt FILE` — normalize indentation of `.lk` source in place (4-space,
/// brace/paren/bracket aware; blank lines kept blank). Mirrors the LSP document
/// formatter. `check` reports drift without writing (plan M5.3).
fn run_fmt(path: &Path, check: bool) -> anyhow::Result<()> {
    let input = std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("read {}: {}", path.display(), e))?;
    let formatted = format_lk_source(&input);
    if check {
        if formatted != input {
            anyhow::bail!("{} is not formatted (run `lk fmt {}`)", path.display(), path.display());
        }
        return Ok(());
    }
    if formatted != input {
        std::fs::write(path, &formatted).map_err(|e| anyhow::anyhow!("write {}: {}", path.display(), e))?;
        println!("formatted {}", path.display());
    }
    Ok(())
}

/// Indentation formatter (idempotent): 4-space, brace/paren/bracket aware.
fn format_lk_source(input: &str) -> String {
    const TAB: usize = 4;
    let mut out = String::with_capacity(input.len() + 16);
    let mut indent: isize = 0;
    for raw in input.lines() {
        let line = raw.trim();
        let leading_closers = line
            .chars()
            .take_while(|c| c.is_whitespace() || matches!(c, '}' | ')' | ']'))
            .filter(|c| matches!(c, '}' | ')' | ']'))
            .count();
        if leading_closers > 0 && indent > 0 {
            indent = (indent - leading_closers as isize).max(0);
        }
        if !line.is_empty() {
            for _ in 0..(indent.max(0) as usize * TAB) {
                out.push(' ');
            }
            out.push_str(line);
        }
        out.push('\n');
        let delta: isize = line
            .chars()
            .map(|c| match c {
                '{' | '(' | '[' => 1,
                '}' | ')' | ']' => -1,
                _ => 0,
            })
            .sum();
        indent = (indent + delta).max(0);
    }
    out
}

/// AOT Tier 0: bundle `source_path` into a self-contained native executable that
/// embeds the program source and the VM (via lk-api's C-ABI staticlib). 100%
/// coverage — the produced binary just runs the VM at launch, so any program that
/// runs under the VM bundles (unlike the MIR native path). Linux/`cc` for now.
fn run_bundle(source_path: &Path, output: &Path) -> anyhow::Result<()> {
    let source =
        std::fs::read_to_string(source_path).map_err(|e| anyhow::anyhow!("read {}: {}", source_path.display(), e))?;
    let staticlib = ensure_lk_api_staticlib()?;
    // Dev workspace layout: the C-ABI header lives in the workspace.
    let header_dir = workspace_root()?.join("api/include");
    let escaped = c_escape(&source);
    let wrapper = format!(
        "#include <stdio.h>\n#include \"lk.h\"\nstatic const char *LK_SRC = \"{escaped}\";\n\
         int main(void) {{\n  LkVm *vm = lk_vm_new();\n  char *out = lk_vm_eval(vm, LK_SRC);\n\
         if (out) {{ if (out[0]) printf(\"%s\\n\", out); lk_string_free(out); lk_vm_free(vm); return 0; }}\n\
         lk_vm_free(vm); fprintf(stderr, \"lk: execution failed\\n\"); return 1;\n}}\n"
    );
    let scratch = std::env::temp_dir().join(format!("lk_bundle_{}", std::process::id()));
    std::fs::create_dir_all(&scratch)?;
    let wrapper_c = scratch.join("wrapper.c");
    std::fs::write(&wrapper_c, wrapper)?;
    let status = std::process::Command::new("cc")
        .arg(&wrapper_c)
        .arg("-I")
        .arg(&header_dir)
        .arg(&staticlib)
        .args(["-lpthread", "-ldl", "-lm"])
        .arg("-o")
        .arg(output)
        .status()
        .map_err(|e| anyhow::anyhow!("cc: {e}"))?;
    let _ = std::fs::remove_dir_all(&scratch);
    if !status.success() {
        anyhow::bail!("cc failed to link the bundle");
    }
    println!(
        "bundled {} -> {} (self-contained; embeds the VM)",
        source_path.display(),
        output.display()
    );
    Ok(())
}

fn workspace_root() -> anyhow::Result<PathBuf> {
    Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cannot locate workspace root"))?
        .to_path_buf())
}

pub(crate) fn build_vm_context(path: &Path) -> anyhow::Result<VmContext> {
    let mut registry = ModuleRegistry::new();
    register_enabled_stdlib(&mut registry)?;
    let mut resolver = ModuleResolver::with_registry(registry);
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        resolver.set_base_dir(parent.to_path_buf());
    }
    configure_package_resolver(&mut resolver, path)?;
    let resolver = Arc::new(resolver);
    Ok(VmContext::new()
        .with_resolver(Arc::clone(&resolver))
        .with_type_checker(Some(TypeChecker::new_strict())))
}

pub(crate) fn register_enabled_stdlib(registry: &mut ModuleRegistry) -> anyhow::Result<()> {
    #[cfg(feature = "stdlib")]
    {
        lk_stdlib::register_stdlib_globals(registry);
        lk_stdlib::register_stdlib_modules(registry)?;
    }
    #[cfg(not(feature = "stdlib"))]
    {
        let _ = registry;
    }
    Ok(())
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

/// Compile-time bundling of file imports (`use "../general/fib"`) for the
/// native path: each imported file compiles on its own, its functions merge
/// into the main artifact's table (indices/global slots rewritten), and the
/// binding map goes to the lowering. Only *pure function-definition* modules
/// bundle (an entry with top-level effects, nested file imports, or non-file
/// import forms in the dep fails → the caller falls back to Tier 0).
#[cfg(feature = "llvm")]
fn bundle_file_imports(
    source: &Path,
    artifact: &ModuleArtifact,
) -> anyhow::Result<Option<(ModuleArtifact, Vec<lk_llvm::BundledImport>)>> {
    use lk_core::stmt::{ImportSource, ImportStmt};
    use lk_core::vm::{Instr, Opcode};

    let mut paths: Vec<String> = Vec::new();
    for import in &artifact.imports {
        let path = match import {
            ImportStmt::File { path } => Some(path),
            ImportStmt::Items {
                source: ImportSource::File(path),
                ..
            }
            | ImportStmt::Namespace {
                source: ImportSource::File(path),
                ..
            } => Some(path),
            _ => None,
        };
        if let Some(path) = path
            && !paths.contains(path)
        {
            paths.push(path.clone());
        }
    }
    if paths.is_empty() {
        return Ok(None);
    }

    let base_dir = source.parent().unwrap_or_else(|| Path::new("."));
    let mut merged = artifact.clone();
    let mut bundles = Vec::with_capacity(paths.len());
    for import_path in paths {
        let raw = Path::new(&import_path);
        // Mirror the runtime resolver's candidates: `p` (already .lk),
        // `p.lk`, `p/mod.lk`, under the importing file's directory.
        let mut candidates = Vec::new();
        if raw.extension().and_then(|e| e.to_str()) == Some("lk") {
            candidates.push(base_dir.join(raw));
        }
        candidates.push(base_dir.join(raw.with_extension("lk")));
        candidates.push(base_dir.join(raw).join("mod.lk"));
        let dep_path = candidates
            .into_iter()
            .find(|c| c.exists())
            .ok_or_else(|| anyhow::anyhow!("bundled import not found: {import_path}"))?;

        let dep = compile_instr_artifact_with_dependencies(&dep_path)?.artifact;
        // v1 guards: no nested file imports, no item/alias imports (their
        // bindings would need the dep's own import environment).
        for dep_import in &dep.imports {
            match dep_import {
                ImportStmt::Module { .. } => {}
                other => anyhow::bail!("bundled import '{import_path}' has an unsupported nested import: {other:?}"),
            }
        }
        let dep_entry = dep.module.entry as usize;
        // The dep entry must be pure `fn` bookkeeping: LoadFunction+SetGlobal
        // pairs and the implicit return. Anything else is a top-level effect
        // the bundle would silently skip — reject instead.
        let mut reg_fn: std::collections::HashMap<u8, u32> = std::collections::HashMap::new();
        let mut fns: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        let mut pairs: Vec<(String, u32)> = Vec::new();
        for raw_instr in &dep.module.functions[dep_entry].code {
            let instr = Instr::try_from_raw(*raw_instr)
                .map_err(|_| anyhow::anyhow!("bundled import '{import_path}': bad instruction"))?;
            match instr.opcode() {
                Opcode::LoadFunction => {
                    reg_fn.insert(instr.a(), u32::from(instr.bx()));
                }
                Opcode::SetGlobal => {
                    let Some(&fidx) = reg_fn.get(&instr.a()) else {
                        anyhow::bail!("bundled import '{import_path}' has a non-function top-level binding");
                    };
                    let name = dep.module.globals.get(instr.bx() as usize).cloned().unwrap_or_default();
                    pairs.push((name, fidx));
                }
                Opcode::Return0 => {}
                other => {
                    anyhow::bail!("bundled import '{import_path}' has top-level effects (opcode {other:?})")
                }
            }
        }

        // Merge: append every dep function except its entry; function indices
        // and global slots (by name) rewrite in place — pcs are unchanged, so
        // pc-keyed facts stay valid.
        let base = merged.module.functions.len() as u32;
        let mut remap: Vec<Option<u32>> = vec![None; dep.module.functions.len()];
        let mut next = base;
        for (i, slot) in remap.iter_mut().enumerate() {
            if i != dep_entry {
                *slot = Some(next);
                next += 1;
            }
        }
        let slot_of = |name: &str, globals: &mut Vec<String>| -> u16 {
            match globals.iter().position(|g| g == name) {
                Some(slot) => slot as u16,
                None => {
                    globals.push(name.to_string());
                    (globals.len() - 1) as u16
                }
            }
        };
        for (i, function) in dep.module.functions.iter().enumerate() {
            if i == dep_entry {
                continue;
            }
            let mut function = function.clone();
            for raw_instr in &mut function.code {
                let instr = Instr::try_from_raw(*raw_instr)
                    .map_err(|_| anyhow::anyhow!("bundled import '{import_path}': bad instruction"))?;
                let rewritten = match instr.opcode() {
                    Opcode::CallDirect | Opcode::MakeClosure => {
                        let fidx = instr.b() as usize;
                        let new = remap
                            .get(fidx)
                            .copied()
                            .flatten()
                            .ok_or_else(|| anyhow::anyhow!("bundled import '{import_path}' calls its entry"))?;
                        let new = u8::try_from(new)
                            .map_err(|_| anyhow::anyhow!("bundled import '{import_path}': function index overflow"))?;
                        Some(Instr::abc(instr.opcode(), instr.a(), new, instr.c()))
                    }
                    Opcode::LoadFunction => {
                        let fidx = instr.bx() as usize;
                        let new = remap
                            .get(fidx)
                            .copied()
                            .flatten()
                            .ok_or_else(|| anyhow::anyhow!("bundled import '{import_path}' loads its entry"))?;
                        let new = u16::try_from(new)
                            .map_err(|_| anyhow::anyhow!("bundled import '{import_path}': function index overflow"))?;
                        Some(Instr::abx(instr.opcode(), instr.a(), new))
                    }
                    Opcode::GetGlobal | Opcode::SetGlobal => {
                        let name = dep.module.globals.get(instr.bx() as usize).cloned().unwrap_or_default();
                        let slot = slot_of(&name, &mut merged.module.globals);
                        Some(Instr::abx(instr.opcode(), instr.a(), slot))
                    }
                    _ => None,
                };
                if let Some(instr) = rewritten {
                    *raw_instr = instr.raw();
                }
            }
            merged.module.functions.push(function);
        }
        for (name, fidx) in pairs {
            let merged_fidx = remap
                .get(fidx as usize)
                .copied()
                .flatten()
                .ok_or_else(|| anyhow::anyhow!("bundled import '{import_path}': dangling fn binding"))?;
            fns.insert(name, merged_fidx);
        }
        bundles.push(lk_llvm::BundledImport { path: import_path, fns });
    }
    Ok(Some((merged, bundles)))
}
