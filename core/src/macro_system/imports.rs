#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use std::path::{Component, Path, PathBuf};

#[cfg(feature = "std")]
use crate::package::PackageGraph;
use crate::{
    token::{ParseError, Token, Tokenizer},
    util::fast_map::FastHashMap,
};

use super::{
    MacroDef, MacroExportItem, MacroRegistry, SourceToken, error_at, expect_id, find_group, macro_rules_start_at,
    parse_macro_def, parse_macro_export_list_at, token_lexeme,
};

const BUILTIN_MACRO_MODULE: &str = "macros";
const BUILTIN_MACRO_SOURCE: &str = r#"
export macro_rules! vec {
    ($($value:expr),*) => { [$($value),*] };
}

export macro_rules! assert {
    ($cond:expr) => { assert($cond) };
    ($cond:expr, $message:expr) => { assert($cond, $message) };
}

export macro_rules! assert_eq {
    ($actual:expr, $expected:expr) => { assert_eq($actual, $expected) };
    ($actual:expr, $expected:expr, $message:expr) => { assert_eq($actual, $expected, $message) };
}

export macro_rules! assert_ne {
    ($actual:expr, $expected:expr) => { assert_ne($actual, $expected) };
    ($actual:expr, $expected:expr, $message:expr) => { assert_ne($actual, $expected, $message) };
}

export macro_rules! matches {
    ($value:expr, $pattern:pat) => { match $value { $pattern => true, _ => false } };
}

export macro_rules! panic {
    () => { panic() };
    ($message:expr) => { panic($message) };
}

export macro_rules! todo {
    () => { panic("not yet implemented") };
    ($message:expr) => { panic($message) };
}

export macro_rules! unreachable {
    () => { panic("entered unreachable code") };
    ($message:expr) => { panic($message) };
}
"#;

#[derive(Debug)]
struct MacroImportSpec {
    source: MacroImportSource,
    kind: MacroImportKind,
    span_index: usize,
}

#[derive(Debug)]
enum MacroImportSource {
    File(String),
    Module(String),
}

#[derive(Debug, Clone)]
pub(in crate::macro_system) enum MacroRuntimeAnchorSource {
    File(String),
    Module(String),
}

impl MacroImportSource {
    fn runtime_anchor_source(&self) -> Option<MacroRuntimeAnchorSource> {
        match self {
            Self::File(path) => Some(MacroRuntimeAnchorSource::File(path.clone())),
            Self::Module(name) if is_builtin_macro_module(name) => None,
            Self::Module(name) => Some(MacroRuntimeAnchorSource::Module(name.clone())),
        }
    }
}

#[derive(Debug)]
enum MacroImportKind {
    Named(Vec<MacroImportItem>),
    Namespace { alias: String },
}

#[derive(Debug)]
struct MacroImportItem {
    name: String,
    alias: String,
}

#[derive(Default)]
struct MacroModule {
    macros: FastHashMap<String, MacroDef>,
    exports: FastHashMap<String, MacroDef>,
    anchors: FastHashMap<String, MacroDef>,
    local_names: Vec<String>,
    imported_runtime_anchors: Vec<(String, MacroRuntimeAnchorSource)>,
    crate_anchor: Option<String>,
}

impl MacroModule {
    fn into_loaded(self) -> LoadedMacroModule {
        LoadedMacroModule {
            public: self.exports,
            anchors: self.anchors,
            runtime_anchors: self.imported_runtime_anchors,
            crate_anchor: self.crate_anchor,
        }
    }
}

#[derive(Default)]
struct LoadedMacroModule {
    public: FastHashMap<String, MacroDef>,
    anchors: FastHashMap<String, MacroDef>,
    runtime_anchors: Vec<(String, MacroRuntimeAnchorSource)>,
    crate_anchor: Option<String>,
}

pub(super) fn collect_imported_macro_defs(
    tokens: &[SourceToken],
    base_dir: Option<&Path>,
    registry: &mut MacroRegistry,
    loading: &mut Vec<PathBuf>,
) -> Result<(), ParseError> {
    for spec in macro_import_specs(tokens)? {
        let loaded = load_imported_macros(base_dir, &spec, tokens, loading)?;
        register_anchor_macros(registry, &loaded.anchors);
        for (anchor, source) in loaded.runtime_anchors.iter().cloned() {
            registry.insert_runtime_anchor(anchor, source);
        }
        if let (Some(anchor), Some(source)) = (loaded.crate_anchor.clone(), spec.source.runtime_anchor_source()) {
            registry.insert_runtime_anchor(anchor, source);
        }
        register_file_macros(registry, &loaded.public, &spec, tokens)?;
    }
    Ok(())
}

pub(super) fn is_builtin_macro_module(name: &str) -> bool {
    name == BUILTIN_MACRO_MODULE
}

