use super::*;

/// The lk-api C-ABI staticlib (VM + `lk_hybrid_*` bridge), built on demand.
/// Shared by the Tier 0 bundle and the Tier 1 hybrid link.
pub(super) fn ensure_lk_api_staticlib() -> anyhow::Result<PathBuf> {
    let workspace = workspace_root()?;
    let staticlib = workspace.join("target/release/liblk_api.a");
    if !staticlib.exists() {
        eprintln!("building lk-api staticlib (one-time)…");
    }
    // Always run the build: a stale staticlib (missing newer symbols such as
    // `lk_hybrid_*`) links partially or not at all, and a fresh build is a
    // sub-second no-op under cargo's fingerprinting.
    let status = std::process::Command::new("cargo")
        .current_dir(&workspace)
        .args(["build", "-p", "lk-api", "--features", "ffi", "--release"])
        .status()
        .map_err(|e| anyhow::anyhow!("cargo build lk-api: {e}"))?;
    if !status.success() {
        anyhow::bail!("failed to build lk-api staticlib");
    }
    Ok(staticlib)
}

/// Escape a string for embedding as a C double-quoted string literal.
pub(super) fn c_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

pub(super) fn compile_instr_module(path: &Path) -> anyhow::Result<()> {
    let artifact = compile_instr_artifact(path)?;
    let output = path.with_extension("lkm");
    std::fs::write(&output, artifact.to_json_string()?)
        .with_context(|| format!("write Instr module {}", output.display()))?;
    println!("{}", output.display());
    // `.lkm` is version-locked to this build and rejected by any other version
    // (see MODULE_ARTIFACT_VERSION); it is an internal/cache artifact, not a
    // distribution format — ship source or a native executable.
    eprintln!("note: `.lkm` is an internal build-locked artifact, not a distribution format");
    Ok(())
}

pub(super) struct CompiledInstrArtifact {
    pub(super) artifact: ModuleArtifact,
    pub(super) proc_macro_dependencies: Vec<ProcMacroDependency>,
}

pub(super) fn compile_instr_artifact(path: &Path) -> anyhow::Result<ModuleArtifact> {
    Ok(compile_instr_artifact_with_dependencies(path)?.artifact)
}

