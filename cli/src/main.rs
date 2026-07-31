use lk_core::vm::ModuleResolver;
use lk_core::vm::ProgramExec;
use std::path::{Path, PathBuf};
#[cfg(feature = "aot")]
use std::process::Command;
use std::sync::{Arc, Once};

static PERF_TRACE_INIT: Once = Once::new();
const DEFAULT_TRACE_FILTER: &str = "lk::vm::alloc=trace,lk::vm::slowpath=debug,lk_core=info,lk_cli=info";

use clap::{Parser, Subcommand};
#[cfg(feature = "aot")]
use lk_core::macro_system::{ProcMacroDependencyFingerprint, fingerprint_proc_macro_dependencies};
use lk_core::{
    macro_system::{AstMacroOrigin, MacroTokenOrigin, ProcMacroDependency},
    module::ModuleRegistry,
    package::{PackageGraph, PackageModule},
    stmt::import::collect_program_imports,
    syntax::{expand_program_source, macro_origin_note_for_span, render_program, render_tokens, type_error_span},
    typ::TypeChecker,
    vm::{
        ModuleArtifact, Opcode, VM_INDEX_KEY_METRIC_NAMES, VM_REGISTER_WRITE_SOURCE_NAMES, VmContext, VmRuntimeMetrics,
        compile_program_module_with_ctx, execute_compiled_module_with_ctx, execute_module_artifact_with_ctx,
        execute_program_with_ctx_and_limits, vm_runtime_metrics_enabled, vm_runtime_metrics_reset,
        vm_runtime_metrics_snapshot,
    },
};

use anyhow::Context;

mod bytecode_cache;
mod coverage;
mod diagnostic;
mod fmt;
#[cfg(test)]
mod main_test;
mod mem;
mod native_compile;

/// Counting global allocator backing the byte-accurate memory limit (`mem`).
#[global_allocator]
static GLOBAL_ALLOCATOR: mem::CountingAllocator = mem::CountingAllocator;
mod paths;
mod pkg;
mod repl;
mod repl_completion;
mod repl_tui;
mod startup_trace;
use self::native_compile::*;