pub(super) fn compile_time_macro_import_end_at(
    tokens: &[SourceToken],
    index: usize,
    registry: &MacroRegistry,
) -> Result<Option<usize>, ParseError> {
    if !matches!(tokens.get(index).map(|token| &token.token), Some(Token::Use)) {
        return Ok(None);
    }
    match tokens.get(index + 1).map(|token| &token.token) {
        Some(Token::LBrace) => {
            let (_, end) = find_group(tokens, index + 1)?;
            if !matches!(tokens.get(end + 1).map(|token| &token.token), Some(Token::From))
                || !matches!(
                    tokens.get(end + 2).map(|token| &token.token),
                    Some(Token::Str(_) | Token::Id(_))
                )
            {
                return Ok(None);
            }
            let aliases = parse_named_macro_import_items(tokens, index + 1, &tokens[index + 2..end])?
                .into_iter()
                .map(|item| item.alias)
                .collect::<Vec<_>>();
            if aliases.iter().any(|alias| registry.contains_macro(alias)) {
                return Ok(Some(use_statement_end(tokens, end + 3)));
            }
        }
        Some(Token::Id(module)) if is_builtin_macro_module(module) => {
            let alias = if matches!(tokens.get(index + 2).map(|token| &token.token), Some(Token::As)) {
                match tokens.get(index + 3).map(|token| &token.token) {
                    Some(Token::Id(alias)) => alias,
                    _ => module,
                }
            } else {
                module
            };
            if registry.contains_macro_namespace(alias) {
                return Ok(Some(use_statement_end(tokens, index + 2)));
            }
        }
        Some(Token::Mul) => {
            if matches!(tokens.get(index + 2).map(|token| &token.token), Some(Token::As))
                && let Some(Token::Id(alias)) = tokens.get(index + 3).map(|token| &token.token)
                && matches!(tokens.get(index + 4).map(|token| &token.token), Some(Token::From))
                && let Some(Token::Id(module)) = tokens.get(index + 5).map(|token| &token.token)
                && is_builtin_macro_module(module)
                && registry.contains_macro_namespace(alias)
            {
                return Ok(Some(use_statement_end(tokens, index + 6)));
            }
        }
        _ => {}
    }
    Ok(None)
}

fn register_file_macros(
    registry: &mut MacroRegistry,
    file_macros: &FastHashMap<String, MacroDef>,
    spec: &MacroImportSpec,
    tokens: &[SourceToken],
) -> Result<(), ParseError> {
    match &spec.kind {
        MacroImportKind::Named(items) => {
            for item in items {
                if let Some(definition) = file_macros.get(&item.name) {
                    registry.insert_macro(item.alias.clone(), definition.clone(), tokens, spec.span_index)?;
                }
            }
        }
        MacroImportKind::Namespace { alias } => {
            for (name, definition) in file_macros {
                registry.insert_macro(format!("{alias}::{name}"), definition.clone(), tokens, spec.span_index)?;
            }
        }
    }
    Ok(())
}

fn load_imported_macros(
    base_dir: Option<&Path>,
    spec: &MacroImportSpec,
    tokens: &[SourceToken],
    loading: &mut Vec<PathBuf>,
) -> Result<LoadedMacroModule, ParseError> {
    match &spec.source {
        MacroImportSource::File(path) => {
            let Some(base_dir) = base_dir else {
                return Ok(LoadedMacroModule::default());
            };
            let resolved = resolve_macro_import_path(base_dir, path)
                .map_err(|message| error_at(tokens, spec.span_index, &message))?;
            load_macro_file(&resolved, loading)
        }
        MacroImportSource::Module(name) => {
            if let Some(macros) = load_builtin_macro_module(name, loading)? {
                return Ok(macros);
            }
            let Some(base_dir) = base_dir else {
                return Ok(LoadedMacroModule::default());
            };
            // Package-based macro imports need the `package` manager, which is
            // gated out of the no_std VM-core surface (plan M0.7/8).
            #[cfg(feature = "std")]
            let resolved = resolve_package_macro_module(base_dir, name, tokens, spec.span_index)?;
            #[cfg(not(feature = "std"))]
            let resolved: Option<PathBuf> = {
                let _ = (base_dir, name, &tokens, spec.span_index);
                None
            };
            let Some(resolved) = resolved else {
                return Ok(LoadedMacroModule::default());
            };
            load_macro_file(&resolved, loading)
        }
    }
}

