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
    rt,
    stmt::{ModuleResolver, import::collect_program_imports},
    syntax::{
        expand_program_source, macro_origin_note_for_span, parse_program_source, render_program, render_tokens,
        type_error_span,
    },
    typ::TypeChecker,
    vm::{
        ModuleArtifact, Opcode, VM_INDEX_KEY_METRIC_NAMES, VM_REGISTER_WRITE_SOURCE_NAMES, VmContext, VmRuntimeMetrics,
        compile_program_module_with_ctx, execute_module_artifact_with_ctx, vm_runtime_metrics_reset,
        vm_runtime_metrics_snapshot,
    },
};
#[cfg(feature = "llvm")]
use lk_llvm::{LlvmBackendOptions, OptLevel};

use anyhow::Context;

mod coverage;
mod diagnostic;
#[cfg(test)]
mod main_test;
mod paths;
mod pkg;
mod repl;
mod repl_completion;
mod repl_tui;
mod startup_trace;

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
    Bytecode,
    Llvm,
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
    Fetch {
        /// Resolve registry dependencies from the local index cache without network requests.
        #[arg(long)]
        offline: bool,
    },
    /// Update one dependency or all dependencies.
    Update {
        name: Option<String>,
        /// Resolve registry dependencies from the local index cache without network requests.
        #[arg(long)]
        offline: bool,
    },
    /// Validate package graph and macro provider distribution metadata.
    Check,
    /// Publish a registry manifest, or print it with --dry-run.
    Publish {
        /// Print the registry publish manifest without uploading.
        #[arg(long)]
        dry_run: bool,
    },
    /// Yank or un-yank a registry package version.
    Yank {
        /// Package name to yank.
        name: String,
        /// Package version to yank.
        version: String,
        /// Reverse a previous yank.
        #[arg(long)]
        undo: bool,
    },
    /// Manage the local registry index cache.
    Index {
        #[command(subcommand)]
        command: PkgIndexCommand,
    },
    /// Manage registry signing keys.
    Key {
        #[command(subcommand)]
        command: PkgKeyCommand,
    },
    /// Serve a local LK package registry.
    Serve {
        /// Address to bind, for example 127.0.0.1:3899.
        #[arg(long, default_value = "127.0.0.1:3899")]
        addr: String,
        /// Durable registry storage directory.
        #[arg(long, value_parser = parse_sanitized_path)]
        storage: PathBuf,
        /// Public registry URL clients will validate in publish manifests.
        #[arg(long)]
        registry_url: String,
        /// Bearer token required for publish/index/yank requests.
        #[arg(long)]
        token: Option<String>,
        /// JSON auth policy with scoped bearer tokens for index/publish/yank routes.
        #[arg(long, value_parser = parse_sanitized_path)]
        auth_policy: Option<PathBuf>,
        /// Load the HMAC signing key from a JSON file.
        #[arg(long, value_parser = parse_sanitized_path)]
        signing_key_file: Option<PathBuf>,
        /// Load an HMAC signing keyring and sign with its active key.
        #[arg(long, value_parser = parse_sanitized_path)]
        signing_keyring_file: Option<PathBuf>,
        /// Load an Ed25519 private signing key from a JSON file.
        #[arg(long, value_parser = parse_sanitized_path)]
        signing_private_key_file: Option<PathBuf>,
        /// Optional HMAC signing key id for generated registry signatures.
        #[arg(long)]
        signing_key_id: Option<String>,
        /// Optional HMAC signing secret for generated registry signatures.
        #[arg(long)]
        signing_secret: Option<String>,
    },
    /// Print the resolved dependency tree.
    Tree,
}

#[derive(Debug, Subcommand)]
enum PkgIndexCommand {
    /// Download [registry].url/api/v1/index into $LK_HOME/registry/<name>/index.json.
    Sync,
}

