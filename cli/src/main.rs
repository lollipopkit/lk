#[cfg(feature = "llvm")]
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Once};
use std::{collections::BTreeMap, fs};

static PERF_TRACE_INIT: Once = Once::new();
const DEFAULT_TRACE_FILTER: &str =
    "lk::vm::alloc=trace,lk::vm::bc32=info,lk::vm::slowpath=debug,lk_core=info,lk_cli=info";

use clap::{Parser, Subcommand, ValueEnum};
#[cfg(feature = "llvm")]
use lk_core::llvm::{LlvmBackendOptions, OptLevel, compile_function_to_llvm};
use lk_core::{
    module::ModuleRegistry,
    package::{
        DependencySpec, DetailedDependency, LOCK_FILE, LockFile, LockedPackage, MANIFEST_FILE, Manifest, PackageGraph,
        PackageModule, PackageSection, cache_dir_for_source, find_manifest,
    },
    rt,
    stmt::{ModuleResolver, Program, deserialize_imports, execute_imports, serialize_imports, stmt_parser::StmtParser},
    token::Tokenizer,
    typ::TypeChecker,
    val::Val,
    vm::{self, BundledModule, BytecodeModule, ModuleFlags, ModuleMeta, Vm, VmContext, compile_program},
};

use anyhow::Context;
#[cfg(feature = "llvm")]
use llvm_tools::LlvmTools;

mod bundler;
#[cfg(test)]
mod main_test;
mod repl;

use bundler::ModuleBundler;

#[cfg(feature = "llvm")]
const RUNTIME_CRATE_NAME: &str = "lk-core";
#[cfg(feature = "llvm")]
const RUNTIME_STDLIB_CRATE: &str = "lk-stdlib";

#[cfg(feature = "llvm")]
struct EncodedBundledModule {
    path: String,
    bytes: Vec<u8>,
}

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
enum CompileMode {
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
        /// 支持 `lk compile [TARGET] FILE`（默认为 `lkb`）
        #[arg(value_name = "ARGS", num_args = 1..=2)]
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

fn read_file_content(path: &str) -> anyhow::Result<String> {
    std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("Failed to read file '{}': {}", path, e))
}

fn sanitize_path(raw: &str) -> anyhow::Result<PathBuf> {
    let p = Path::new(raw);

    for comp in p.components() {
        if matches!(comp, Component::ParentDir) {
            return Err(anyhow::anyhow!(
                "Parent directory components ('..') are not allowed in file paths."
            ));
        }
    }

    Ok(p.to_path_buf())
}