fn load_macro_file(path: &Path, loading: &mut Vec<PathBuf>) -> Result<LoadedMacroModule, ParseError> {
    let canonical = path.canonicalize();
    let path = match &canonical {
        Ok(p) => p.clone(),
        Err(e) => {
            // Canonicalization may fail for non-existent or inaccessible paths.
            // Normalize to an absolute path so comparisons remain consistent.
            eprintln!(
                "warning: macro import path canonicalization failed for {}: {e}",
                path.display()
            );
            path.to_path_buf()
        }
    };
    if loading.iter().any(|entry| entry == &path) {
        return Ok(LoadedMacroModule::default());
    }
    loading.push(path.clone());
    let result = load_macro_file_inner(&path, loading);
    loading.pop();
    result
}

fn load_macro_file_inner(path: &Path, loading: &mut Vec<PathBuf>) -> Result<LoadedMacroModule, ParseError> {
    let source = std::fs::read_to_string(path)
        .map_err(|error| ParseError::new(format!("Failed to read macro import '{}': {error}", path.display())))?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let crate_anchor = macro_crate_anchor_for_label(&path.display().to_string());
    load_macro_source(&source, base_dir, loading, crate_anchor)
}

fn load_builtin_macro_module(name: &str, loading: &mut Vec<PathBuf>) -> Result<Option<LoadedMacroModule>, ParseError> {
    if !is_builtin_macro_module(name) {
        return Ok(None);
    }
    Ok(Some(load_macro_source(
        BUILTIN_MACRO_SOURCE,
        Path::new("."),
        loading,
        macro_crate_anchor_for_label(BUILTIN_MACRO_MODULE),
    )?))
}

fn load_macro_source(
    source: &str,
    base_dir: &Path,
    loading: &mut Vec<PathBuf>,
    crate_anchor: String,
) -> Result<LoadedMacroModule, ParseError> {
    let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(source)?;
    let source_tokens = tokens
        .into_iter()
        .zip(spans)
        .map(|(token, span)| {
            let lexeme = token_lexeme(&token);
            SourceToken {
                token,
                span,
                lexeme,
                origins: Vec::new(),
            }
        })
        .collect::<Vec<_>>();
    let mut module = MacroModule {
        crate_anchor: Some(crate_anchor.clone()),
        ..Default::default()
    };
    for spec in macro_import_specs(&source_tokens)? {
        let imported = load_imported_macros(Some(base_dir), &spec, &source_tokens, loading)?;
        register_file_macro_map(&mut module.macros, &imported.public, &spec, &source_tokens)?;
        merge_anchor_macros(&mut module.anchors, &imported.anchors);
        module.imported_runtime_anchors.extend(imported.runtime_anchors);
        if let (Some(anchor), Some(source)) = (imported.crate_anchor, spec.source.runtime_anchor_source()) {
            module.imported_runtime_anchors.push((anchor, source));
        }
    }
    let mut exports = Vec::new();
    let mut index = 0usize;
    while index < source_tokens.len() {
        if let Some((macro_start, exported)) = macro_rules_start_at(&source_tokens, index) {
            let (mut definition, next) = parse_macro_def(&source_tokens, macro_start)?;
            definition.crate_anchor = Some(crate_anchor.clone());
            let name = definition.name.clone();
            insert_file_macro(&mut module.macros, name.clone(), definition, &source_tokens, index)?;
            module.local_names.push(name.clone());
            if exported {
                exports.push(MacroExportItem {
                    name: name.clone(),
                    alias: name,
                    span_index: index,
                });
            }
            index = next;
        } else if let Some((items, next)) = parse_macro_export_list_at(&source_tokens, index)? {
            exports.extend(items);
            index = next;
        } else {
            index += 1;
        }
    }
    module.exports = exported_file_macros(&module.macros, &exports, &source_tokens)?;
    merge_anchor_macros(
        &mut module.anchors,
        &anchored_file_macros(&module.macros, &module.local_names, &crate_anchor, &source_tokens)?,
    );
    Ok(module.into_loaded())
}

pub(super) fn local_macro_crate_anchor() -> String {
    macro_crate_anchor_for_label("local")
}

/// Computes a deterministic crate anchor from a label using a stable hash.
/// Uses FxHash (multiply-xor) which produces the same value for the same input
/// regardless of Rust version or process, unlike `DefaultHasher` which is
/// non-deterministic across versions. The anchor is stable enough for the
/// macro expansion use case and avoids pulling in a cryptographic dependency.
fn macro_crate_anchor_for_label(label: &str) -> String {
    // FxHash: deterministic, fast, and stable across Rust versions.
    let mut hash = 0_u64;
    for byte in label.bytes() {
        hash = hash.wrapping_mul(0x517cc1b727220a95).wrapping_add(byte as u64);
    }
    format!("__lk_macro_crate_{:016x}", hash)
}

fn register_anchor_macros(registry: &mut MacroRegistry, anchors: &FastHashMap<String, MacroDef>) {
    for (name, definition) in anchors {
        registry.insert_macro_if_absent(name.clone(), definition.clone());
    }
}