#[derive(Debug, Subcommand)]
enum PkgKeyCommand {
    /// Generate an HMAC registry signing key JSON file.
    Generate {
        /// Output path for the key JSON file.
        #[arg(long, value_parser = parse_sanitized_path)]
        out: PathBuf,
        /// Key id embedded in registry signatures.
        #[arg(long)]
        key_id: String,
    },
    /// Generate an Ed25519 private/public registry signing key pair.
    GenerateAsymmetric {
        /// Output path for the private key JSON file.
        #[arg(long, value_parser = parse_sanitized_path)]
        private_out: PathBuf,
        /// Output path for the public key JSON file.
        #[arg(long, value_parser = parse_sanitized_path)]
        public_out: PathBuf,
        /// Key id embedded in registry signatures.
        #[arg(long)]
        key_id: String,
    },
    /// Initialize an HMAC registry signing keyring JSON file.
    InitKeyring {
        /// Output path for the keyring JSON file.
        #[arg(long, value_parser = parse_sanitized_path)]
        out: PathBuf,
        /// Initial active key id embedded in registry signatures.
        #[arg(long)]
        key_id: String,
    },
    /// Add a new active key to an existing keyring.
    Rotate {
        /// Keyring JSON file to update.
        #[arg(long, value_parser = parse_sanitized_path)]
        keyring: PathBuf,
        /// New active key id.
        #[arg(long)]
        key_id: String,
    },
    /// Mark a non-active key id as revoked in a keyring.
    Revoke {
        /// Keyring JSON file to update.
        #[arg(long, value_parser = parse_sanitized_path)]
        keyring: PathBuf,
        /// Existing non-active key id to revoke.
        #[arg(long)]
        key_id: String,
    },
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
                        sanitize_path(p.to_string_lossy().as_ref()).map_err(|e| {
                            diagnostic::error(&e);
                            e
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
        let artifact =
            ModuleArtifact::from_json_str(&input).with_context(|| format!("decode Instr module {}", safe.display()))?;
        let mut base_env = build_vm_context(&safe)?;
        let profile_enabled = vm_profile_enabled();
        maybe_start_vm_profile(profile_enabled);
        let exec_result =
            execute_module_artifact_with_ctx(artifact, &mut base_env).with_context(|| "VM module execution failed");
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

    #[cfg(feature = "llvm")]
    if try_execute_cached_native(&safe, input.as_bytes())? {
        return Ok(());
    }

    // Initialize runtime for concurrency if enabled
    if let Err(e) = rt::init_runtime() {
        diagnostic::warning(format_args!("Failed to initialize runtime: {}", e));
    }

    // Parse, expand macros, and execute as statements.
    // NOTE: Direct `.lk` execution does not check proc-macro dependency
    // freshness against cached native binaries. Proc macros are always
    // re-expanded through the macro system when running in VM mode.
    let program = match parse_program_source(&input, parse_options_for_file(&safe)?) {
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
        .execute_with_ctx(&mut base_env)
        .with_context(|| "VM execution failed");

    // Shutdown runtime after execution
    rt::shutdown_runtime();

    let result = exec_result?;
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
        diagnostic::error(&anyhow::anyhow!(message));
        std::process::exit(1);
    }
    Ok(())
}

fn compile_instr_module(path: &Path) -> anyhow::Result<()> {
    let artifact = compile_instr_artifact(path)?;
    let output = path.with_extension("lkm");
    std::fs::write(&output, artifact.to_json_string()?)
        .with_context(|| format!("write Instr module {}", output.display()))?;
    println!("{}", output.display());
    Ok(())
}

struct CompiledInstrArtifact {
    artifact: ModuleArtifact,
    proc_macro_dependencies: Vec<ProcMacroDependency>,
}

fn compile_instr_artifact(path: &Path) -> anyhow::Result<ModuleArtifact> {
    Ok(compile_instr_artifact_with_dependencies(path)?.artifact)
}

fn compile_instr_artifact_with_dependencies(path: &Path) -> anyhow::Result<CompiledInstrArtifact> {
    let expansion = expand_program_file(path)?;
    let mut ctx = build_vm_context(path)?;
    let module = compile_program_module_with_ctx(&expansion.program, &mut ctx)
        .with_context(|| format!("compile Instr module for {}", path.display()))?;
    Ok(CompiledInstrArtifact {
        artifact: ModuleArtifact::new(collect_program_imports(&expansion.program), &module)?,
        proc_macro_dependencies: expansion.proc_macro_dependencies,
    })
}

#[cfg(feature = "llvm")]
fn compile_llvm_ir(path: &Path, options: LlvmBackendOptions) -> anyhow::Result<()> {
    let artifact = compile_instr_artifact(path)?;
    let llvm = lk_llvm::compile_module_artifact_to_llvm(&artifact, options)
        .with_context(|| format!("compile LLVM IR for {}", path.display()))?;
    let output = path.with_extension("ll");
    std::fs::write(&output, llvm.module.ir).with_context(|| format!("write LLVM IR {}", output.display()))?;
    println!("{}", output.display());
    Ok(())
}

#[cfg(feature = "llvm")]
fn compile_executable(path: &Path, output: Option<&Path>, options: LlvmBackendOptions) -> anyhow::Result<()> {
    let output = output.map(Path::to_path_buf).unwrap_or_else(|| path.with_extension(""));
    compile_executable_to_path(path, &output, options)?;
    println!("{}", output.display());
    Ok(())
}

#[cfg(feature = "llvm")]
fn compile_executable_to_path(path: &Path, output: &Path, options: LlvmBackendOptions) -> anyhow::Result<()> {
    compile_executable_to_path_with_dependencies(path, output, options).map(|_| ())
}

#[cfg(feature = "llvm")]
fn compile_executable_to_path_with_dependencies(
    path: &Path,
    output: &Path,
    options: LlvmBackendOptions,
) -> anyhow::Result<Vec<ProcMacroDependency>> {
    let compiled = compile_instr_artifact_with_dependencies(path)?;
    let llvm = lk_llvm::compile_module_artifact_to_llvm(&compiled.artifact, options)
        .with_context(|| format!("compile native executable LLVM IR for {}", path.display()))?;
    lk_llvm::compile_native_executable_from_llvm(path, &output, &llvm.module.ir)?;
    Ok(compiled.proc_macro_dependencies)
}

#[cfg(feature = "llvm")]
fn try_execute_cached_native(path: &Path, source: &[u8]) -> anyhow::Result<bool> {
    if !native_run_enabled() {
        return Ok(false);
    }
    let Some(output) = cached_native_executable_path(path, source)? else {
        return Ok(false);
    };
    if !output.exists() || !native_cache_proc_macro_dependencies_fresh(path, &output) {
        let tmp = native_cache_tmp_path(&output);
        let options = LlvmBackendOptions {
            module_name: module_name_from_path(path),
            ..LlvmBackendOptions::default()
        };
        let dependencies = match compile_executable_to_path_with_dependencies(path, &tmp, options) {
            Ok(dependencies) => dependencies,
            Err(err) => {
                let _ = std::fs::remove_file(&tmp);
                if native_trace_enabled() {
                    diagnostic::warning(format_args!("Native cache build skipped: {err:#}"));
                }
                return Ok(false);
            }
        };
        let installed_by_this_process = match std::fs::rename(&tmp, &output) {
            Ok(()) => true,
            Err(err) => {
                let _ = std::fs::remove_file(&tmp);
                // Retry a few times in case another process is still writing the same output.
                let max_retries = 3;
                for attempt in 0..max_retries {
                    if output.exists()
                        && native_cache_proc_macro_dependencies_fresh(path, &output)
                    {
                        break; // Another process finished first.
                    }
                    if attempt + 1 < max_retries {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    } else if native_trace_enabled() {
                        diagnostic::warning(format_args!(
                            "Native cache install failed after {max_retries} retries: {err}"
                        ));
                    }
                }
                if output.exists()
                    && native_cache_proc_macro_dependencies_fresh(path, &output)
                {
                    false
                } else {
                    if native_trace_enabled() {
                        diagnostic::warning(format_args!(
                            "Native cache install failed: {err}"
                        ));
                    }
                    return Ok(false);
                }
            }
        };
        if installed_by_this_process
            && let Err(err) = write_native_cache_proc_macro_dependencies(path, &output, &dependencies)
            && native_trace_enabled()
        {
            diagnostic::warning(format_args!("Native cache dependency metadata skipped: {err:#}"));
        }
    }

    let status = match Command::new(&output).status() {
        Ok(status) => status,
        Err(err) => {
            let _ = std::fs::remove_file(&output);
            if native_trace_enabled() {
                diagnostic::warning(format_args!("Native cache run failed: {err}"));
            }
            return Ok(false);
        }
    };
    if status.success() {
        return Ok(true);
    }
    anyhow::bail!("cached native executable exited with status {status}");
}

#[cfg(feature = "llvm")]
fn native_run_enabled() -> bool {
    native_run_enabled_from_flags(
        env_flag("LK_FORCE_VM"),
        env_flag("LK_VM_ONLY"),
        env_flag("LK_VM_PROFILE"),
        env_flag("LK_NATIVE_RUN"),
    )
}

#[cfg(feature = "llvm")]
fn native_run_enabled_from_flags(force_vm: bool, vm_only: bool, vm_profile: bool, native_run: bool) -> bool {
    native_run && !(force_vm || vm_only || vm_profile)
}

#[cfg(feature = "llvm")]
fn native_trace_enabled() -> bool {
    env_flag("LK_NATIVE_TRACE")
}

#[cfg(feature = "llvm")]
fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref(),
        Ok("1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON")
    )
}