fn parse_sanitized_path(raw: &str) -> Result<PathBuf, String> {
    sanitize_path(raw).map_err(|e| e.to_string())
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

fn parse_program_file(path: &Path) -> anyhow::Result<Program> {
    let src = read_file_content(&path.to_string_lossy())?;
    let (tokens, spans) = match Tokenizer::tokenize_enhanced_with_spans(&src) {
        Ok(result) => result,
        Err(parse_err) => {
            eprintln!("Error: {}", parse_err);
            std::process::exit(1);
        }
    };
    let mut parser = StmtParser::new_with_spans(&tokens, &spans);
    match parser.parse_program_with_enhanced_errors(&src) {
        Ok(program) => Ok(program),
        Err(parse_err) => {
            eprintln!("Error: {}", parse_err);
            std::process::exit(1);
        }
    }
}

pub(crate) fn split_compile_args(args: &[String]) -> anyhow::Result<(Option<CompileMode>, PathBuf)> {
    match args.len() {
        1 => Ok((None, sanitize_path(&args[0])?)),
        2 => {
            #[cfg(not(feature = "llvm"))]
            if matches!(args[0].to_ascii_lowercase().as_str(), "llvm" | "exe") {
                anyhow::bail!(
                    "LLVM backend disabled at build time; rebuild with `--features llvm` to use '{}' target",
                    args[0]
                );
            }
            let mode = CompileMode::from_str(&args[0], true)
                .map_err(|_| anyhow::anyhow!("Unknown compile target '{}'", args[0]))?;
            let file = sanitize_path(&args[1])?;
            Ok((Some(mode), file))
        }
        _ => anyhow::bail!("compile requires <FILE> or <TARGET FILE>"),
    }
}

#[cfg(feature = "llvm")]
fn resolve_llvm_tool(tool: &str, env_var: &str) -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var(env_var) {
        let path = PathBuf::from(explicit);
        if path.exists() {
            return Some(path);
        }
    }
    if let Ok(tools) = LlvmTools::new()
        && let Some(path) = tools.tool(tool)
    {
        return Some(path);
    }
    let fallback = PathBuf::from(tool);
    if Command::new(&fallback)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
    {
        Some(fallback)
    } else {
        None
    }
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
) -> RuntimeInitPlan {
    let mut plan = RuntimeInitPlan::default();

    let decls = [
        "declare void @lk_rt_begin_session()",
        "declare void @lk_rt_register_search_path(i8*, i64)",
        "declare i32 @lk_rt_register_bundled_module(i8*, i64, i8*, i64)",
        "declare i32 @lk_rt_register_imports(i8*, i64)",
        "declare i32 @lk_rt_register_package_modules(i8*, i64)",
        "declare i32 @lk_rt_apply_imports()",
    ];
    for decl in decls {
        if !module_ir.contains(decl) {
            plan.declarations.push(decl.to_string());
        }
    }

    plan.body_lines.push("call void @lk_rt_begin_session()".to_string());

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

    plan.body_lines.push("call i32 @lk_rt_apply_imports()".to_string());

    plan
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
                        let llvm_artifact = compile_function_to_llvm(&func, "lk_entry", options);
                        let (ll_with_main, unopt_with_main, bytecode_trampoline_c_src) = match llvm_artifact {
                            Ok(artifact) => {
                                let final_ir = artifact.optimised_ir.as_deref().unwrap_or(&artifact.module.ir);
                                let runtime_plan = build_runtime_init_plan(
                                    final_ir,
                                    &search_paths,
                                    imports_serialized.as_deref(),
                                    package_modules_json.as_deref(),
                                    &encoded_modules,
                                );
                                let ll_with_main = append_main_stub(final_ir, "lk_entry", &runtime_plan);
                                let unopt_plan = build_runtime_init_plan(
                                    &artifact.module.ir,
                                    &search_paths,
                                    imports_serialized.as_deref(),
                                    package_modules_json.as_deref(),
                                    &encoded_modules,
                                );
                                let unopt_with_main = append_main_stub(&artifact.module.ir, "lk_entry", &unopt_plan);
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

                        let runtime_staticlibs = ensure_runtime_staticlib(
                            target_triple.as_deref(),
                            matches!(opt_level_cli, OptLevelCli::O3) && !skip_opt,
                        )
                        .with_context(|| "failed to produce LLVM runtime static library")?;

                        let exe_path = output.clone().unwrap_or_else(|| safe.with_extension("elf"));
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
                            cc_cmd.arg("-framework").arg("CoreFoundation");
                            cc_cmd.arg("-framework").arg("CoreServices");
                        }
                        let cc_status = cc_cmd
                            .status()
                            .with_context(|| format!("failed to spawn linker {}", cc))?;
                        if !cc_status.success() {
                            anyhow::bail!("linker {} failed with status {}", cc, cc_status);
                        }

                        let backend_label = if bytecode_trampoline_c_src.is_some() {
                            "VM bytecode trampoline"
                        } else {
                            opt_level_cli.label()
                        };
                        eprintln!(
                            "Emitted ELF executable to {} (backend {}, LLVM IR at {})",
                            exe_path.display(),
                            backend_label,
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

        let mut vm = vm::Vm::new();
        let result = vm.exec_with(&module.entry, &mut base_env, None);

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
        let mut vm = Vm::new();
        let val = vm
            .exec_with(&compiled, &mut base_env, None)
            .with_context(|| "VM execution failed")?;
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

fn init_package(name: Option<String>) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("read current directory")?;
    let package_name = name
        .or_else(|| cwd.file_name().map(|name| name.to_string_lossy().to_string()))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "lk-package".to_string());
    let manifest_path = cwd.join(MANIFEST_FILE);
    if manifest_path.exists() {
        anyhow::bail!("{} already exists", manifest_path.display());
    }
    let manifest = Manifest {
        package: Some(PackageSection {
            name: package_name.clone(),
            version: Some("0.1.0".to_string()),
            edition: Some("2026".to_string()),
            license: None,
            authors: Vec::new(),
            description: None,
        }),
        workspace: None,
        dependencies: BTreeMap::new(),
    };
    manifest.write(&manifest_path)?;
    let src_dir = cwd.join("src");
    fs::create_dir_all(&src_dir).with_context(|| format!("create {}", src_dir.display()))?;
    let main_path = src_dir.join("main.lk");
    if !main_path.exists() {
        fs::write(
            &main_path,
            "println(\"hello from ${pkg}\");\n".replace("${pkg}", &package_name),
        )
        .with_context(|| format!("write {}", main_path.display()))?;
    }
    eprintln!("Created {}", manifest_path.display());
    Ok(())
}

fn run_pkg_command(command: PkgCommand) -> anyhow::Result<()> {
    match command {
        PkgCommand::Add {
            name,
            source,
            branch,
            tag,
            rev,
        } => add_dependency(name, source, branch, tag, rev),
        PkgCommand::Fetch => fetch_dependencies(None),
        PkgCommand::Update { name } => fetch_dependencies(name),
        PkgCommand::Tree => print_package_tree(),
    }
}

fn load_project_manifest() -> anyhow::Result<(PathBuf, Manifest)> {
    let cwd = std::env::current_dir().context("read current directory")?;
    let manifest_path = find_manifest(&cwd).ok_or_else(|| anyhow::anyhow!("No {MANIFEST_FILE} found"))?;
    let manifest = Manifest::read(&manifest_path)?;
    Ok((manifest_path, manifest))
}

fn add_dependency(
    name: String,
    source: String,
    branch: Option<String>,
    tag: Option<String>,
    rev: Option<String>,
) -> anyhow::Result<()> {
    let (manifest_path, mut manifest) = load_project_manifest()?;
    let spec = if branch.is_none() && tag.is_none() && rev.is_none() {
        DependencySpec::GitHub(source)
    } else {
        DependencySpec::Detailed(DetailedDependency {
            github: Some(source),
            branch,
            tag,
            rev,
            ..Default::default()
        })
    };
    manifest.dependencies.insert(name, spec);
    manifest.write(&manifest_path)?;
    eprintln!("Updated {}", manifest_path.display());
    Ok(())
}

fn fetch_dependencies(only: Option<String>) -> anyhow::Result<()> {
    let (manifest_path, manifest) = load_project_manifest()?;
    let root = manifest_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("manifest has no parent"))?;
    let mut lock = LockFile::read(&root.join(LOCK_FILE))?;
    let mut locked = BTreeMap::new();
    for pkg in lock.package {
        locked.insert(pkg.name.clone(), pkg);
    }

    let dependencies = manifest.dependencies;
    for (name, spec) in dependencies {
        if only.as_ref().is_some_and(|only| only != &name) {
            continue;
        }
        if spec.is_workspace() || spec.path().is_some() {
            continue;
        }
        let source = spec
            .git_url()
            .ok_or_else(|| anyhow::anyhow!("dependency '{name}' has no git source"))?;
        let dir = cache_dir_for_source(&source);
        fetch_git_dependency(&source, &dir, &spec)?;
        let rev = git_output(&dir, ["rev-parse", "HEAD"])?;
        locked.insert(name.clone(), LockedPackage { name, source, rev });
    }

    lock = LockFile {
        package: locked.into_values().collect(),
    };
    lock.write(&root.join(LOCK_FILE))?;
    eprintln!("Updated {}", root.join(LOCK_FILE).display());
    Ok(())
}

