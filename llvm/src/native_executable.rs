use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::Context;

/// Tier 1 hybrid link info (`docs/llvm/tier1-hybrid.md`): the serialized module
/// artifact to embed (registered via a C constructor so the generated `main`
/// stays untouched) and the lk-api staticlib providing `lk_hybrid_*`.
pub struct HybridLink<'a> {
    pub module_artifact_json: &'a str,
    pub lk_api_staticlib: &'a Path,
}

/// Link a Cranelift-produced relocatable object (already machine code — no
/// `clang` optimization step) into a native executable, against the `lkrt`
/// staticlib. `clang` is used purely as the linker driver.
pub fn compile_native_executable_from_object(path: &Path, output: &Path, object: &[u8]) -> anyhow::Result<()> {
    let _ = lkrt::link_anchor();
    let object_path = temp_llvm_source_path(path).with_extension("o");
    std::fs::write(&object_path, object).with_context(|| format!("write native object {}", object_path.display()))?;
    let clang = clang_command();
    let mut command = Command::new(&clang);
    command.arg(&object_path).arg("-o").arg(output);
    if let Some(sanitizers) = std::env::var_os("LK_NATIVE_SANITIZE")
        && !sanitizers.is_empty()
    {
        command.arg(format!("-fsanitize={}", sanitizers.to_string_lossy()));
    }
    if let Some(staticlib) = lkrt_staticlib_path() {
        add_force_load_staticlib(&mut command, &staticlib);
    }
    // `pthread`/`dl` are Unix libraries; on Windows they are part of the CRT.
    if !cfg!(target_os = "windows") {
        command.args(["-lpthread", "-ldl"]);
    }
    if cfg!(target_os = "linux") {
        command.arg("-lm");
    }
    if cfg!(target_os = "macos") {
        command.args(["-framework", "CoreFoundation"]);
    }
    // Remove the temp object whether or not clang spawns, so a spawn failure
    // does not leak it.
    let status = command
        .output()
        .with_context(|| format!("spawn clang to link native object {}", output.display()));
    let _ = std::fs::remove_file(&object_path);
    let status = status?;
    if !status.status.success() {
        anyhow::bail!(
            "native object link failed for {}:\n{}",
            path.display(),
            String::from_utf8_lossy(&status.stderr)
        );
    }
    Ok(())
}

/// Link a Cranelift-produced object that references the Tier 1 bridge
/// (`lk_hybrid_call_*`) into a native executable: like
/// [`compile_native_executable_from_object`], but also compiles the hybrid
/// wrapper (embedded module artifact + rt registration) and links the lk-api
/// staticlib alongside `lkrt`.
pub fn compile_native_executable_from_object_hybrid(
    path: &Path,
    output: &Path,
    object: &[u8],
    hybrid: HybridLink<'_>,
) -> anyhow::Result<()> {
    let _ = lkrt::link_anchor();
    let object_path = temp_llvm_source_path(path).with_extension("o");
    std::fs::write(&object_path, object).with_context(|| format!("write native object {}", object_path.display()))?;
    let wrapper = hybrid_wrapper_c(hybrid.module_artifact_json);
    let wrapper_path = object_path.with_extension("hybrid.c");
    if let Err(error) = std::fs::write(&wrapper_path, wrapper) {
        let _ = std::fs::remove_file(&object_path);
        return Err(error).with_context(|| format!("write hybrid wrapper {}", wrapper_path.display()));
    }
    let clang = clang_command();
    let mut command = Command::new(&clang);
    command.arg(&object_path).arg(&wrapper_path).arg("-o").arg(output);
    if let Some(sanitizers) = std::env::var_os("LK_NATIVE_SANITIZE")
        && !sanitizers.is_empty()
    {
        command.arg(format!("-fsanitize={}", sanitizers.to_string_lossy()));
    }
    if let Some(staticlib) = lkrt_staticlib_path() {
        add_force_load_staticlib(&mut command, &staticlib);
    }
    // The bridge VM (lk-api + lk-core + stdlib) rides in via its staticlib; the
    // wrapper references `lk_hybrid_register`, the object references
    // `lk_hybrid_call_*`, which pull the objects in.
    command.arg(hybrid.lk_api_staticlib);
    // `pthread`/`dl` are Unix libraries; on Windows they are part of the CRT.
    if !cfg!(target_os = "windows") {
        command.args(["-lpthread", "-ldl"]);
    }
    if cfg!(target_os = "linux") {
        command.arg("-lm");
    }
    if cfg!(target_os = "macos") {
        command.args(["-framework", "CoreFoundation"]);
    }
    let status = command
        .output()
        .with_context(|| format!("spawn clang to link hybrid native object {}", output.display()));
    let _ = std::fs::remove_file(&object_path);
    let _ = std::fs::remove_file(&wrapper_path);
    let status = status?;
    if !status.status.success() {
        anyhow::bail!(
            "hybrid native object link failed for {}:\n{}",
            path.display(),
            String::from_utf8_lossy(&status.stderr)
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
    let mut candidates = vec![dir.join(file)];
    // The `lk` CLI runs from `target/<profile>/`, whose `deps` subdir holds the
    // hashed `liblkrt-<hash>.a`; a `cargo test` binary runs from
    // `target/<profile>/deps/` itself, where that hashed archive sits right
    // beside it. Search both so either launcher resolves the staticlib.
    for search in [dir.to_path_buf(), dir.join("deps")] {
        if let Some(path) = latest_lkrt_staticlib_in(&search) {
            candidates.push(path);
        }
    }
    newest_existing_path(candidates)
}

fn latest_lkrt_staticlib_in(deps_dir: &Path) -> Option<PathBuf> {
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