#[cfg(feature = "llvm")]
fn cached_native_executable_path(path: &Path, source: &[u8]) -> anyhow::Result<Option<PathBuf>> {
    let cache_dir = std::env::var_os("LK_NATIVE_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("lk-native-cache"));
    std::fs::create_dir_all(&cache_dir).with_context(|| format!("create native cache {}", cache_dir.display()))?;
    let source_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let exe = std::env::current_exe().ok();
    let mut hash = Fnv64::new();
    hash.bytes(source_path.to_string_lossy().as_bytes());
    hash.bytes(source);
    hash.bytes(env!("CARGO_PKG_VERSION").as_bytes());
    if let Some(exe) = exe.as_ref() {
        hash.bytes(exe.to_string_lossy().as_bytes());
        if let Ok(meta) = exe.metadata() {
            hash.u64(meta.len());
            hash_modified(&mut hash, &meta);
        }
    }
    Ok(Some(cache_dir.join(format!("lk-native-{:016x}", hash.finish()))))
}

#[cfg(feature = "llvm")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct NativeCacheProcMacroDependencies {
    dependencies: Vec<ProcMacroDependency>,
    fingerprint: ProcMacroDependencyFingerprint,
}

#[cfg(feature = "llvm")]
fn native_cache_proc_macro_dependencies_fresh(source_path: &Path, output: &Path) -> bool {
    let metadata_path = native_cache_proc_macro_dependencies_path(output);
    let Ok(raw) = std::fs::read_to_string(&metadata_path) else {
        if native_trace_enabled() {
            diagnostic::warning(format_args!(
                "proc-macro-deps metadata missing: {}",
                metadata_path.display()
            ));
        }
        return false;
    };
    let Ok(metadata) = serde_json::from_str::<NativeCacheProcMacroDependencies>(&raw) else {
        if native_trace_enabled() {
            diagnostic::warning(format_args!(
                "proc-macro-deps metadata corrupt at: {}",
                metadata_path.display()
            ));
        }
        return false;
    };
    metadata
        .fingerprint
        .is_current(&metadata.dependencies, source_path.parent())
}