fn merge_anchor_macros(target: &mut FastHashMap<String, MacroDef>, source: &FastHashMap<String, MacroDef>) {
    for (name, definition) in source {
        target.entry(name.clone()).or_insert_with(|| definition.clone());
    }
}

fn anchored_file_macros(
    macros: &FastHashMap<String, MacroDef>,
    local_names: &[String],
    crate_anchor: &str,
    tokens: &[SourceToken],
) -> Result<FastHashMap<String, MacroDef>, ParseError> {
    let mut anchors = FastHashMap::default();
    for name in local_names {
        if let Some(definition) = macros.get(name) {
            insert_file_macro(
                &mut anchors,
                format!("{crate_anchor}::{name}"),
                definition.clone(),
                tokens,
                0,
            )?;
        }
    }
    Ok(anchors)
}

fn exported_file_macros(
    macros: &FastHashMap<String, MacroDef>,
    exports: &[MacroExportItem],
    tokens: &[SourceToken],
) -> Result<FastHashMap<String, MacroDef>, ParseError> {
    let mut public = FastHashMap::default();
    for item in exports {
        let definition = macros.get(&item.name).ok_or_else(|| {
            error_at(
                tokens,
                item.span_index,
                &format!("Cannot export unknown macro `{}`", item.name),
            )
        })?;
        insert_file_macro(
            &mut public,
            item.alias.clone(),
            definition.clone(),
            tokens,
            item.span_index,
        )?;
    }
    Ok(public)
}

fn register_file_macro_map(
    macros: &mut FastHashMap<String, MacroDef>,
    imported: &FastHashMap<String, MacroDef>,
    spec: &MacroImportSpec,
    tokens: &[SourceToken],
) -> Result<(), ParseError> {
    match &spec.kind {
        MacroImportKind::Named(items) => {
            for item in items {
                if let Some(definition) = imported.get(&item.name) {
                    insert_file_macro(macros, item.alias.clone(), definition.clone(), tokens, spec.span_index)?;
                }
            }
        }
        MacroImportKind::Namespace { alias } => {
            for (name, definition) in imported {
                insert_file_macro(
                    macros,
                    format!("{alias}::{name}"),
                    definition.clone(),
                    tokens,
                    spec.span_index,
                )?;
            }
        }
    }
    Ok(())
}

fn insert_file_macro(
    macros: &mut FastHashMap<String, MacroDef>,
    name: String,
    mut definition: MacroDef,
    tokens: &[SourceToken],
    index: usize,
) -> Result<(), ParseError> {
    if macros.contains_key(&name) {
        return Err(error_at(
            tokens,
            index,
            &format!("Macro `{name}` is already defined in this macro module"),
        ));
    }
    definition.name = name.clone();
    macros.insert(name, definition);
    Ok(())
}

