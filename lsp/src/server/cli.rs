use anyhow::Context;
use std::path::{Component, Path};

use crate::analyzer::LkrAnalyzer;

pub(crate) fn try_cli_analyze() -> anyhow::Result<Option<String>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() <= 1 {
        return Ok(None);
    }

    if let Some(i) = args.iter().position(|a| a == "--analyze") {
        let mut path_index = i + 1;
        while path_index < args.len() && args[path_index].starts_with("--") {
            path_index += 1;
        }

        let path = args.get(path_index).cloned().ok_or_else(|| {
            anyhow::anyhow!("Usage: lkr-lsp --analyze [--errors-only] <relative-file-path>\n  --analyze <file>     : Full analysis with JSON output\n  --errors-only        : Show only errors in simple format")
        })?;

        let errors_only = args.iter().any(|a| a == "--errors-only");
        let content = read_file_content(&path)?;

        let mut analyzer = LkrAnalyzer::new();
        let analysis = analyzer.analyze(&content);

        if errors_only {
            let errors: Vec<String> = analysis
                .diagnostics
                .iter()
                .filter(|d| d.severity == Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR))
                .map(|d| {
                    format!(
                        "Line {}:{}: {}",
                        d.range.start.line + 1,
                        d.range.start.character + 1,
                        d.message
                    )
                })
                .collect();

            if errors.is_empty() {
                return Ok(Some("No errors found".to_string()));
            } else {
                return Ok(Some(errors.join("\n")));
            }
        } else {
            let tokens = analyzer.generate_semantic_tokens(&content);

            let mut id_roots: Vec<String> = analysis.identifier_roots.iter().cloned().collect();
            id_roots.sort();

            let tokens_simple: Vec<[u32; 5]> = tokens
                .iter()
                .map(|t| {
                    [
                        t.delta_line,
                        t.delta_start,
                        t.length,
                        t.token_type,
                        t.token_modifiers_bitset,
                    ]
                })
                .collect();

            let output = serde_json::json!({
                "diagnostics": analysis.diagnostics,
                "symbols": analysis.symbols,
                "identifier_roots": id_roots,
                "semantic_tokens": tokens_simple
            });
            return Ok(Some(serde_json::to_string_pretty(&output)?));
        }
    }

    Ok(None)
}

pub(crate) fn is_safe_path(path: &str) -> bool {
    let path = Path::new(path);

    if path.as_os_str().is_empty() {
        return false;
    }
    if path.is_absolute() {
        return false;
    }
    if path.components().any(|c| c == Component::ParentDir) {
        return false;
    }

    let s = path.to_string_lossy();
    let suspicious = ['\0', '\n', '\r', '\t'];
    if s.chars().any(|c| suspicious.contains(&c)) {
        return false;
    }
    if s.len() >= 2 {
        let bytes = s.as_bytes();
        if bytes[1] == b':' {
            return false;
        }
    }
    true
}

pub(crate) fn read_file_content(path: &str) -> anyhow::Result<String> {
    if !is_safe_path(path) {
        return Err(anyhow::anyhow!("Unsafe file path: {}", path));
    }
    std::fs::read_to_string(path).with_context(|| format!("Failed to read file '{}'", path))
}