pub(super) fn compile_instr_artifact_with_dependencies(path: &Path) -> anyhow::Result<CompiledInstrArtifact> {
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
pub(super) fn compile_llvm_ir(path: &Path, options: LlvmBackendOptions) -> anyhow::Result<()> {
    let artifact = compile_instr_artifact(path)?;
    let bundled = bundle_file_imports(path, &artifact)?;
    let (artifact, bundles): (&ModuleArtifact, Vec<lk_llvm::BundledImport>) = match &bundled {
        Some((merged, bundles)) => (merged, bundles.clone()),
        None => (&artifact, Vec::new()),
    };
    let llvm = lk_llvm::compile_bundled_module_artifact_to_llvm(artifact, &bundles, options)
        .with_context(|| format!("compile LLVM IR for {}", path.display()))?;
    let output = path.with_extension("ll");
    std::fs::write(&output, llvm.module.ir).with_context(|| format!("write LLVM IR {}", output.display()))?;
    println!("{}", output.display());
    Ok(())
}

#[cfg(feature = "llvm")]
pub(super) fn compile_executable(
    path: &Path,
    output: Option<&Path>,
    options: LlvmBackendOptions,
) -> anyhow::Result<()> {
    let output = output.map(Path::to_path_buf).unwrap_or_else(|| path.with_extension(""));
    // Parse + compile up front so genuine source errors (syntax/type) surface
    // here, rather than being masked by the Tier 0 fallback below — that path
    // embeds the source verbatim and would only fail at runtime (plan M4.2).
    let compiled = compile_instr_artifact_with_dependencies(path)?;
    match compile_native_executable_from_artifact(path, &output, &compiled.artifact, options) {
        Ok(()) => {
            println!("{}", output.display());
            Ok(())
        }
        Err(native_err) => {
            // Opt-out (strict native-only) for tooling/tests that want to verify
            // the native lowering in isolation rather than the graceful fallback.
            if std::env::var_os("LK_AOT_NO_FALLBACK").is_some_and(|value| value != "0") {
                return Err(native_err);
            }
            // The native (MIR/LLVM) backend covers only a lowerable subset.
            // Instead of failing the whole program (the old all-or-nothing —
            // plan 问题 2), fall back to the Tier 0 VM bundle, which embeds the
            // interpreter and runs any valid program. `lk compile` thus never
            // rejects a valid program: native when possible, VM-embed otherwise.
            diagnostic::warning(format!(
                "native AOT does not support this program yet ({native_err:#}); \
                 falling back to the Tier 0 VM bundle (embeds the interpreter)"
            ));
            // If the Tier 0 fallback also fails (e.g. the VM staticlib is
            // unavailable outside the dev workspace), surface both reasons so the
            // failure is understandable rather than a bare bundle error.
            run_bundle(path, &output).map_err(|bundle_err| {
                anyhow::anyhow!(
                    "cannot compile `{}`: it is not natively AOT-lowerable ({native_err:#}), \
                     and the Tier 0 VM-bundle fallback also failed ({bundle_err:#})",
                    path.display()
                )
            })
        }
    }
}

/// Lower an already-compiled bytecode artifact to a native executable via LLVM.
/// Returns the (subset-only) lowering error so `compile_executable` can decide
/// whether to fall back to the Tier 0 VM bundle.
#[cfg(feature = "llvm")]
pub(super) fn compile_native_executable_from_artifact(
    path: &Path,
    output: &Path,
    artifact: &ModuleArtifact,
    options: LlvmBackendOptions,
) -> anyhow::Result<()> {
    let bundled = bundle_file_imports(path, artifact)?;
    let (artifact, bundles): (&ModuleArtifact, Vec<lk_llvm::BundledImport>) = match &bundled {
        Some((merged, bundles)) => (merged, bundles.clone()),
        None => (artifact, Vec::new()),
    };
    // Strangler front (`docs/llvm/aot-redesign.md`): the Cranelift backend is the
    // default native path (opt out with `LK_AOT_CLIF=0`). It reaches full example
    // parity with the string-IR path (pure-native + Tier 1 hybrid) and emits a
    // native object directly (typed builder + verifier, no clang optimization
    // pass); any shape it still rejects falls through to the string-IR path below,
    // so no program regresses.
    if clif_backend_enabled() {
        match lk_llvm::compile_artifact_to_clif_object(artifact, &bundles)? {
            Ok(clif) => {
                if native_trace_enabled() {
                    eprintln!("clif: native object for {}", path.display());
                }
                if clif.vm_function_count > 0 {
                    // Tier 1 hybrid: the object bridges to the VM for the
                    // non-lowering helpers, so embed the artifact and link the
                    // lk-api staticlib alongside lkrt (mirrors the string-IR path).
                    let artifact_json = artifact.to_json_string()?;
                    let staticlib = ensure_lk_api_staticlib()?;
                    eprintln!(
                        "note: {} function(s) run on the embedded VM (Tier 1 hybrid, Cranelift)",
                        clif.vm_function_count
                    );
                    lk_llvm::compile_native_executable_from_object_hybrid(
                        path,
                        output,
                        &clif.object,
                        lk_llvm::HybridLink {
                            module_artifact_json: &artifact_json,
                            lk_api_staticlib: &staticlib,
                        },
                    )?;
                } else {
                    lk_llvm::compile_native_executable_from_object(path, output, &clif.object)?;
                }
                return Ok(());
            }
            Err(reason) => {
                // `LK_AOT_CLIF_ONLY` turns a fallback into a hard error — the
                // differential harness uses it to guarantee a case is compiled
                // *through Cranelift*, not silently by the string-IR path.
                if env_flag("LK_AOT_CLIF_ONLY") {
                    anyhow::bail!(
                        "LK_AOT_CLIF_ONLY: Cranelift cannot compile `{}` yet ({reason})",
                        path.display()
                    );
                }
                if native_trace_enabled() {
                    eprintln!("clif: fallback for {} ({reason})", path.display());
                }
            }
        }
    }
    let llvm = lk_llvm::compile_bundled_module_artifact_to_llvm(artifact, &bundles, options)
        .with_context(|| format!("compile native executable LLVM IR for {}", path.display()))?;
    if llvm.module.vm_function_count > 0 {
        // Tier 1 hybrid (docs/llvm/tier1-hybrid.md): the IR bridges into the
        // VM for the marked functions, so embed the module artifact and link
        // the lk-api staticlib alongside lkrt.
        let artifact_json = artifact.to_json_string()?;
        let staticlib = ensure_lk_api_staticlib()?;
        eprintln!(
            "note: {} function(s) run on the embedded VM (Tier 1 hybrid)",
            llvm.module.vm_function_count
        );
        lk_llvm::compile_native_executable_from_llvm_hybrid(
            path,
            output,
            &llvm.module.ir,
            llvm.opt_level.as_flag(),
            Some(lk_llvm::HybridLink {
                module_artifact_json: &artifact_json,
                lk_api_staticlib: &staticlib,
            }),
        )?;
        return Ok(());
    }
    lk_llvm::compile_native_executable_from_llvm(path, output, &llvm.module.ir, llvm.opt_level.as_flag())?;
    Ok(())
}

#[cfg(feature = "llvm")]
pub(super) fn compile_executable_to_path_with_dependencies(
    path: &Path,
    output: &Path,
    options: LlvmBackendOptions,
) -> anyhow::Result<Vec<ProcMacroDependency>> {
    let compiled = compile_instr_artifact_with_dependencies(path)?;
    let bundled = bundle_file_imports(path, &compiled.artifact)?;
    let (artifact, bundles): (&ModuleArtifact, Vec<lk_llvm::BundledImport>) = match &bundled {
        Some((merged, bundles)) => (merged, bundles.clone()),
        None => (&compiled.artifact, Vec::new()),
    };
    let llvm = lk_llvm::compile_bundled_module_artifact_to_llvm(artifact, &bundles, options)
        .with_context(|| format!("compile native executable LLVM IR for {}", path.display()))?;
    lk_llvm::compile_native_executable_from_llvm(path, output, &llvm.module.ir, llvm.opt_level.as_flag())?;
    Ok(compiled.proc_macro_dependencies)
}

#[cfg(feature = "llvm")]
pub(super) fn try_execute_cached_native(path: &Path, source: &[u8]) -> anyhow::Result<bool> {
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
                    if output.exists() && native_cache_proc_macro_dependencies_fresh(path, &output) {
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
                if output.exists() && native_cache_proc_macro_dependencies_fresh(path, &output) {
                    false
                } else {
                    if native_trace_enabled() {
                        diagnostic::warning(format_args!("Native cache install failed: {err}"));
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
pub(super) fn native_run_enabled() -> bool {
    native_run_enabled_from_flags(
        env_flag("LK_FORCE_VM"),
        env_flag("LK_VM_ONLY"),
        env_flag("LK_VM_PROFILE"),
        env_flag("LK_NATIVE_RUN"),
    )
}

#[cfg(feature = "llvm")]
pub(super) fn native_run_enabled_from_flags(force_vm: bool, vm_only: bool, vm_profile: bool, native_run: bool) -> bool {
    native_run && !(force_vm || vm_only || vm_profile)
}

#[cfg(feature = "llvm")]
pub(super) fn native_trace_enabled() -> bool {
    env_flag("LK_NATIVE_TRACE")
}

/// Whether the Cranelift backend drives the native compile. Default on since it
/// reached full example + hybrid parity with the string-IR path; `LK_AOT_CLIF=0`
/// opts back into string-IR-first (e.g. to compare, or as an escape hatch).
#[cfg(feature = "llvm")]
pub(super) fn clif_backend_enabled() -> bool {
    std::env::var_os("LK_AOT_CLIF").is_none_or(|value| value != "0")
}

#[cfg(feature = "llvm")]
pub(super) fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref(),
        Ok("1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON")
    )
}

#[cfg(feature = "llvm")]
pub(super) fn cached_native_executable_path(path: &Path, source: &[u8]) -> anyhow::Result<Option<PathBuf>> {
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
    // Build-affecting environment must be part of the key, or a cached binary
    // built under different flags gets silently reused.
    if let Some(sanitize) = std::env::var_os("LK_NATIVE_SANITIZE") {
        hash.bytes(b"LK_NATIVE_SANITIZE=");
        hash.bytes(sanitize.to_string_lossy().as_bytes());
    }
    // NOTE: the key covers only this file's bytes. AOT lowering rejects
    // imports today, so imported-module content cannot affect a cached
    // native binary yet — when import lowering lands, imported file contents
    // must join this hash or stale caches will run old dependency code.
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
pub(super) struct NativeCacheProcMacroDependencies {
    dependencies: Vec<ProcMacroDependency>,
    fingerprint: ProcMacroDependencyFingerprint,
}

#[cfg(feature = "llvm")]
pub(super) fn native_cache_proc_macro_dependencies_fresh(source_path: &Path, output: &Path) -> bool {
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
pub(super) fn write_native_cache_proc_macro_dependencies(
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
pub(super) fn native_cache_proc_macro_dependencies_path(output: &Path) -> PathBuf {
    let file = output.file_name().and_then(|file| file.to_str()).unwrap_or("lk-native");
    output.with_file_name(format!("{file}.proc-macro-deps.json"))
}

#[cfg(feature = "llvm")]
pub(super) fn native_cache_tmp_path(output: &Path) -> PathBuf {
    let file = output.file_name().and_then(|file| file.to_str()).unwrap_or("lk-native");
    output.with_file_name(format!("{file}.tmp-{}", std::process::id()))
}

#[cfg(feature = "llvm")]
pub(super) fn hash_modified(hash: &mut Fnv64, meta: &std::fs::Metadata) {
    if let Ok(modified) = meta.modified()
        && let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH)
    {
        hash.u64(duration.as_secs());
        hash.u64(u64::from(duration.subsec_nanos()));
    }
}

#[cfg(feature = "llvm")]
pub(super) struct Fnv64(u64);

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
pub(super) fn module_name_from_path(path: &Path) -> String {
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