fn macro_import_specs(tokens: &[SourceToken]) -> Result<Vec<MacroImportSpec>, ParseError> {
    let mut specs = Vec::new();
    let mut index = 0usize;
    while index < tokens.len() {
        if !matches!(tokens.get(index).map(|token| &token.token), Some(Token::Use)) {
            index += 1;
            continue;
        }
        match tokens.get(index + 1).map(|token| &token.token) {
            Some(Token::Str(path)) => {
                let alias = default_namespace_alias(path).ok_or_else(|| {
                    error_at(
                        tokens,
                        index + 1,
                        &format!("Cannot derive macro namespace alias from import path `{path}`"),
                    )
                })?;
                specs.push(MacroImportSpec {
                    source: MacroImportSource::File(path.clone()),
                    kind: MacroImportKind::Namespace { alias },
                    span_index: index + 1,
                });
            }
            Some(Token::Id(module)) => {
                let alias = if matches!(tokens.get(index + 2).map(|token| &token.token), Some(Token::As)) {
                    match tokens.get(index + 3).map(|token| &token.token) {
                        Some(Token::Id(alias)) => alias.clone(),
                        _ => module.clone(),
                    }
                } else {
                    module.clone()
                };
                specs.push(MacroImportSpec {
                    source: MacroImportSource::Module(module.clone()),
                    kind: MacroImportKind::Namespace { alias },
                    span_index: index + 1,
                });
            }
            Some(Token::LBrace) => {
                let (_, end) = find_group(tokens, index + 1)?;
                if matches!(tokens.get(end + 1).map(|token| &token.token), Some(Token::From)) {
                    match tokens.get(end + 2).map(|token| &token.token) {
                        Some(Token::Str(path)) => {
                            specs.push(MacroImportSpec {
                                source: MacroImportSource::File(path.clone()),
                                kind: MacroImportKind::Named(parse_named_macro_import_items(
                                    tokens,
                                    index + 1,
                                    &tokens[index + 2..end],
                                )?),
                                span_index: end + 2,
                            });
                        }
                        Some(Token::Id(module)) => {
                            specs.push(MacroImportSpec {
                                source: MacroImportSource::Module(module.clone()),
                                kind: MacroImportKind::Named(parse_named_macro_import_items(
                                    tokens,
                                    index + 1,
                                    &tokens[index + 2..end],
                                )?),
                                span_index: end + 2,
                            });
                        }
                        _ => {}
                    }
                }
            }
            Some(Token::Mul) => {
                if matches!(tokens.get(index + 2).map(|token| &token.token), Some(Token::As))
                    && let Some(Token::Id(alias)) = tokens.get(index + 3).map(|token| &token.token)
                    && matches!(tokens.get(index + 4).map(|token| &token.token), Some(Token::From))
                    && let Some(source) = tokens.get(index + 5).map(|token| &token.token)
                {
                    match source {
                        Token::Str(path) => {
                            specs.push(MacroImportSpec {
                                source: MacroImportSource::File(path.clone()),
                                kind: MacroImportKind::Namespace { alias: alias.clone() },
                                span_index: index + 5,
                            });
                        }
                        Token::Id(module) => {
                            specs.push(MacroImportSpec {
                                source: MacroImportSource::Module(module.clone()),
                                kind: MacroImportKind::Namespace { alias: alias.clone() },
                                span_index: index + 5,
                            });
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        index += 1;
    }
    Ok(specs)
}

fn parse_named_macro_import_items(
    all_tokens: &[SourceToken],
    group_index: usize,
    tokens: &[SourceToken],
) -> Result<Vec<MacroImportItem>, ParseError> {
    let mut items = Vec::new();
    let mut index = 0usize;
    while index < tokens.len() {
        if matches!(tokens[index].token, Token::Comma) {
            index += 1;
            continue;
        }
        let name = expect_id(tokens, index, "Expected macro name in macro import list")?;
        let mut alias = name.clone();
        index += 1;
        if matches!(tokens.get(index).map(|token| &token.token), Some(Token::As)) {
            alias = expect_id(tokens, index + 1, "Expected macro alias after `as`")?;
            index += 2;
        }
        items.push(MacroImportItem { name, alias });
        if index < tokens.len() {
            if matches!(tokens[index].token, Token::Comma) {
                index += 1;
            } else {
                return Err(error_at(
                    all_tokens,
                    group_index,
                    "Expected `,` between macro import names",
                ));
            }
        }
    }
    Ok(items)
}

fn default_namespace_alias(raw: &str) -> Option<String> {
    let stem = Path::new(raw).file_stem()?.to_str()?;
    if stem.is_empty() { None } else { Some(stem.to_string()) }
}

#[cfg(feature = "std")]
fn resolve_package_macro_module(
    base_dir: &Path,
    name: &str,
    tokens: &[SourceToken],
    index: usize,
) -> Result<Option<PathBuf>, ParseError> {
    let graph = PackageGraph::discover(base_dir).map_err(|error| {
        error_at(
            tokens,
            index,
            &format!("Failed to discover macro package graph: {error}"),
        )
    })?;
    let Some(graph) = graph else {
        return Ok(None);
    };
    Ok(graph
        .modules
        .into_iter()
        .find(|module| module.name == name)
        .map(|module| module.root))
}

fn use_statement_end(tokens: &[SourceToken], mut index: usize) -> usize {
    while index < tokens.len() {
        let current = index;
        index += 1;
        if matches!(tokens[current].token, Token::Semicolon) {
            break;
        }
    }
    index
}

fn resolve_macro_import_path(base_dir: &Path, raw: &str) -> Result<PathBuf, String> {
    let path = Path::new(raw);
    if !path.is_relative() {
        return Err(format!(
            "Absolute paths are not allowed for macro imports: {}",
            path.display()
        ));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "Parent directory components are not allowed for macro imports: {}",
            path.display()
        ));
    }
    let candidates = if path.extension().and_then(|ext| ext.to_str()) == Some("lk") {
        vec![base_dir.join(path)]
    } else {
        vec![
            base_dir.join(path.with_extension("lk")),
            base_dir.join(path).join("mod.lk"),
        ]
    };
    candidates
        .into_iter()
        .find(|candidate| candidate.exists())
        .ok_or_else(|| {
            format!(
                "File not found for macro import '{}': expected '{}.lk' or '{}/mod.lk'",
                path.display(),
                path.display(),
                path.display()
            )
        })
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use crate::{
        stmt::ModuleResolver,
        syntax::{ParseOptions, expand_source, parse_program_source, render_tokens},
        vm::{VmContext, execute_source},
    };

    fn write_package(root: &Path, name: &str, body: &str) {
        let package_root = root.join("deps").join(name);
        fs::create_dir_all(package_root.join("src")).expect("create package src");
        fs::write(
            package_root.join("Lk.toml"),
            format!(
                r#"
[package]
name = "{name}"
"#
            ),
        )
        .expect("write package manifest");
        fs::write(package_root.join("src/mod.lk"), body).expect("write package module");
    }

    fn write_app_manifest(root: &Path, package_name: &str) -> std::path::PathBuf {
        let app_src = root.join("src");
        fs::create_dir_all(&app_src).expect("create app src");
        fs::write(
            root.join("Lk.toml"),
            format!(
                r#"
[package]
name = "app"

[dependencies]
{package_name} = {{ path = "deps/{package_name}" }}
"#
            ),
        )
        .expect("write app manifest");
        app_src
    }

    fn write_workspace(root: &Path) -> std::path::PathBuf {
        let app_src = root.join("apps/app/src");
        fs::create_dir_all(&app_src).expect("create app src");
        fs::create_dir_all(root.join("crates/util/src")).expect("create workspace util src");
        fs::write(
            root.join("Lk.toml"),
            r#"
[workspace]
members = ["apps/*", "crates/*"]
"#,
        )
        .expect("write workspace manifest");
        fs::write(
            root.join("apps/app/Lk.toml"),
            r#"
[package]
name = "app"
"#,
        )
        .expect("write app manifest");
        fs::write(
            root.join("crates/util/Lk.toml"),
            r#"
[package]
name = "util"
"#,
        )
        .expect("write util manifest");
        fs::write(
            root.join("crates/util/src/mod.lk"),
            r#"
export macro_rules! answer {
    () => { 42 };
}
"#,
        )
        .expect("write workspace util module");
        app_src
    }

    #[test]
    fn package_named_macro_import_expands_and_is_compile_time_only() {
        let temp = tempfile::tempdir().expect("temp dir");
        write_package(
            temp.path(),
            "util",
            r#"
export macro_rules! answer {
    () => { 42 };
}
"#,
        );
        let app_src = write_app_manifest(temp.path(), "util");
        let program = parse_program_source(
            r#"
use { answer } from util;
return answer!();
"#,
            ParseOptions {
                base_dir: Some(app_src),
                ..ParseOptions::default()
            },
        )
        .expect("package macro import should parse");
        assert_eq!(program.statements.len(), 1);
    }

    #[test]
    fn package_namespace_macro_import_expands() {
        let temp = tempfile::tempdir().expect("temp dir");
        write_package(
            temp.path(),
            "util",
            r#"
export macro_rules! answer {
    () => { 42 };
}
"#,
        );
        let app_src = write_app_manifest(temp.path(), "util");
        let expanded = expand_source(
            r#"
use util;
return util::answer!();
"#,
            ParseOptions {
                base_dir: Some(app_src),
                ..ParseOptions::default()
            },
        )
        .expect("package namespace macro import should expand");
        let rendered = render_tokens(&expanded.tokens);
        assert!(rendered.contains("use util;"));
        assert!(rendered.contains("return 42;"));
    }

    #[test]
    fn package_alias_macro_namespace_expands() {
        let temp = tempfile::tempdir().expect("temp dir");
        write_package(
            temp.path(),
            "util",
            r#"
export macro_rules! answer {
    () => { 42 };
}
"#,
        );
        let app_src = write_app_manifest(temp.path(), "util");
        let expanded = expand_source(
            r#"
use util as u;
return u::answer!();
"#,
            ParseOptions {
                base_dir: Some(app_src),
                ..ParseOptions::default()
            },
        )
        .expect("package alias macro import should expand");
        assert!(render_tokens(&expanded.tokens).contains("return 42;"));
    }

    #[test]
    fn workspace_member_macro_import_expands() {
        let temp = tempfile::tempdir().expect("temp dir");
        let app_src = write_workspace(temp.path());
        let expanded = expand_source(
            r#"
use { answer } from util;
return answer!();
"#,
            ParseOptions {
                base_dir: Some(app_src),
                ..ParseOptions::default()
            },
        )
        .expect("workspace member macro import should expand");
        assert!(render_tokens(&expanded.tokens).contains("return 42;"));
    }

    #[test]
    fn file_macro_reexport_alias_expands() {
        let temp = tempfile::tempdir().expect("temp dir");
        fs::write(
            temp.path().join("macros.lk"),
            r#"
macro_rules! internal {
    () => { 42 };
}
export { internal as public };
"#,
        )
        .expect("write macro module");
        let expanded = expand_source(
            r#"
use { public } from "macros";
return public!();
"#,
            ParseOptions {
                base_dir: Some(temp.path().to_path_buf()),
                ..ParseOptions::default()
            },
        )
        .expect("re-exported macro alias should expand");
        assert!(render_tokens(&expanded.tokens).contains("return 42;"));
    }

    #[test]
    fn exported_macro_can_call_private_helper_through_crate_anchor() {
        let temp = tempfile::tempdir().expect("temp dir");
        fs::write(
            temp.path().join("macros.lk"),
            r#"
macro_rules! helper {
    () => { 40 };
}

export macro_rules! answer {
    () => { $crate::helper!() + 2 };
}
"#,
        )
        .expect("write macro module");
        let program = parse_program_source(
            r#"
use { answer } from "macros";
return answer!();
"#,
            ParseOptions {
                base_dir: Some(temp.path().to_path_buf()),
                ..ParseOptions::default()
            },
        )
        .expect("public macro should expand through private helper anchor");
        let result = program.execute().expect("expanded program should execute");
        assert_eq!(result.display_first_return(), "42");
    }

    #[test]
    fn file_macro_import_requires_explicit_export() {
        let temp = tempfile::tempdir().expect("temp dir");
        fs::write(
            temp.path().join("macros.lk"),
            r#"
macro_rules! answer {
    () => { 42 };
}
"#,
        )
        .expect("write macro module");
        let expanded = expand_source(
            r#"
use { answer } from "macros";
return answer!();
"#,
            ParseOptions {
                base_dir: Some(temp.path().to_path_buf()),
                ..ParseOptions::default()
            },
        )
        .expect("private macro import should leave invocation untouched");
        let rendered = render_tokens(&expanded.tokens);
        assert!(rendered.contains("return answer !"));
        assert!(
            parse_program_source(
                r#"
                use { answer } from "macros";
                return answer!();
                "#,
                ParseOptions {
                    base_dir: Some(temp.path().to_path_buf()),
                    ..ParseOptions::default()
                },
            )
            .is_err()
        );
    }

    #[test]
    fn namespace_macro_import_exposes_only_exports() {
        let temp = tempfile::tempdir().expect("temp dir");
        fs::write(
            temp.path().join("macros.lk"),
            r#"
export macro_rules! public {
    () => { 42 };
}

macro_rules! hidden {
    () => { 99 };
}
"#,
        )
        .expect("write macro module");
        let expanded = expand_source(
            r#"
use "macros";
let value = macros::public!();
return macros::hidden!();
"#,
            ParseOptions {
                base_dir: Some(temp.path().to_path_buf()),
                ..ParseOptions::default()
            },
        )
        .expect("public macro should expand and private macro should remain untouched");
        let rendered = render_tokens(&expanded.tokens);
        assert!(rendered.contains("let value = 42;"));
        assert!(rendered.contains("return macros::hidden !"));
    }

    #[test]
    fn macro_export_list_rejects_unknown_macro() {
        let temp = tempfile::tempdir().expect("temp dir");
        fs::write(temp.path().join("macros.lk"), "export { missing };\n").expect("write macro module");
        let err = expand_source(
            r#"
use { missing } from "macros";
return missing!();
"#,
            ParseOptions {
                base_dir: Some(temp.path().to_path_buf()),
                ..ParseOptions::default()
            },
        )
        .expect_err("unknown macro export should fail while loading macro module");
        assert!(err.to_string().contains("Cannot export unknown macro `missing`"));
    }

    #[test]
    fn builtin_named_macro_import_is_compile_time_only_without_base_dir() {
        let result = execute_source(
            r#"
use { vec, matches as is_match } from macros;
let values = vec![1, 2 + 3, 4];
return is_match!(values.1, 5);
"#,
        )
        .expect("builtin named macro imports should expand without runtime imports");
        assert_eq!(result.display_first_return(), "true");
    }

    #[test]
    fn builtin_assertion_macros_expand_to_globals() {
        let expanded = expand_source(
            r#"
use { assert_eq, assert_ne } from macros;
assert_eq!(1, 1.0);
assert_eq!(["a", 2], ["a", 2.0], "numeric equality should coerce");
assert_ne!(1, 2);
return 42;
"#,
            ParseOptions::default(),
        )
        .expect("builtin assertion macros should expand to global assertion functions");
        let rendered = render_tokens(&expanded.tokens);
        assert!(rendered.contains("assert_eq (1, 1);"), "expanded source: {rendered}");
        assert!(
            rendered.contains("assert_eq ([\"a\", 2], [\"a\", 2], \"numeric equality should coerce\");"),
            "expanded source: {rendered}"
        );
        assert!(rendered.contains("assert_ne (1, 2);"), "expanded source: {rendered}");
    }

    #[test]
    fn builtin_panic_family_macros_expand_to_global_panic() {
        let expanded = expand_source(
            r#"
use { panic as fail, todo, unreachable } from macros;
fail!("boom");
todo!();
unreachable!("bad path");
"#,
            ParseOptions::default(),
        )
        .expect("panic-family macros should expand");
        let rendered = render_tokens(&expanded.tokens);
        assert!(rendered.contains("panic (\"boom\");"), "expanded source: {rendered}");
        assert!(
            rendered.contains("panic (\"not yet implemented\");"),
            "expanded source: {rendered}"
        );
        assert!(
            rendered.contains("panic (\"bad path\");"),
            "expanded source: {rendered}"
        );
    }

    #[test]
    fn builtin_macro_namespace_import_is_compile_time_only() {
        let result = execute_source(
            r#"
use macros;
let values = macros::vec![1, 2, 3];
return macros::matches!(values.1, 2);
"#,
        )
        .expect("builtin macro namespace should not reach runtime imports");
        assert_eq!(result.display_first_return(), "true");
    }

    #[test]
    fn builtin_macro_namespace_alias_import_is_compile_time_only() {
        let result = execute_source(
            r#"
use * as m from macros;
let values = m::vec![1, 2, 3];
return m::matches!(values.2, 3);
"#,
        )
        .expect("builtin macro namespace alias should not reach runtime imports");
        assert_eq!(result.display_first_return(), "true");
    }

    #[test]
    fn runtime_file_item_import_without_macro_is_not_rejected() {
        let temp = tempfile::tempdir().expect("temp dir");
        fs::write(
            temp.path().join("lib.lk"),
            r#"
fn value() {
    return 42;
}
"#,
        )
        .expect("write runtime module");
        let program = parse_program_source(
            r#"
use { value } from "lib";
return value();
"#,
            ParseOptions {
                base_dir: Some(temp.path().to_path_buf()),
                ..ParseOptions::default()
            },
        )
        .expect("runtime file item import should remain valid");
        assert_eq!(program.statements.len(), 2);
    }

    #[test]
    fn runtime_package_item_import_without_macro_is_not_rejected() {
        let temp = tempfile::tempdir().expect("temp dir");
        write_package(
            temp.path(),
            "util",
            r#"
fn value() {
    return 42;
}
"#,
        );
        let app_src = write_app_manifest(temp.path(), "util");
        let program = parse_program_source(
            r#"
use { value } from util;
return value();
"#,
            ParseOptions {
                base_dir: Some(app_src),
                ..ParseOptions::default()
            },
        )
        .expect("runtime package item import should remain valid");
        assert_eq!(program.statements.len(), 2);
    }

    #[test]
    fn local_crate_anchor_runtime_item_reference_uses_same_file_item() {
        let result = execute_source(
            r#"
fn helper() {
    return 40;
}

macro_rules! answer {
    () => { $crate::helper() + 2 };
}

return answer!();
"#,
        )
        .expect("local crate anchor runtime item should execute");

        assert_eq!(result.display_first_return(), "42");
    }

    #[test]
    fn imported_crate_anchor_runtime_item_reference_imports_definition_module() {
        let temp = tempfile::tempdir().expect("temp dir");
        fs::write(
            temp.path().join("macros.lk"),
            r#"
fn helper() {
    return 40;
}

export macro_rules! answer {
    () => { $crate::helper() + 2 };
}
"#,
        )
        .expect("write macro module");
        let program = parse_program_source(
            r#"
use { answer } from "macros";
return answer!();
"#,
            ParseOptions {
                base_dir: Some(temp.path().to_path_buf()),
                ..ParseOptions::default()
            },
        )
        .expect("program should parse with injected runtime anchor import");
        let mut resolver = ModuleResolver::new();
        resolver.set_base_dir(temp.path().to_path_buf());
        let mut ctx = VmContext::new().with_resolver(alloc::sync::Arc::new(resolver));
        let result = program
            .execute_with_ctx(&mut ctx)
            .expect("injected runtime anchor import should execute");

        assert_eq!(result.display_first_return(), "42");
    }

    #[test]
    fn package_crate_anchor_runtime_item_reference_imports_definition_module() {
        let temp = tempfile::tempdir().expect("temp dir");
        write_package(
            temp.path(),
            "util",
            r#"
fn helper() {
    return 40;
}

export macro_rules! answer {
    () => { $crate::helper() + 2 };
}
"#,
        );
        let app_src = write_app_manifest(temp.path(), "util");
        let program = parse_program_source(
            r#"
use { answer } from util;
return answer!();
"#,
            ParseOptions {
                base_dir: Some(app_src),
                ..ParseOptions::default()
            },
        )
        .expect("program should parse with injected package runtime anchor import");
        let resolver = ModuleResolver::new();
        resolver.register_package_module("util", temp.path().join("deps/util/src/mod.lk"));
        let mut ctx = VmContext::new().with_resolver(alloc::sync::Arc::new(resolver));
        let result = program
            .execute_with_ctx(&mut ctx)
            .expect("injected package runtime anchor import should execute");

        assert_eq!(result.display_first_return(), "42");
    }
}