use coverage::run_coverage_report;
use fmt::run_fmt;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CompileMode {
    /// Emit a `.lkm` bytecode module. This is an INTERNAL artifact (version-locked
    /// to this build, like Python's `.pyc`), not a distribution format — ship
    /// source or a native executable instead.
    Bytecode,
    /// Emit a native executable (default).
    Exe,
    /// Emit a relocatable object file for a given target, and stop.
    ///
    /// This is the bare-metal path. LK does not link those images itself, and
    /// should not: the linker script, entry point and memory map belong to the
    /// board, not to the language. Emitting an object lets an existing
    /// embedded build (cargo + build.rs + a linker script, or a Makefile) place
    /// it — the same way a C library is consumed.
    Object {
        /// Target triple, e.g. `aarch64-unknown-none`.
        triple: String,
    },
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Compile sources into supported migration targets.
    Compile {
        /// 支持 `lk compile [TARGET] [FILE]`（默认编译 exe；省略 FILE 时自动查找当前目录入口）
        #[arg(value_name = "ARGS", num_args = 0..=2)]
        positional: Vec<String>,
        #[cfg(feature = "aot")]
        /// 输出文件路径（针对默认 exe 目标指定最终可执行文件路径）
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Type-check a source file without executing it.
    Check {
        /// Source file to type-check
        #[arg(value_name = "FILE", value_parser = parse_sanitized_path)]
        file: PathBuf,
    },
    /// Format LK sources in place (4-space indent). Without a path, formats the
    /// whole project (nearest `Lk.toml` directory, else the current directory).
    /// `--check` reports without writing.
    Fmt {
        /// Files or directories to format. Defaults to the whole project.
        #[arg(value_name = "PATH", value_parser = parse_sanitized_path)]
        paths: Vec<PathBuf>,
        /// Do not write; exit non-zero if any file is not already formatted.
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
    eprintln!("{}", vm_profile_report());
}

/// What `LK_VM_PROFILE=1` prints — including when it can't profile.
///
/// The recording sites are `#[cfg]`-gated: without `--features vm-profile` they
/// compile to nothing, so every counter reads 0. This asked the environment
/// variable and nothing else, so a default build answered `LK_VM_PROFILE=1` with
/// a full, well-formed profile in which every single number was fiction —
/// `opcode_steps=0` for a program that had just run four thousand of them.
/// `lk coverage --runtime` was already checking `vm_runtime_metrics_enabled()`;
/// one rule, two carriers, one of them following it.
fn vm_profile_report() -> String {
    if !vm_runtime_metrics_enabled() {
        return "VM profile: unavailable — this binary has no profiling counters compiled in. \
                Rebuild with `cargo build -p lk-cli --features vm-profile`."
            .to_string();
    }
    vm_profile_line(vm_runtime_metrics_snapshot())
}

fn vm_profile_line(metrics: VmRuntimeMetrics) -> String {
    format!(
        "VM profile: opcode_steps={} top_opcodes={} write_sources={} index_keys={} calls={} branches={} typed_branches={} containers={} list_ops={} map_ops={} string_ops={} register_writes={}",
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
        metrics.register_writes,
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

/// Die on `SIGPIPE` like every other Unix filter, instead of panicking.
///
/// Rust sets `SIGPIPE` to `SIG_IGN` before `main`, so a write to a closed pipe
/// comes back as `EPIPE` and `println!` unwraps it into a panic: `lk gen.lk |
/// head` printed `thread 'main' panicked at library/std/src/io/stdio.rs … note:
/// run with RUST_BACKTRACE=1` and exited 101. That is the implementation
/// talking, not the language, and piping into `head` is the most ordinary thing
/// a shell does with a program that prints.
///
/// The AOT-compiled binary was already right — its `main` is a C `main`, so
/// Rust's startup never ran and it died with signal 13 (exit 141), silently.
/// So this is also the two backends disagreeing, with the native one correct.
#[cfg(unix)]
fn restore_default_sigpipe() {
    // SAFETY: sets a signal disposition before anything has been printed and
    // before any thread exists.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn restore_default_sigpipe() {}

fn main() -> anyhow::Result<()> {
    restore_default_sigpipe();
    let mut startup = startup_trace::StartupTrace::new("main");
    mem::configure();
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
                #[cfg(feature = "aot")]
                    output: output_arg,
            } => {
                let (pos_target, safe) = split_compile_args(&positional)?;

                #[cfg(feature = "aot")]
                let output = output_arg
                    .map(|p| {
                        sanitize_path(p.to_string_lossy().as_ref()).inspect_err(|e| {
                            diagnostic::error(e);
                        })
                    })
                    .transpose()?;

                let compile_mode = pos_target;

                #[cfg(feature = "aot")]
                if matches!(compile_mode, CompileMode::Bytecode) && output.is_some() {
                    anyhow::bail!("--output is only supported for `lk compile <FILE>` and `object:<triple>`");
                }

                match compile_mode {
                    CompileMode::Bytecode => {
                        compile_instr_module(&safe)?;
                        return Ok(());
                    }
                    CompileMode::Object { triple } => {
                        #[cfg(not(feature = "aot"))]
                        {
                            let _ = triple;
                            anyhow::bail!(
                                "native backend disabled at build time; rebuild with `--features aot` to emit objects"
                            );
                        }
                        #[cfg(feature = "aot")]
                        {
                            compile_object(&safe, &triple, output.as_deref())?;
                            return Ok(());
                        }
                    }
                    CompileMode::Exe => {
                        #[cfg(not(feature = "aot"))]
                        anyhow::bail!(
                            "native backend disabled at build time; rebuild with `--features aot` to compile native executables"
                        );
                        #[cfg(feature = "aot")]
                        {
                            compile_executable(&safe, output.as_deref())?;
                            return Ok(());
                        }
                    }
                }
            }
            Commands::Check { file } => {
                run_type_check(&file)?;
                return Ok(());
            }
            Commands::Fmt { paths, check } => {
                run_fmt(&paths, check)?;
                return Ok(());
            }
            Commands::Bundle { file, output } => {
                #[cfg(not(feature = "aot"))]
                {
                    let _ = (&file, &output);
                    anyhow::bail!(
                        "bundling links the VM in through lk-api's staticlib, which is part of the native backend; rebuild with `--features aot`"
                    );
                }
                #[cfg(feature = "aot")]
                {
                    run_bundle(&file, &output)?;
                    return Ok(());
                }
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

    #[cfg(feature = "aot")]
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

    // Built before the type check, not after: registering the standard library
    // is what publishes its declared signatures to the checker, and a check that
    // runs first sees only the small fallback table in core — `string.split("a")`
    // would go unchecked here while `lk check` caught it.
    let mut base_env = build_vm_context(&safe)?;

    // Cross-file signatures, checked here rather than inside the VM: the type
    // check `execute_with_ctx` runs has no path to resolve imports against, so
    // this is the only place a running program gets the same checking that
    // `lk check` and `lk compile` give it.
    {
        let mut checker = TypeChecker::new();
        seed_imports(&program, &safe, &mut checker);
        program.type_check(&mut checker)?;
    }

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
            // The directory, not `None`: `execute_with_ctx` type-checks the
            // program *again* with a fresh checker, and one without a path to
            // resolve imports against rejects every name that crosses a module
            // boundary — so `lk check FILE` passed and `lk FILE` answered
            // `Unknown type 'P' in parameter 'p'` for the same file. A program
            // that clears the pre-flight command has to be runnable.
            //
            // The check above stays because it is the only one the sandboxed and
            // cached branches get; that this path now checks twice is a startup
            // cost, not a correctness one.
            None => program.execute_with_ctx_from(&mut base_env, safe.parent()),
        }
    };

    // Shutdown runtime after execution
    base_env.shutdown_async_runtime();

    // Reported here rather than propagated, so a failing program reads the same
    // whichever backend ran it. This used to carry `.with_context("VM execution
    // failed")`, which anyhow rendered as four lines around the real message —
    // and the claim was false for half of what reaches here: the compiler's own
    // errors come out of this `Result` too (`compile_program_module_with_ctx`),
    // and nothing had executed. A native binary prints one `Error: …` line;
    // `an_uncaught_error_exits_and_reads_the_same_on_both_backends` pins the two together.
    let result = match unwrap_with_traceback(exec_result, &base_env) {
        Ok(result) => result,
        Err(err) => {
            diagnostic::error(format!("{err:#}"));
            std::process::exit(1);
        }
    };
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
    ensure_stdlib_signatures();
    let mut checker = TypeChecker::new_strict();
    seed_imports(&expanded.program, path, &mut checker);
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
    // Then compile it, without running it.
    //
    // Type checking answers "do the types agree", not "can this be built": a
    // call to a name that does not exist anywhere passes the type check (the
    // callee is `Any`) and fails in the compiler with `undefined callable`.
    // Without this step `lk check` reported success for a program that could
    // not be compiled at all — the one thing a check command must not do. The
    // compiler is reused rather than a second undefined-name analysis written
    // here, so the two cannot drift apart about what counts as defined.
    let mut ctx = build_vm_context(path)?;
    if let Err(err) = compile_program_module_with_ctx(&expanded.program, &mut ctx) {
        diagnostic::error(anyhow::anyhow!(err.to_string()));
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
/// to a positive integer, exceeding it aborts with a heap-limit error instead
/// of growing unbounded — the memory knob of the sandbox model (plan M2.6). The
/// cap bounds the count of *reachable* objects (a collect-then-recheck reclaims
/// transient churn first, so allocation-heavy-but-low-live-set programs are not
/// tripped), a coarse memory proxy — not bytes; pair with `LK_FUEL` to bound
/// total work/allocation. Absent/0/invalid means unlimited.
fn heap_object_limit_from_env() -> Option<usize> {
    std::env::var("LK_MAX_HEAP_OBJECTS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|&limit| limit > 0)
}

/// AOT Tier 0: bundle `source_path` into a self-contained native executable that
/// embeds the program source and the VM (via lk-api's C-ABI staticlib). 100%
/// coverage — the produced binary just runs the VM at launch, so any program that
/// runs under the VM bundles (unlike the MIR native path). Linux/`cc` for now.
#[cfg(feature = "aot")]
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

/// Both callers (`run_bundle` and `native_compile`'s staticlib builder) are
/// `#[cfg(feature = "aot")]`, so this is too — a helper that outlives the only
/// configuration that calls it is dead code, and CI builds the CLI *without*
/// `aot` (the bare-metal step needs a `lk` that only compiles bytecode).
#[cfg(feature = "aot")]
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
    // A bare `lk main.lk` has an empty parent; that still means "the current
    // directory", and it has to be set — the base directory is what establishes
    // the import containment root, so skipping it disabled containment for
    // exactly the most common invocation.
    let base = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    resolver.set_base_dir(base);
    configure_package_resolver(&mut resolver, path)?;
    let resolver = Arc::new(resolver);
    Ok(VmContext::new()
        .with_resolver(Arc::clone(&resolver))
        .with_type_checker(Some(TypeChecker::new_strict())))
}

/// Publish the standard library's declared signatures to the type checker.
///
/// Registering the modules is what does it — `register_stdlib_module_metadata`
/// forwards each module's signatures to `lk_core::typ`, process-wide. The
/// commands that type-check without running (`lk check`, `lk compile`) never
/// build a `VmContext`, so without this they fall back to the small table core
/// keeps for its own tests and miss everything outside `os`/`env`/`math`.
///
/// The registry is built and dropped; what survives is global. Idempotent.
pub(crate) fn ensure_stdlib_signatures() {
    let mut registry = ModuleRegistry::new();
    let _ = register_enabled_stdlib(&mut registry);
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
/// What `bundle_file_imports` concluded.
#[cfg(feature = "aot")]
pub(crate) enum BundleOutcome {
    /// No file imports to bundle.
    Nothing,
    /// Bundling would change the program's meaning. The string says why, in a
    /// form worth showing a user: the executable path falls back silently, but
    /// `compile object:` has no fallback and would otherwise report a symptom
    /// (an unlowerable `GetGlobal`) rather than a cause.
    Declined(String),
    Bundled(ModuleArtifact, Vec<lk_aot::BundledImport>),
}

#[cfg(feature = "aot")]
/// One validated dependency, held until the merge knows how to number it.
#[cfg(feature = "aot")]
struct PendingBundle {
    import_path: String,
    canonical: PathBuf,
    dep: ModuleArtifact,
    dep_entry: usize,
    /// Exported name → the dep's own function index.
    pairs: Vec<(String, u32)>,
    dep_consts: Vec<(String, BundledConst)>,
}

#[cfg(feature = "aot")]
fn bundle_file_imports(source: &Path, artifact: &ModuleArtifact) -> anyhow::Result<BundleOutcome> {
    use lk_core::vm::{Instr, Opcode};

    let base_dir = source.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
    // Depth-first over the import graph, so a driver may import another
    // driver. Keyed by resolved path rather than by the text of the import,
    // because two files can name the same module differently — and because
    // that is also what makes a cycle terminate.
    // Every renamed item a file import binds, from any module in the bundle.
    //
    // A bundled module's constants fold into their reads by *slot*, and the slot
    // is the constant's own name. `use { SIZE as TSS_SIZE }` reads a different
    // name, so the fold missed it and the read survived as a `GetGlobal` of a
    // slot nothing initialises — "does not resolve to anything natively
    // lowerable" under `compile object:`, a fall back to the VM otherwise. The
    // VM binds it, so the two backends differed in coverage.
    //
    // Collected from every module because a driver may rename another driver's
    // constant, which is where this would have been found rather than reasoned
    // about.
    let mut renamed_items: Vec<(String, String)> = Vec::new();
    collect_renamed_file_items(&artifact.imports, &mut renamed_items);
    let mut queue: Vec<(String, PathBuf)> = file_import_paths(&artifact.imports)
        .into_iter()
        .map(|path| resolve_bundled_import(&base_dir, &path).map(|resolved| (path, resolved)))
        .collect::<anyhow::Result<Vec<_>>>()?;
    if queue.is_empty() {
        return Ok(BundleOutcome::Nothing);
    }
    // Keyed by resolved path, and remembering what each one exported: a file
    // reached twice is merged once, but *both* import paths still need a
    // binding table. `drivers/ata` imported by the program and `ata` imported
    // by a driver next to it are the same file under two names; recording only
    // the first left the second one's `use { .. }` resolving to nothing, which
    // the lowering reports as an unresolved global far from the cause.
    let mut bundled_fns: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    // Every constant any bundled module defined, and which module defined it.
    // Two deps exporting the same name would both fold into one merged slot,
    // and the first one popped off the queue would win for the importer's
    // reads — silently, and depending on traversal order. Under the VM each
    // module keeps its own namespace, so that is a divergence rather than a
    // preference.
    let mut bundled_consts: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    let mut merged = artifact.clone();
    let mut bundles: Vec<lk_aot::BundledImport> = Vec::new();
    // Validated deps, waiting to be numbered. Nothing is appended inside the
    // loop: which merged index each function gets depends on every dep, so the
    // numbering is decided once, afterwards.
    let mut pending: Vec<PendingBundle> = Vec::new();
    // Import paths naming a file some other path already brought in. They add
    // no functions, only a binding table — which does not exist until the
    // numbering does.
    let mut aliases: Vec<(String, PathBuf)> = Vec::new();
    while let Some((import_path, dep_path)) = queue.pop() {
        let canonical = std::fs::canonicalize(&dep_path).unwrap_or_else(|_| dep_path.clone());
        if bundled_fns.contains(&canonical) {
            aliases.push((import_path, canonical));
            continue;
        }
        bundled_fns.insert(canonical.clone());

        let dep = compile_instr_artifact_with_dependencies(&dep_path)?.artifact;
        // Bundling this module would give its functions a *reference* to the
        // caller's containers where the VM hands them a copy. Rather than
        // produce a program that computes something the VM would not, decline
        // to bundle: the caller falls back, and `compile object:` — which has
        // no fallback — reports it.
        if module_may_mutate_a_parameter(&dep.module) {
            return Ok(BundleOutcome::Declined(format!(
                "'{import_path}' has a function that writes through a container parameter, keeps one, \
                 or calls a method on one. Bundling would hand it the caller's container by reference, \
                 while the VM gives each module its own copy — so the two would disagree"
            )));
        }
        // The dep's own file imports resolve relative to *its* directory, not
        // the importing file's.
        let dep_dir = dep_path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
        collect_renamed_file_items(&dep.imports, &mut renamed_items);
        for nested in file_import_paths(&dep.imports) {
            let resolved = resolve_bundled_import(&dep_dir, &nested)
                .with_context(|| format!("nested import of '{import_path}'"))?;
            queue.push((nested, resolved));
        }
        let dep_entry = dep.module.entry as usize;
        // The dep entry must be pure `fn` bookkeeping: LoadFunction+SetGlobal
        // pairs and the implicit return. Anything else is a top-level effect
        // the bundle would silently skip — reject instead.
        // The dep's entry must be pure binding: `fn` definitions, which the
        // merge performs directly, and scalar constants, which fold into their
        // uses. Anything else is rejected — see the container case below for
        // why this restriction is what keeps the two backends agreeing.
        let mut reg_fn: std::collections::HashMap<u8, u32> = std::collections::HashMap::new();
        let mut reg_const: std::collections::HashMap<u8, BundledConst> = std::collections::HashMap::new();
        let mut pairs: Vec<(String, u32)> = Vec::new();
        let mut dep_consts: Vec<(String, BundledConst)> = Vec::new();
        let dep_entry_fn = &dep.module.functions[dep_entry];
        for raw_instr in &dep_entry_fn.code {
            let instr = Instr::try_from_raw(*raw_instr)
                .map_err(|_| anyhow::anyhow!("bundled import '{import_path}': bad instruction"))?;
            match instr.opcode() {
                Opcode::LoadFunction => {
                    reg_fn.insert(instr.a(), u32::from(instr.bx()));
                }
                Opcode::LoadInt => {
                    let value = dep_entry_fn
                        .consts
                        .ints
                        .get(instr.bx() as usize)
                        .copied()
                        .ok_or_else(|| anyhow::anyhow!("bundled import '{import_path}': bad int constant"))?;
                    reg_const.insert(instr.a(), BundledConst::Int(value));
                }
                Opcode::LoadFloat => {
                    let value = dep_entry_fn
                        .consts
                        .floats
                        .get(instr.bx() as usize)
                        .copied()
                        .ok_or_else(|| anyhow::anyhow!("bundled import '{import_path}': bad float constant"))?;
                    reg_const.insert(instr.a(), BundledConst::Float(value));
                }
                Opcode::LoadString => {
                    let value = dep_entry_fn
                        .consts
                        .strings
                        .get(instr.bx() as usize)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("bundled import '{import_path}': bad string constant"))?;
                    reg_const.insert(instr.a(), BundledConst::Str(value));
                }
                Opcode::LoadBool => {
                    reg_const.insert(instr.a(), BundledConst::Bool(instr.b() != 0));
                }
                Opcode::LoadNil => {
                    reg_const.insert(instr.a(), BundledConst::Nil);
                }
                Opcode::SetGlobal => {
                    let name = dep.module.globals.get(instr.bx() as usize).cloned().unwrap_or_default();
                    if let Some(&fidx) = reg_fn.get(&instr.a()) {
                        pairs.push((name, fidx));
                    } else if let Some(value) = reg_const.get(&instr.a()) {
                        dep_consts.push((name, value.clone()));
                    } else {
                        anyhow::bail!(
                            "bundled import '{import_path}' has a top-level binding that is neither a function \
                             nor a scalar constant"
                        );
                    }
                }
                // A constant derived from constants.
                //
                // A module's top level is already required to be effect-free —
                // that is what everything else in this scan enforces. What was
                // missing was the ability to *evaluate* a pure one, so
                // `const FRAME = HEADER + BODY;` was rejected as an effect
                // while `const FRAME = 42;` was not. Deriving one constant from
                // two others is the ordinary shape of a protocol header, and
                // the alternative is the same number written twice.
                //
                // Folded rather than deferred: the value has to be known here,
                // because what crosses the bundle boundary is a value and not
                // an expression — `rewrite_bundled_globals` replaces each
                // `GetGlobal` in the importer with a load.
                Opcode::GetGlobal => {
                    let name = dep.module.globals.get(instr.bx() as usize).cloned().unwrap_or_default();
                    match dep_consts.iter().rev().find(|(known, _)| *known == name) {
                        Some((_, value)) => {
                            reg_const.insert(instr.a(), value.clone());
                        }
                        None => anyhow::bail!(
                            "bundled import '{import_path}' reads `{name}` at its top level, which is not a constant defined above it"
                        ),
                    }
                }
                Opcode::Move => {
                    if let Some(value) = reg_const.get(&instr.b()).cloned() {
                        reg_const.insert(instr.a(), value);
                    } else if let Some(&fidx) = reg_fn.get(&instr.b()) {
                        reg_fn.insert(instr.a(), fidx);
                    } else {
                        anyhow::bail!(
                            "bundled import '{import_path}' moves a top-level register that holds neither a function nor a constant"
                        )
                    }
                }
                // Integer arithmetic on values already known.
                //
                // These three and their immediate forms, and deliberately not
                // division: `/` is float division in LK, so an integer divide
                // here is a cast the front end proved, and matching its exact
                // truncation and its behaviour at zero is a second
                // implementation of a thing worth having only one of. A header
                // constant that needs one gets the diagnostic below, naming the
                // opcode.
                //
                // Wrapping, because that is what the executor does — a fold
                // that panicked where the VM wrapped would be a compiler that
                // rejects a program the VM runs.
                Opcode::AddInt | Opcode::SubInt | Opcode::MulInt => {
                    let lhs = int_operand(&reg_const, instr.b(), &import_path)?;
                    let rhs = int_operand(&reg_const, instr.c(), &import_path)?;
                    let value = match instr.opcode() {
                        Opcode::AddInt => lhs.wrapping_add(rhs),
                        Opcode::SubInt => lhs.wrapping_sub(rhs),
                        _ => lhs.wrapping_mul(rhs),
                    };
                    reg_const.insert(instr.a(), BundledConst::Int(value));
                }
                Opcode::AddIntI | Opcode::MulIntI => {
                    let lhs = int_operand(&reg_const, instr.b(), &import_path)?;
                    let rhs = instr.sc() as i64;
                    let value = if instr.opcode() == Opcode::AddIntI {
                        lhs.wrapping_add(rhs)
                    } else {
                        lhs.wrapping_mul(rhs)
                    };
                    reg_const.insert(instr.a(), BundledConst::Int(value));
                }
                Opcode::Return0 => {}
                // A container at a module's top level cannot cross this
                // boundary and keep the VM's meaning.
                //
                // Bundling *flattens* the modules into one, so a container the
                // module exposes becomes shared with the importer. The VM
                // gives each module its own heap and copies a container that
                // crosses the boundary — measurably: with `const NAMES = […]`
                // in a module, `let xs = get(); xs.push(…)` changes what the
                // module sees under a flattened build and does not under the
                // VM. Rejecting here is what keeps the two backends agreeing;
                // it is not an arbitrary limit on what a module may hold.
                Opcode::LoadHeapConst => anyhow::bail!(
                    "bundled import '{import_path}' has a container at its top level. Bundling flattens \
                     modules together, which would share it with the importer, while the VM gives each \
                     module its own copy. Move it to the importing file, or build it inside a function."
                ),
                other => {
                    anyhow::bail!("bundled import '{import_path}' has top-level effects (opcode {other:?})")
                }
            }
        }

        // A binding the *main* module also writes would leave two definitions
        // sharing one merged slot, because the merge maps globals by name.
        // Refuse rather than pick one.
        for (name, _) in &dep_consts {
            if let Some(previous) = bundled_consts.get(name) {
                anyhow::bail!(
                    "bundled imports '{previous}' and '{import_path}' both define `{name}`. Bundling merges \
                     them into one slot, so one definition would silently win; the VM gives each module its \
                     own. Rename one of them."
                );
            }
            bundled_consts.insert(name.clone(), import_path.clone());
            if let Some(slot) = artifact.module.globals.iter().position(|g| g == name) {
                let main_entry = artifact.module.entry as usize;
                let written = artifact.module.functions[main_entry].code.iter().any(|raw| {
                    Instr::try_from_raw(*raw)
                        .map(|i| i.opcode() == Opcode::SetGlobal && i.bx() as usize == slot)
                        .unwrap_or(false)
                });
                if written {
                    anyhow::bail!(
                        "bundled import '{import_path}' defines `{name}`, which the importing file also defines"
                    );
                }
            }
        }

        // Nothing is merged yet: the numbering the merge hands out depends on
        // every dep, so it is decided once, after all of them are known.
        pending.push(PendingBundle {
            import_path,
            canonical,
            dep,
            dep_entry,
            pairs,
            dep_consts,
        });
    }

    // The numbering, and the reason it is not simply "append in the order they
    // arrived".
    //
    // A `CallDirect` or `MakeClosure` names its target in the instruction's `b`
    // field, which is a byte. A dep's instructions are already emitted by the
    // time they reach here — rewriting one into two would move every jump
    // offset after it — so any dep function that one of those names has to land
    // below 256. Appending in arrival order made that a bound on the *whole*
    // program, and `bare-metal-x86/program.lk` with its drivers hit it at 260.
    //
    // But most functions are not named that way. Of 159 driver functions there,
    // 52 are: the rest are reached by name from the importing program, which
    // the lowering resolves through `BundledImport::fns` — a `u32`. So the
    // targets go first and the bound becomes "the importing file's functions,
    // plus the ones a dep calls directly", which for that program is 144.
    //
    // What still has no answer is a program that crosses *that*. The honest fix
    // is a wider field, and that is an instruction-encoding change.
    let mut targets: Vec<std::collections::HashSet<usize>> = Vec::with_capacity(pending.len());
    for entry in &pending {
        let mut set = std::collections::HashSet::new();
        for (index, function) in entry.dep.module.functions.iter().enumerate() {
            if index == entry.dep_entry {
                continue;
            }
            for raw in &function.code {
                let instr = Instr::try_from_raw(*raw)
                    .map_err(|_| anyhow::anyhow!("bundled import '{}': bad instruction", entry.import_path))?;
                if matches!(instr.opcode(), Opcode::CallDirect | Opcode::MakeClosure) {
                    set.insert(instr.b() as usize);
                }
            }
        }
        targets.push(set);
    }

    let base = merged.module.functions.len() as u32;
    let mut remaps: Vec<Vec<Option<u32>>> = pending
        .iter()
        .map(|entry| vec![None; entry.dep.module.functions.len()])
        .collect();
    let mut next = base;
    // Directly-called functions first, across every dep, then everything else.
    #[allow(
        clippy::needless_range_loop,
        reason = "the bound is the dep module's function count, not `remaps`' length"
    )]
    for directly_called in [true, false] {
        for (which, entry) in pending.iter().enumerate() {
            for index in 0..entry.dep.module.functions.len() {
                if index == entry.dep_entry || targets[which].contains(&index) != directly_called {
                    continue;
                }
                remaps[which][index] = Some(next);
                next += 1;
            }
        }
    }

    // Laid out by merged index rather than pushed as they are rewritten: the
    // two passes above interleave the deps, so arrival order is no longer
    // append order.
    let mut placed: Vec<Option<lk_core::vm::FunctionData>> = (base..next).map(|_| None).collect();
    let mut canonical_fns: std::collections::HashMap<PathBuf, std::collections::HashMap<String, u32>> =
        std::collections::HashMap::new();
    let mut all_consts: Vec<(String, Vec<(String, BundledConst)>)> = Vec::new();
    for (which, entry) in pending.into_iter().enumerate() {
        let PendingBundle {
            import_path,
            canonical,
            dep,
            dep_entry,
            pairs,
            dep_consts,
        } = entry;
        let remap = &remaps[which];
        let slot_of = |name: &str, globals: &mut Vec<String>| -> u16 {
            match globals.iter().position(|g| g == name) {
                Some(slot) => slot as u16,
                None => {
                    globals.push(name.to_string());
                    (globals.len() - 1) as u16
                }
            }
        };
        for (index, function) in dep.module.functions.iter().enumerate() {
            if index == dep_entry {
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
                        // The numbering above put every one of these below 256.
                        // Reaching here means the *importing* file plus every
                        // directly-called dep function came to more than that,
                        // which is the ceiling this layout postponed rather
                        // than removed.
                        let new = u8::try_from(new).map_err(|_| {
                            anyhow::anyhow!(
                                "bundled import '{import_path}': more than 256 directly-called functions — \
                                 the call instruction names its target in a byte"
                            )
                        })?;
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
            let at = remap[index].expect("every non-entry function was numbered") - base;
            placed[at as usize] = Some(function);
        }
        // The dep's `impl` blocks come across too, with their method indices
        // rewritten by the same remap.
        //
        // Without this the merged artifact had the *functions* of an imported
        // `impl` but no record that they implement anything, so the AOT's trait
        // environment (`trait_env_prescan`, which reads `type_info.impls`) could
        // not see them: `types.make(3, 4).norm()` fell out of the native subset
        // — every cross-module method call did — while the same code inside the
        // defining module lowered fine.
        //
        // A type declared in two bundled modules under one name would now share
        // a dispatch key. The VM keeps them apart by `TypeScope`, so this
        // refuses rather than resolving, by the rule the rest of this bundler
        // follows.
        for decl in &dep.module.type_info.impls {
            let mut rewritten = decl.clone();
            for method in &mut rewritten.methods {
                method.function = remap
                    .get(method.function as usize)
                    .copied()
                    .flatten()
                    .ok_or_else(|| anyhow::anyhow!("bundled import '{import_path}': dangling impl method"))?;
            }
            if let Some(existing) = merged
                .module
                .type_info
                .impls
                .iter()
                .find(|other| other.type_name == rewritten.type_name && other.trait_name == rewritten.trait_name)
                && existing.methods != rewritten.methods
            {
                anyhow::bail!(
                    "bundled import '{import_path}': type `{}` is implemented in more than one module —                      the VM keeps them apart by declaring module, the bundle cannot",
                    rewritten.type_name
                );
            }
            merged.module.type_info.impls.push(rewritten);
        }
        let mut fns: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        for (name, fidx) in pairs {
            let merged_fidx = remap
                .get(fidx as usize)
                .copied()
                .flatten()
                .ok_or_else(|| anyhow::anyhow!("bundled import '{import_path}': dangling fn binding"))?;
            fns.insert(name, merged_fidx);
        }
        canonical_fns.insert(canonical, fns.clone());
        bundles.push(lk_aot::BundledImport {
            path: import_path.clone(),
            fns,
        });
        if !dep_consts.is_empty() {
            all_consts.push((import_path, dep_consts));
        }
    }
    for (which, function) in placed.into_iter().enumerate() {
        merged.module.functions.push(
            function
                .ok_or_else(|| anyhow::anyhow!("bundled merge left function {} unfilled", base as usize + which))?,
        );
    }

    // A second import path for a file already merged: same functions, its own
    // binding table.
    for (import_path, canonical) in aliases {
        let fns = canonical_fns
            .get(&canonical)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("bundled import '{import_path}': no merged module to bind to"))?;
        bundles.push(lk_aot::BundledImport { path: import_path, fns });
    }

    // A bundled module's constants have no initialiser in the merged program:
    // its entry — the only code that would have run the assignment — is the one
    // function the merge drops. Rather than splice an initialiser into the
    // importing entry (which would shift every pc and invalidate the pc-keyed
    // facts), fold the value into each read. They are constants; substituting
    // them is what `const` means.
    //
    // After every function is in place, which is also a fix: folding used to
    // happen as each dep landed, so a dep merged *later* had its reads of an
    // earlier dep's constant left as a `GetGlobal` of a slot nothing ever
    // initialises. Nothing depended on that yet — a driver importing another
    // driver's constant is what would have found it.
    for (import_path, dep_consts) in all_consts {
        let slot_of = |name: &str, globals: &mut Vec<String>| -> u16 {
            match globals.iter().position(|g| g == name) {
                Some(slot) => slot as u16,
                None => {
                    globals.push(name.to_string());
                    (globals.len() - 1) as u16
                }
            }
        };
        let mut const_slots: std::collections::HashMap<u16, BundledConst> = std::collections::HashMap::new();
        for (name, value) in dep_consts {
            // The name every module that did *not* rename it reads.
            const_slots.insert(slot_of(&name, &mut merged.module.globals), value.clone());
            // And every name one that did. Skipped when something writes the
            // alias's slot: a module of its own with that name shadows the
            // import, which is what the VM does, and folding would answer the
            // constant where the VM answers the variable.
            for (alias, _) in renamed_items.iter().filter(|(_, original)| *original == name) {
                let slot = slot_of(alias, &mut merged.module.globals);
                let written = merged.module.functions.iter().any(|function| {
                    function.code.iter().any(|raw| {
                        Instr::try_from_raw(*raw)
                            .map(|i| i.opcode() == Opcode::SetGlobal && i.bx() == slot)
                            .unwrap_or(false)
                    })
                });
                if !written {
                    const_slots.insert(slot, value.clone());
                }
            }
        }
        for function in &mut merged.module.functions {
            fold_global_constants(function, &const_slots)
                .with_context(|| format!("bundled import '{import_path}': folding constants"))?;
        }
    }
    Ok(BundleOutcome::Bundled(merged, bundles))
}

/// The `use { name as alias } from "path"` bindings a module declares, as
/// `(alias, name)`. Only renamed ones: an unrenamed item already reads the name
/// the bundle flattened it under.
#[cfg(feature = "aot")]
fn collect_renamed_file_items(imports: &[lk_core::stmt::ImportStmt], out: &mut Vec<(String, String)>) {
    use lk_core::stmt::{ImportSource, ImportStmt};
    for import in imports {
        if let ImportStmt::Items {
            items,
            source: ImportSource::File(_),
        } = import
        {
            for item in items {
                if let Some(alias) = &item.alias
                    && alias != &item.name
                {
                    out.push((alias.clone(), item.name.clone()));
                }
            }
        }
    }
}

/// The file imports (`use "path"` in any of its forms) a module declares.
#[cfg(feature = "aot")]
fn file_import_paths(imports: &[lk_core::stmt::ImportStmt]) -> Vec<String> {
    use lk_core::stmt::{ImportSource, ImportStmt};
    let mut paths = Vec::new();
    for import in imports {
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
    paths
}

/// Mirrors the runtime resolver's candidates: `p` (already `.lk`), `p.lk`, and
/// `p/mod.lk`, under the importing file's directory.
#[cfg(feature = "aot")]
fn resolve_bundled_import(base_dir: &Path, import_path: &str) -> anyhow::Result<PathBuf> {
    let raw = Path::new(import_path);
    let mut candidates = Vec::new();
    if raw.extension().and_then(|e| e.to_str()) == Some("lk") {
        candidates.push(base_dir.join(raw));
    }
    candidates.push(base_dir.join(raw.with_extension("lk")));
    candidates.push(base_dir.join(raw).join("mod.lk"));
    candidates
        .into_iter()
        .find(|candidate| candidate.exists())
        .ok_or_else(|| anyhow::anyhow!("bundled import not found: {import_path}"))
}

/// Whether a method provably neither writes through its receiver nor keeps it.
///
/// An allow list, and short on purpose: everything not named here is assumed to
/// write, which is the same default the rest of this scan takes. Each of these
/// answers a number or a bool computed from the receiver's current contents and
/// holds on to nothing.
///
/// `user_methods` is what makes the list safe rather than a guess. A name an
/// `impl` in this module defines could dispatch to anything — a type may have
/// its own `contains` that sorts first — so a name that is also a user method is
/// not treated as the builtin it resembles.
#[cfg(feature = "aot")]
fn reads_only(name: &str, user_methods: &std::collections::HashSet<&str>) -> bool {
    //
    // Every name here has to be a method the language actually has, or the
    // entry is a comment that looks like code: `char_at` and `find` sat in this
    // list long after one became `get` (the accessor every sequence spells) and
    // the other `index_of`, so neither had matched anything for as long as it
    // had been written. `get` does *not* replace `char_at` here — on a list it
    // answers an element, and an element can be a handle into the receiver,
    // which is exactly the "keeps it" case this list excludes.
    const PURE_READS: &[&str] = &[
        "len",
        "byte_at",
        "starts_with",
        "ends_with",
        "contains",
        "index_of",
        "count",
        "is_empty",
    ];
    PURE_READS.contains(&name) && !user_methods.contains(name)
}

/// Whether any function in a module can mutate or retain a container that came
/// in as a parameter.
///
/// Bundling flattens the modules into one program, so an argument reaches the
/// callee by reference. The VM runs them as separate modules with separate
/// heaps and *copies* arguments across the boundary — deliberately: see
/// `copy_runtime_positional_args_to_frame` and the `cross_heap` tests. The two
/// therefore disagree the moment a callee writes through a parameter, and the
/// disagreement is silent:
///
/// ```lk
/// // m.lk:  fn put(xs: List<Int>, i: Int, v: Int) { xs[i] = v; }
/// let xs = [0, 0, 0];
/// put(xs, 0, 7);
/// xs[0]        // VM: 0 (the module got a copy).  Bundled: 7.
/// ```
///
/// So a module that might do this is not bundled at all. The caller then falls
/// back (or, for `compile object:`, reports it), which is the outcome that
/// cannot be wrong.
///
/// Reads are fine and stay bundlable: indexing, iterating, `len`. What counts
/// as unsafe is writing through a parameter, storing one in a global, calling
/// a method on one (the method table is not enumerated here, so an unknown
/// method is assumed to mutate), or passing one to a function that does — the
/// last of which is why this is a fixpoint over the module's own functions.
#[cfg(feature = "aot")]
fn module_may_mutate_a_parameter(module: &lk_core::vm::ModuleData) -> bool {
    use lk_core::vm::{Instr, Opcode};

    // `unsafe_params[f][i]`: function `f` may mutate or retain its parameter
    // `i`. Grows monotonically, so the fixpoint terminates.
    let mut unsafe_params: Vec<Vec<bool>> = module
        .functions
        .iter()
        .map(|function| vec![false; function.param_count as usize])
        .collect();

    // Only a *container* parameter can be aliased into the caller — a scalar is
    // copied either way. The bytecode carries no parameter types, so
    // container-ness is read off the operations: a register a container opcode
    // touches is one. Over-approximate on purpose; guessing "container" for
    // something that is not costs a needlessly unbundled module, and guessing
    // the other way costs a wrong answer.
    let container_regs: Vec<std::collections::HashSet<u8>> = module
        .functions
        .iter()
        .map(|function| {
            let mut regs = std::collections::HashSet::new();
            for raw in &function.code {
                let Ok(instr) = Instr::try_from_raw(*raw) else {
                    continue;
                };
                if matches!(
                    instr.opcode(),
                    Opcode::GetIndex
                        | Opcode::GetList
                        | Opcode::SetIndex
                        | Opcode::SetIndexStrI
                        | Opcode::GetIndexStrI
                        | Opcode::GetFieldK
                        | Opcode::SetFieldK
                        | Opcode::ListPush
                        | Opcode::ToIter
                        | Opcode::Len
                        | Opcode::Contains
                        | Opcode::SliceFrom
                        | Opcode::CallMethodK
                ) {
                    regs.insert(instr.a());
                    regs.insert(instr.b());
                    regs.insert(instr.c());
                }
            }
            regs
        })
        .collect();

    // Every method name an `impl` in this module defines.
    //
    // A name in here is not necessarily the builtin it looks like: nothing stops
    // a type from having its own `len` that rearranges the receiver, and the
    // bytecode carries no types, so a name that could dispatch to user code has
    // to be treated as if it does.
    let user_methods: std::collections::HashSet<&str> = module
        .type_info
        .impls
        .iter()
        .flat_map(|decl| decl.methods.iter().map(|method| method.name.as_str()))
        .collect();

    loop {
        let mut changed = false;
        for (fi, function) in module.functions.iter().enumerate() {
            // A parameter arrives in the register with its own index.
            let mut tainted: std::collections::HashMap<u8, usize> = (0..function.param_count)
                .filter_map(|i| u8::try_from(i).ok().map(|reg| (reg, i as usize)))
                .filter(|(reg, _)| container_regs[fi].contains(reg))
                .collect();
            let mark = |slot: usize, unsafe_params: &mut Vec<Vec<bool>>, changed: &mut bool| {
                if let Some(flag) = unsafe_params[fi].get_mut(slot)
                    && !*flag
                {
                    *flag = true;
                    *changed = true;
                }
            };
            for raw in &function.code {
                let Ok(instr) = Instr::try_from_raw(*raw) else {
                    continue;
                };
                match instr.opcode() {
                    // A handle copied into another register carries the taint;
                    // anything else that writes a register produces a *new*
                    // value, so it does not.
                    Opcode::Move => {
                        if let Some(&slot) = tainted.get(&instr.b()) {
                            tainted.insert(instr.a(), slot);
                        } else {
                            tainted.remove(&instr.a());
                        }
                    }
                    // Writes through the container in `a`.
                    Opcode::SetIndex | Opcode::SetIndexStrI | Opcode::SetFieldK | Opcode::ListPush => {
                        if let Some(&slot) = tainted.get(&instr.a()) {
                            mark(slot, &mut unsafe_params, &mut changed);
                        }
                    }
                    // Outliving the call is as observable as mutating.
                    Opcode::SetGlobal => {
                        if let Some(&slot) = tainted.get(&instr.a()) {
                            mark(slot, &mut unsafe_params, &mut changed);
                        }
                    }
                    // The receiver is `a`. `len` and indexing are opcodes of
                    // their own, so what reaches here is the long tail — and
                    // the safe assumption about a method this does not know is
                    // that it writes.
                    //
                    // Except for the ones that provably do not. What bundling
                    // changes is that the module gets the caller's container
                    // rather than a copy of it, and that difference is
                    // observable only through a *write* — either this function's
                    // own, or someone else's through a handle it kept. A method
                    // that reads and returns a number can do neither.
                    //
                    // Without this, a bundled module could not have a function
                    // that takes a `String` and looks at it: strings are
                    // immutable, so every string method is a read, and
                    // `fn log(message: String)` in a driver is the most ordinary
                    // thing there is. It was `uart_text(text: String)` calling
                    // `text.byte_at(i)` that found this.
                    Opcode::CallMethodK => {
                        // `b`, not `bx`: the receiver is `a` and the argument
                        // count is `c`, so the method-name constant only has a
                        // byte to live in.
                        let name = function
                            .consts
                            .strings
                            .get(instr.b() as usize)
                            .map(|s| s.as_ref())
                            .unwrap_or("");
                        if !reads_only(name, &user_methods)
                            && let Some(&slot) = tainted.get(&instr.a())
                        {
                            mark(slot, &mut unsafe_params, &mut changed);
                        }
                    }
                    // A direct call passes registers `b+1..b+1+argc`; taint
                    // flows to the callee's parameter of the same position.
                    Opcode::CallDirect => {
                        let callee = instr.b() as usize;
                        let base = instr.a();
                        let argc = instr.c() as usize;
                        for arg in 0..argc {
                            let Some(reg) = base.checked_add(1).and_then(|r| r.checked_add(arg as u8)) else {
                                continue;
                            };
                            let Some(&slot) = tainted.get(&reg) else {
                                continue;
                            };
                            if unsafe_params.get(callee).and_then(|p| p.get(arg)).copied() == Some(true) {
                                mark(slot, &mut unsafe_params, &mut changed);
                            }
                        }
                    }
                    // An indirect call could be anything, including a closure
                    // that keeps the handle.
                    Opcode::Call | Opcode::CallNamed => {
                        let base = instr.a();
                        for offset in 1..=instr.c() {
                            if let Some(reg) = base.checked_add(offset)
                                && let Some(&slot) = tainted.get(&reg)
                            {
                                mark(slot, &mut unsafe_params, &mut changed);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if !changed {
            break;
        }
    }

    unsafe_params.iter().any(|params| params.iter().any(|flag| *flag))
}

/// A scalar a bundled module binds at its top level.
#[cfg(feature = "aot")]
#[derive(Clone, Debug)]
enum BundledConst {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Nil,
}

/// One integer operand of a top-level fold, or a diagnostic naming what it was.
///
/// A register holding a float or a string here is not a bug in the scan — it is
/// a module whose top level does arithmetic this does not evaluate, and saying
/// which register held what is the difference between "fix your constant" and
/// "the bundler is broken".
#[cfg(feature = "aot")]
fn int_operand(
    reg_const: &std::collections::HashMap<u8, BundledConst>,
    reg: u8,
    import_path: &str,
) -> anyhow::Result<i64> {
    match reg_const.get(&reg) {
        Some(BundledConst::Int(value)) => Ok(*value),
        Some(other) => anyhow::bail!(
            "bundled import '{import_path}' does integer arithmetic at its top level on a {other:?}, which is not a constant this can evaluate"
        ),
        None => anyhow::bail!(
            "bundled import '{import_path}' does integer arithmetic at its top level on a value that is not a constant"
        ),
    }
}

/// Rewrites every `GetGlobal` of a bundled constant into a load of its value.
///
/// One instruction replaces one instruction, so pcs — and the facts keyed by
/// them — are untouched. The value goes into the reading function's own
/// constant pool, since pools are per function.
#[cfg(feature = "aot")]
fn fold_global_constants(
    function: &mut lk_core::vm::FunctionData,
    const_slots: &std::collections::HashMap<u16, BundledConst>,
) -> anyhow::Result<()> {
    use lk_core::vm::{Instr, Opcode};

    for index in 0..function.code.len() {
        let Ok(instr) = Instr::try_from_raw(function.code[index]) else {
            continue;
        };
        if instr.opcode() != Opcode::GetGlobal {
            continue;
        }
        let Some(value) = const_slots.get(&instr.bx()) else {
            continue;
        };
        let replacement = match value {
            BundledConst::Int(value) => {
                let slot = pool_index(&mut function.consts.ints, *value)?;
                Instr::abx(Opcode::LoadInt, instr.a(), slot)
            }
            BundledConst::Float(value) => {
                let slot = pool_index_by(&mut function.consts.floats, *value, |a, b| a.to_bits() == b.to_bits())?;
                Instr::abx(Opcode::LoadFloat, instr.a(), slot)
            }
            BundledConst::Str(value) => {
                let slot = pool_index(&mut function.consts.strings, value.clone())?;
                Instr::abx(Opcode::LoadString, instr.a(), slot)
            }
            BundledConst::Bool(value) => Instr::abc(Opcode::LoadBool, instr.a(), u8::from(*value), 0),
            BundledConst::Nil => Instr::abc(Opcode::LoadNil, instr.a(), 0, 0),
        };
        function.code[index] = replacement.raw();
    }
    Ok(())
}

#[cfg(feature = "aot")]
fn pool_index<T: PartialEq>(pool: &mut Vec<T>, value: T) -> anyhow::Result<u16> {
    pool_index_by(pool, value, |a, b| a == b)
}

#[cfg(feature = "aot")]
fn pool_index_by<T>(pool: &mut Vec<T>, value: T, eq: impl Fn(&T, &T) -> bool) -> anyhow::Result<u16> {
    if let Some(index) = pool.iter().position(|existing| eq(existing, &value)) {
        return u16::try_from(index).map_err(|_| anyhow::anyhow!("constant pool index overflow"));
    }
    pool.push(value);
    u16::try_from(pool.len() - 1).map_err(|_| anyhow::anyhow!("constant pool index overflow"))
}

/// Gives the checker the signatures of everything `path`'s program imports
/// from other files.
///
/// Without it those names are `Any`, and a call across a module boundary is
/// not checked at all — the error surfaces much later, from the native
/// lowering, naming an opcode rather than the call.
fn seed_imports(program: &lk_core::stmt::Program, path: &Path, checker: &mut TypeChecker) {
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    lk_core::typ::seed_imported_signatures(program, base_dir, checker);
}