fn fetch_git_dependency(source: &str, dir: &Path, spec: &DependencySpec) -> anyhow::Result<()> {
    if dir.exists() {
        git_status(
            Command::new("git")
                .arg("-C")
                .arg(dir)
                .arg("fetch")
                .arg("--tags")
                .arg("--prune"),
        )?;
    } else {
        if let Some(parent) = dir.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        git_status(Command::new("git").arg("clone").arg(source).arg(dir))?;
    }

    if let DependencySpec::Detailed(dep) = spec {
        if let Some(rev) = dep.rev.as_ref() {
            git_status(Command::new("git").arg("-C").arg(dir).arg("checkout").arg(rev))?;
        } else if let Some(tag) = dep.tag.as_ref() {
            git_status(
                Command::new("git")
                    .arg("-C")
                    .arg(dir)
                    .arg("checkout")
                    .arg(format!("tags/{tag}")),
            )?;
        } else if let Some(branch) = dep.branch.as_ref() {
            git_status(Command::new("git").arg("-C").arg(dir).arg("checkout").arg(branch))?;
            git_status(Command::new("git").arg("-C").arg(dir).arg("pull").arg("--ff-only"))?;
        }
    }
    Ok(())
}

fn git_status(cmd: &mut Command) -> anyhow::Result<()> {
    let status = cmd.status().context("run git")?;
    if !status.success() {
        anyhow::bail!("git failed with status {status}");
    }
    Ok(())
}

