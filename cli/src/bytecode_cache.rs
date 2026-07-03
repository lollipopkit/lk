//! Optional bytecode compile cache for `lk FILE.lk` (plan M1.3).
//!
//! `.lkm` is an internal, build-locked artifact — a *cache*, not a distribution
//! format. This module makes that concrete: with `LK_CACHE=1`, the first run of
//! a source file compiles it to a module artifact stored under `$LK_HOME/cache`
//! (keyed by the source path + bytes + artifact version + CLI version); later
//! runs of the unchanged source skip parsing/macro-expansion/compilation and
//! execute the cached module directly.
//!
//! Caching is **opt-in** so the default run path — and the performance bench —
//! is untouched. Only programs with **no external proc-macro dependencies** are
//! stored: their compiled bytecode is a pure function of the source bytes, so
//! every cache hit is safe to reuse. (Imports are re-resolved fresh on each run,
//! so a changed dependency module is always picked up.)

use std::path::{Path, PathBuf};

use lk_core::package::lk_home;
use lk_core::stmt::{Program, import::collect_program_imports};
use lk_core::vm::{MODULE_ARTIFACT_VERSION, Module, ModuleArtifact};

/// Whether the bytecode cache is enabled (`LK_CACHE=1`).
pub fn enabled() -> bool {
    matches!(
        std::env::var("LK_CACHE").as_deref(),
        Ok("1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON")
    )
}

fn cache_dir() -> PathBuf {
    std::env::var_os("LK_BYTECODE_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| lk_home().join("cache"))
}

fn fnv1a(chunks: &[&[u8]]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for chunk in chunks {
        for byte in *chunk {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash
}

/// The cache file for a source file and its bytes, or `None` when caching is
/// disabled. The key covers the canonical source path, the source bytes, the
/// module artifact version (bumped on any encoding change), and the CLI version,
/// so a stale entry is never mis-decoded or run under a different build.
pub fn cache_path(source_path: &Path, source: &[u8]) -> Option<PathBuf> {
    if !enabled() {
        return None;
    }
    let canonical = source_path.canonicalize().unwrap_or_else(|_| source_path.to_path_buf());
    let key = fnv1a(&[
        canonical.to_string_lossy().as_bytes(),
        source,
        &MODULE_ARTIFACT_VERSION.to_le_bytes(),
        env!("CARGO_PKG_VERSION").as_bytes(),
    ]);
    Some(cache_dir().join(format!("lk-bc-{key:016x}.lkm")))
}

/// Load a cached module artifact, or `None` if it is absent or invalid (a
/// version mismatch, corruption, or a partial write all fall back to a fresh
/// compile).
pub fn load(cache_file: &Path) -> Option<ModuleArtifact> {
    let text = std::fs::read_to_string(cache_file).ok()?;
    ModuleArtifact::from_json_str(&text).ok()
}

/// Best-effort store of a freshly compiled module. Programs with external
/// proc-macro dependencies are skipped by the caller (their expansion is not a
/// pure function of the source). Any I/O or serialization error is swallowed —
/// the cache is an optimization, never a correctness dependency.
pub fn store(cache_file: &Path, program: &Program, module: &Module) {
    let Ok(artifact) = ModuleArtifact::new(collect_program_imports(program), module) else {
        return; // e.g. inline native entries cannot be serialized
    };
    let Ok(text) = artifact.to_json_string() else {
        return;
    };
    if let Some(parent) = cache_file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(cache_file, text);
}
