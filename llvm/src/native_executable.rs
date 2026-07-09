use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::Context;

pub fn compile_native_executable_from_llvm(path: &Path, output: &Path, ir: &str, opt_flag: &str) -> anyhow::Result<()> {
    compile_native_executable_from_llvm_hybrid(path, output, ir, opt_flag, None)
}

/// Tier 1 hybrid link info (`docs/llvm/tier1-hybrid.md`): the serialized module
/// artifact to embed (registered via a C constructor so the generated `main`
/// stays untouched) and the lk-api staticlib providing `lk_hybrid_*`.
pub struct HybridLink<'a> {
    pub module_artifact_json: &'a str,
    pub lk_api_staticlib: &'a Path,
}

pub fn compile_native_executable_from_llvm_hybrid(
    path: &Path,
    output: &Path,
    ir: &str,
    opt_flag: &str,
    hybrid: Option<HybridLink<'_>>,
) -> anyhow::Result<()> {
    let _ = lkrt::link_anchor();
    let source_path = temp_llvm_source_path(path);
    std::fs::write(&source_path, ir).with_context(|| format!("write native LLVM IR {}", source_path.display()))?;
    let wrapper_path = hybrid
        .as_ref()
        .map(|link| {
            let wrapper = hybrid_wrapper_c(link.module_artifact_json);
            let wrapper_path = source_path.with_extension("hybrid.c");
            std::fs::write(&wrapper_path, wrapper)
                .with_context(|| format!("write hybrid wrapper {}", wrapper_path.display()))?;
            Ok::<_, anyhow::Error>(wrapper_path)
        })
        .transpose()
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&source_path);
        })?;
    let clang = clang_command();
    let mut command = Command::new(&clang);
    // The MIR codegen emits naive SSA text and relies on clang's optimizer for
    // cleanup; `opt_flag` comes from `LlvmBackendOptions` (`-O2` by default,
    // `-O0` under `--skip-opt`).
    command.arg(opt_flag).arg(&source_path).arg("-o").arg(output);
    if let Some(wrapper_path) = &wrapper_path {
        command.arg(wrapper_path);
    }
    // `LK_NATIVE_SANITIZE=address,undefined` forwards `-fsanitize=` so the
    // differential corpora can run native binaries under ASan/UBSan; the
    // handwritten runtime helpers and generated IR are otherwise only
    // exercised without sanitizers.
    if let Some(sanitizers) = std::env::var_os("LK_NATIVE_SANITIZE")
        && !sanitizers.is_empty()
    {
        command.arg(format!("-fsanitize={}", sanitizers.to_string_lossy()));
    }
    if let Some(staticlib) = lkrt_staticlib_path() {
        add_force_load_staticlib(&mut command, &staticlib);
    }
    if let Some(link) = &hybrid {
        // The bridge VM (lk-api + lk-core + stdlib) rides in via the same
        // staticlib the Tier 0 bundle links; no force-load needed — the
        // wrapper references `lk_hybrid_register` and the IR references
        // `lk_hybrid_call_v`, which pulls the objects in.
        command.arg(link.lk_api_staticlib);
        command.args(["-lpthread", "-ldl"]);
    }
    // lkrt's float math (`powf` etc.) lowers to libm calls; macOS bundles libm
    // in libSystem, Windows in the CRT, so the explicit link is Linux-only.
    if cfg!(target_os = "linux") {
        command.arg("-lm");
    }
    let output_status = match command
        .output()
        .with_context(|| format!("spawn clang to build native executable {}", output.display()))
    {
        Ok(output_status) => output_status,
        Err(error) => {
            let _ = std::fs::remove_file(&source_path);
            return Err(error);
        }
    };
    let _ = std::fs::remove_file(&source_path);
    if let Some(wrapper_path) = &wrapper_path {
        let _ = std::fs::remove_file(wrapper_path);
    }
    if !output_status.status.success() {
        anyhow::bail!(
            "native executable build failed for {}:\n{}",
            path.display(),
            String::from_utf8_lossy(&output_status.stderr)
        );
    }
    Ok(())
}

/// The hybrid link wrapper: embeds the artifact JSON and registers it before
/// `main` runs (C constructor), so the first bridge call can decode it lazily.
/// It also hands lk-api the lkrt container constructors (the v2 return
/// bridge's deep conversion): only this wrapper is compiled into binaries
/// that link *both* staticlibs, so it is the one place that can connect them
/// without lk-api depending on lkrt.
fn hybrid_wrapper_c(module_artifact_json: &str) -> String {
    let escaped = c_escape(module_artifact_json);
    format!(
        "typedef struct LkDyn {{ long long tag; long long payload; }} LkDyn;\n\
         extern void lk_hybrid_register(const char *module_artifact_json);\n\
         extern void lk_hybrid_register_rt(void *(*list_dyn_new)(void),\n\
                                           void (*list_dyn_push)(void *, LkDyn),\n\
                                           void *(*map_str_dyn_new)(void),\n\
                                           void (*map_str_dyn_set)(void *, const char *, LkDyn),\n\
                                           void (*raise_dyn)(LkDyn));\n\
         extern void *lkrt_lklist_dyn_new(void);\n\
         extern void lkrt_lklist_dyn_push(void *, LkDyn);\n\
         extern void *lkrt_lkmap_str_dyn_new(void);\n\
         extern void lkrt_lkmap_str_dyn_set(void *, const char *, LkDyn);\n\
         extern void lkrt_rt_raise_dyn(LkDyn);\n\
         static const char *LK_HYBRID_ARTIFACT = \"{escaped}\";\n\
         __attribute__((constructor)) static void lk_hybrid_setup(void) {{\n\
             lk_hybrid_register(LK_HYBRID_ARTIFACT);\n\
             lk_hybrid_register_rt(lkrt_lklist_dyn_new, lkrt_lklist_dyn_push,\n\
                                   lkrt_lkmap_str_dyn_new, lkrt_lkmap_str_dyn_set,\n\
                                   lkrt_rt_raise_dyn);\n\
         }}\n"
    )
}

/// Escape a string for embedding as a C double-quoted string literal.
fn c_escape(s: &str) -> String {
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

fn add_force_load_staticlib(command: &mut Command, staticlib: &Path) {
    if cfg!(target_os = "macos") {
        command.arg(format!("-Wl,-force_load,{}", staticlib.display()));
    } else {
        command
            .arg("-Wl,--whole-archive")
            .arg(staticlib)
            .arg("-Wl,--no-whole-archive");
    }
}

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
    let mut candidates = vec![candidate];
    if let Some(path) = latest_lkrt_staticlib_in_deps(&dir.join("deps")) {
        candidates.push(path);
    }
    newest_existing_path(candidates)
}

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

fn newest_existing_path(paths: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    paths
        .into_iter()
        .filter_map(|path| {
            let modified = path.metadata().ok()?.modified().ok()?;
            Some((modified, path))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

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

fn temp_llvm_source_path(path: &Path) -> PathBuf {
    let stem = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or("lk");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("lk-{stem}-{}-{nanos}.ll", std::process::id()))
}