fn git_output<const N: usize>(dir: &Path, args: [&str; N]) -> anyhow::Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .context("run git")?;
    if !output.status.success() {
        anyhow::bail!("git failed with status {}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn print_package_tree() -> anyhow::Result<()> {
    let cwd = std::env::current_dir().context("read current directory")?;
    let graph = PackageGraph::discover(&cwd)?.ok_or_else(|| anyhow::anyhow!("No {MANIFEST_FILE} found"))?;
    let root_name = graph
        .manifest
        .package
        .as_ref()
        .map(|package| package.name.as_str())
        .unwrap_or("<workspace>");
    println!("{root_name} ({})", graph.manifest_dir().display());
    for module in &graph.modules {
        println!("  {} -> {}", module.name, module.root.display());
    }
    for missing in &graph.missing {
        println!("  {} -> <missing; run lk pkg fetch>", missing);
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

#[cfg(feature = "llvm")]
fn ensure_runtime_staticlib(target_triple: Option<&str>, use_release: bool) -> anyhow::Result<Vec<PathBuf>> {
    if let Some(packaged) = find_packaged_staticlibs(target_triple, use_release) {
        return Ok(packaged);
    }
    let libs = vec![
        build_staticlib(RUNTIME_CRATE_NAME, target_triple, use_release)?,
        build_staticlib(RUNTIME_STDLIB_CRATE, target_triple, use_release)?,
    ];
    Ok(libs)
}

#[cfg(feature = "llvm")]
fn build_staticlib(crate_name: &str, target_triple: Option<&str>, use_release: bool) -> anyhow::Result<PathBuf> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let runtime_target_root = std::env::var("LK_RUNTIME_TARGET_DIR")
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                workspace_root.join(path)
            }
        })
        .unwrap_or_else(|_| workspace_root.join("target").join("lk-native"));

    let mut cmd = Command::new(&cargo);
    cmd.arg("build").arg("-p").arg(crate_name).arg("--lib");
    if use_release {
        cmd.arg("--release");
    }
    if let Some(triple) = target_triple {
        cmd.arg("--target").arg(triple);
    }
    cmd.current_dir(&workspace_root);
    cmd.env("CARGO_TARGET_DIR", &runtime_target_root);
    let status = cmd
        .status()
        .with_context(|| format!("failed to run `{cargo} build` for {crate_name} staticlib"))?;
    if !status.success() {
        anyhow::bail!("{cargo} build exited with status {status}");
    }

    let mut lib_path = runtime_target_root.clone();
    if let Some(triple) = target_triple {
        lib_path.push(triple);
    }
    lib_path.push(if use_release { "release" } else { "debug" });
    let crate_stub = crate_name.replace('-', "_");
    lib_path.push(format!("lib{crate_stub}.a"));
    if !lib_path.exists() {
        anyhow::bail!(
            "runtime static library {} was not produced (expected `{}`)",
            crate_name,
            lib_path.display()
        );
    }
    Ok(lib_path)
}

#[cfg(feature = "llvm")]
fn find_packaged_staticlibs(target_triple: Option<&str>, use_release: bool) -> Option<Vec<PathBuf>> {
    let mut roots = Vec::new();
    if let Ok(env_dir) = std::env::var("LK_RUNTIME_LIB_DIR") {
        let candidate = PathBuf::from(env_dir);
        if candidate.exists() {
            roots.push(candidate);
        }
    }

    if let Ok(exe_path) = std::env::current_exe()
        && let Some(bin_dir) = exe_path.parent()
    {
        roots.push(bin_dir.to_path_buf());
        roots.push(bin_dir.join("lib"));
        if let Some(parent) = bin_dir.parent() {
            roots.push(parent.to_path_buf());
            roots.push(parent.join("lib"));
        }
    }

    let profile_dir = if use_release { "release" } else { "debug" };
    let mut seen = std::collections::HashSet::new();

    for root in roots.into_iter() {
        if !seen.insert(root.clone()) {
            continue;
        }

        let mut dirs = vec![
            root.clone(),
            root.join(profile_dir),
            root.join("lib"),
            root.join("lib").join(profile_dir),
        ];
        if let Some(triple) = target_triple {
            dirs.push(root.join(triple));
            dirs.push(root.join(triple).join(profile_dir));
            dirs.push(root.join("lib").join(triple));
            dirs.push(root.join("lib").join(triple).join(profile_dir));
        }

        for dir in dirs {
            if let Some(paths) = staticlibs_from_dir(&dir) {
                return Some(paths);
            }
        }
    }

    None
}

#[cfg(feature = "llvm")]
fn staticlibs_from_dir(dir: &Path) -> Option<Vec<PathBuf>> {
    if !dir.exists() {
        return None;
    }

    let mut libs = Vec::new();
    for crate_name in [RUNTIME_CRATE_NAME, RUNTIME_STDLIB_CRATE] {
        let filename = format!("lib{}.a", crate_name.replace('-', "_"));
        let path = dir.join(filename);
        if !path.exists() {
            return None;
        }
        libs.push(path);
    }

    Some(libs)
}

#[cfg(all(test, feature = "llvm"))]
mod packaged_staticlib_tests {
    use super::staticlibs_from_dir;
    use std::fs;

    #[test]
    fn uses_env_dir_when_all_libs_present() {
        let temp = tempfile::tempdir().expect("tempdir");
        for name in ["liblk_core.a", "liblk_stdlib.a"] {
            let path = temp.path().join(name);
            fs::write(&path, []).expect("write stub lib");
        }

        let libs = staticlibs_from_dir(temp.path()).expect("should discover libs");
        assert_eq!(libs.len(), 2);
        assert!(libs.iter().all(|p| p.exists()));
    }
}