#[cfg(feature = "llvm")]
fn write_native_cache_proc_macro_dependencies(
    source_path: &Path,
    output: &Path,
    dependencies: &[ProcMacroDependency],
) -> anyhow::Result<()> {
    let metadata_path = native_cache_proc_macro_dependencies_path(output);
    let metadata = NativeCacheProcMacroDependencies {
        dependencies: dependencies.to_vec(),
        fingerprint: fingerprint_proc_macro_dependencies(dependencies, source_path.parent()),
    };
    std::fs::write(&metadata_path, serde_json::to_vec_pretty(&metadata)?)
        .with_context(|| format!("write native cache dependency metadata {}", metadata_path.display()))
}

#[cfg(feature = "llvm")]
fn native_cache_proc_macro_dependencies_path(output: &Path) -> PathBuf {
    let file = output.file_name().and_then(|file| file.to_str()).unwrap_or("lk-native");
    output.with_file_name(format!("{file}.proc-macro-deps.json"))
}

#[cfg(feature = "llvm")]
fn native_cache_tmp_path(output: &Path) -> PathBuf {
    let file = output.file_name().and_then(|file| file.to_str()).unwrap_or("lk-native");
    output.with_file_name(format!("{file}.tmp-{}", std::process::id()))
}

#[cfg(feature = "llvm")]
fn hash_modified(hash: &mut Fnv64, meta: &std::fs::Metadata) {
    if let Ok(modified) = meta.modified()
        && let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH)
    {
        hash.u64(duration.as_secs());
        hash.u64(u64::from(duration.subsec_nanos()));
    }
}

#[cfg(feature = "llvm")]
struct Fnv64(u64);

#[cfg(feature = "llvm")]
impl Fnv64 {
    fn new() -> Self {
        Self(0xcbf29ce484222325)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }

    fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    fn finish(self) -> u64 {
        self.0
    }
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
