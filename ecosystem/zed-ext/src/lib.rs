use std::path::{Path, PathBuf};
use zed_extension_api::{self as zed, settings::LspSettings, LanguageServerId, Result};

struct LkExtension;

impl zed::Extension for LkExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let settings = LspSettings::for_worktree(language_server_id.as_ref(), worktree).ok();
        let binary = settings.as_ref().and_then(|settings| settings.binary.as_ref());
        let args = binary.and_then(|binary| binary.arguments.clone()).unwrap_or_default();

        let command = if let Some(path) = binary.and_then(|binary| binary.path.clone()) {
            path
        } else {
            find_lk_lsp(worktree)?
        };

        Ok(zed::Command {
            command,
            args,
            env: worktree.shell_env(),
        })
    }

    fn language_server_initialization_options(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        LspSettings::for_worktree(language_server_id.as_ref(), worktree).map(|settings| settings.initialization_options)
    }

    fn language_server_workspace_configuration(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        LspSettings::for_worktree(language_server_id.as_ref(), worktree).map(|settings| settings.settings)
    }
}

fn find_lk_lsp(worktree: &zed::Worktree) -> Result<String> {
    let executable = if cfg!(windows) { "lk-lsp.exe" } else { "lk-lsp" };

    for path in repo_candidate_paths(worktree, executable) {
        if is_executable_file(&path) {
            return Ok(path.display().to_string());
        }
    }

    if let Some(path) = worktree.which(executable) {
        return Ok(path);
    }

    Err(format!(
        "LK language server binary was not found. Build it with `cargo build -p lk-lsp`, install it with `cargo install --path lsp --force`, or configure `lsp.lk-lsp.binary.path` in Zed settings."
    ))
}

fn repo_candidate_paths(worktree: &zed::Worktree, executable: &str) -> Vec<PathBuf> {
    let root = PathBuf::from(worktree.root_path());
    let mut paths = Vec::new();

    for ancestor in root.ancestors().take(8) {
        paths.push(ancestor.join("target").join("debug").join(executable));
        paths.push(ancestor.join("target").join("release").join(executable));
    }

    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        paths.push(PathBuf::from(home).join(".cargo").join("bin").join(executable));
    }

    paths.push(PathBuf::from("/opt/homebrew/bin").join(executable));
    paths.push(PathBuf::from("/usr/local/bin").join(executable));
    paths
}

fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
}

zed::register_extension!(LkExtension);
