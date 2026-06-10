use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use lk_core::token::{Span, Token};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::json;
use tower_lsp::lsp_types::{Hover, HoverContents, Location, MarkupContent, MarkupKind, Position, Range, Url};

use super::analysis::{find_stdlib_export_location, find_stdlib_module_location};
use super::text::describe_token_hover;

static DECL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\s*(fn|struct|trait|type)\s+([A-Za-z_][A-Za-z0-9_-]*)").expect("valid declaration regex")
});
static TYPE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b[A-Z][A-Za-z0-9_]*\??\b").expect("valid type regex"));

const BUILTIN_TYPES: &[&str] = &[
    "Any", "Int", "Float", "String", "Bool", "Nil", "List", "Map", "Set", "Tuple", "Optional", "Task", "Channel",
    "Bytes", "Slice", "Object", "Number",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LkDeclKind {
    Function,
    Struct,
    Trait,
    Type,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LkDecl {
    pub(crate) name: String,
    pub(crate) kind: LkDeclKind,
    pub(crate) signature: String,
    pub(crate) doc: Option<String>,
    pub(crate) range: Range,
    pub(crate) name_range: Range,
    name_start_offset: usize,
    name_end_offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LkDocIndex {
    pub(crate) crate_doc: Option<String>,
    pub(crate) decls: Vec<LkDecl>,
}

pub(crate) fn document_hover(
    content: &str,
    uri: &Url,
    tokens: &[Token],
    spans: &[Span],
    idx: usize,
    package_modules: &HashMap<String, PathBuf>,
) -> Hover {
    let index = scan_lk_docs(content);
    if let Some(hover) = declaration_hover(content, uri, tokens, spans, idx, &index) {
        return hover;
    }
    if let Some(hover) = package_doc_hover(tokens, idx, package_modules) {
        return hover;
    }
    if let Some(hover) = stdlib_hover(content, uri, tokens, idx, &index) {
        return hover;
    }

    markdown_hover(describe_token_hover(tokens, spans, idx), None)
}

pub(crate) fn scan_lk_docs(content: &str) -> LkDocIndex {
    let mut scanner = SourceScanner::new(content);
    let crate_doc = scanner.take_crate_doc();
    let decls = scanner.scan_declarations();
    LkDocIndex { crate_doc, decls }
}

fn declaration_hover(
    content: &str,
    uri: &Url,
    tokens: &[Token],
    spans: &[Span],
    idx: usize,
    index: &LkDocIndex,
) -> Option<Hover> {
    let Token::Id(name) = tokens.get(idx)? else {
        return None;
    };

    let span = spans.get(idx)?;
    let direct_decl = index.decls.iter().find(|decl| {
        decl.name == *name && decl.name_start_offset <= span.start.offset && span.start.offset < decl.name_end_offset
    });
    let callable_decl = if matches!(tokens.get(idx + 1), Some(Token::LParen)) {
        index
            .decls
            .iter()
            .find(|decl| decl.name == *name && decl.kind == LkDeclKind::Function)
    } else {
        None
    };
    let decl = direct_decl.or(callable_decl)?;
    Some(markdown_hover(
        render_decl_markdown(decl, content, uri, index),
        Some(decl.name_range),
    ))
}

fn render_decl_markdown(decl: &LkDecl, content: &str, uri: &Url, index: &LkDocIndex) -> String {
    let mut out = String::new();
    out.push_str("```lk\n");
    out.push_str(&decl.signature);
    out.push_str("\n```");
    if let Some(doc) = &decl.doc {
        out.push_str("\n\n");
        out.push_str(doc.trim());
    }

    let links = type_links_for_signature(&decl.signature, content, uri, index);
    if !links.is_empty() {
        out.push_str("\n\n");
        out.push_str(&links.join(" | "));
    }
    out
}

fn type_links_for_signature(signature: &str, content: &str, uri: &Url, index: &LkDocIndex) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut links = Vec::new();
    for mat in TYPE_RE.find_iter(signature) {
        let type_name = mat.as_str().trim_end_matches('?');
        if !seen.insert(type_name.to_string()) {
            continue;
        }
        if let Some(decl) = index
            .decls
            .iter()
            .find(|decl| decl.name == type_name && decl.kind != LkDeclKind::Function)
        {
            links.push(format!(
                "[Go to {}]({})",
                type_name,
                command_uri(uri, decl.name_range, content)
            ));
        } else if BUILTIN_TYPES.contains(&type_name) {
            if let Some(location) = spec_type_location(type_name) {
                links.push(format!(
                    "[Go to {}]({})",
                    type_name,
                    command_uri_for_location(&location)
                ));
            }
        }
    }
    links
}

fn stdlib_hover(content: &str, uri: &Url, tokens: &[Token], idx: usize, index: &LkDocIndex) -> Option<Hover> {
    let path = dotted_token_path(tokens, idx)?;
    let catalog = lk_stdlib::stdlib_catalog();

    if path.len() == 1 {
        if let Some(global) = catalog.global(path[0]) {
            let signature = global.signature.clone().unwrap_or_else(|| global.detail.clone());
            let docs = global.docs.clone().unwrap_or_else(|| "LK stdlib global".to_string());
            return Some(markdown_hover(
                render_stdlib_markdown(path[0], &signature, &docs, None, content, uri, index),
                None,
            ));
        }
        if let Some(module) = catalog.module(path[0]) {
            let docs = module.docs.clone().unwrap_or_else(|| module.detail.clone());
            let implementation = find_stdlib_module_location(path[0]);
            return Some(markdown_hover(
                render_stdlib_module_markdown(path[0], &docs, implementation.as_ref()),
                None,
            ));
        }
        return None;
    }

    let export = catalog.export_path(&path)?;
    let signature = export.signature.clone().unwrap_or_else(|| export.detail.clone());
    let docs = export.docs.clone().unwrap_or_else(|| "LK stdlib export".to_string());
    let implementation = find_stdlib_export_location(path[0], path[path.len() - 1]);
    Some(markdown_hover(
        render_stdlib_markdown(
            &path.join("."),
            &signature,
            &docs,
            implementation.as_ref(),
            content,
            uri,
            index,
        ),
        None,
    ))
}

fn render_stdlib_module_markdown(path: &str, docs: &str, implementation: Option<&Location>) -> String {
    let mut out = format!("`{path}`\n\n{}", docs.trim());
    if let Some(location) = implementation {
        out.push_str("\n\n");
        out.push_str(&format!(
            "[Go to implementation]({})",
            command_uri_for_location(location)
        ));
    }
    out
}

fn render_stdlib_markdown(
    path: &str,
    signature: &str,
    docs: &str,
    implementation: Option<&Location>,
    content: &str,
    uri: &Url,
    index: &LkDocIndex,
) -> String {
    let mut out = format!("`{path}`\n\n```lk\n{signature}\n```");
    let docs = docs.trim();
    if !docs.is_empty() {
        out.push_str("\n\n");
        out.push_str(docs);
    }

    let mut links = type_links_for_signature(signature, content, uri, index);
    if let Some(location) = implementation {
        links.push(format!(
            "[Go to implementation]({})",
            command_uri_for_location(location)
        ));
    }
    if !links.is_empty() {
        out.push_str("\n\n");
        out.push_str(&links.join(" | "));
    }
    out
}

fn dotted_token_path(tokens: &[Token], idx: usize) -> Option<Vec<&str>> {
    let Token::Id(name) = tokens.get(idx)? else {
        return None;
    };
    let mut path = vec![name.as_str()];
    let mut cursor = idx;
    while cursor >= 2 {
        if !matches!(tokens.get(cursor - 1), Some(Token::Dot)) {
            break;
        }
        let Some(Token::Id(parent)) = tokens.get(cursor - 2) else {
            break;
        };
        path.push(parent.as_str());
        cursor -= 2;
    }
    path.reverse();
    Some(path)
}

fn package_doc_hover(tokens: &[Token], idx: usize, package_modules: &HashMap<String, PathBuf>) -> Option<Hover> {
    let Token::Id(name) = tokens.get(idx)? else {
        return None;
    };
    let package_name = import_package_name_for_token(tokens, idx)?;
    let path = package_modules.get(&package_name)?;
    let content = fs::read_to_string(path).ok()?;
    let index = scan_lk_docs(&content);
    let doc = index.crate_doc?;
    let title = if name == &package_name {
        format!("# `{package_name}`")
    } else {
        format!("# `{name}`\n\nAlias for `{package_name}`")
    };
    Some(markdown_hover(format!("{title}\n\n{}", doc.trim()), None))
}

fn import_package_name_for_token(tokens: &[Token], idx: usize) -> Option<String> {
    match tokens.get(idx) {
        Some(Token::Id(name)) if matches!(tokens.get(idx.wrapping_sub(1)), Some(Token::Use | Token::From)) => {
            Some(name.clone())
        }
        Some(Token::Id(_alias))
            if idx >= 3
                && matches!(tokens.get(idx - 1), Some(Token::As))
                && matches!(tokens.get(idx - 3), Some(Token::Use)) =>
        {
            match tokens.get(idx - 2) {
                Some(Token::Id(package_name)) => Some(package_name.clone()),
                _ => None,
            }
        }
        _ => None,
    }
}

fn markdown_hover(value: String, range: Option<Range>) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range,
    }
}

fn command_uri(uri: &Url, range: Range, content: &str) -> String {
    let start_offset = offset_for_position(content, range.start).unwrap_or(0);
    let end_offset = offset_for_position(content, range.end).unwrap_or(start_offset);
    let args = json!([{
        "uri": uri.as_str(),
        "range": {
            "start": {
                "line": range.start.line,
                "character": range.start.character,
                "offset": start_offset
            },
            "end": {
                "line": range.end.line,
                "character": range.end.character,
                "offset": end_offset
            }
        }
    }]);
    format!("command:lk.openLocation?{}", percent_encode(&args.to_string()))
}

fn command_uri_for_location(location: &Location) -> String {
    let content = location
        .uri
        .to_file_path()
        .ok()
        .and_then(|path| fs::read_to_string(path).ok())
        .unwrap_or_default();
    command_uri(&location.uri, location.range, &content)
}

fn spec_type_location(type_name: &str) -> Option<Location> {
    let path = repo_root_from_manifest()
        .join("website")
        .join("src")
        .join("spec")
        .join("LANG.md");
    let content = fs::read_to_string(&path).ok()?;
    let line = spec_type_line(&content, type_name)?;
    let uri = Url::from_file_path(path).ok()?;
    let line_text = content.lines().nth(line)?;
    let character = line_text.find(type_name).unwrap_or(0) as u32;
    Some(Location::new(
        uri,
        Range::new(
            Position::new(line as u32, character),
            Position::new(line as u32, character + type_name.chars().count() as u32),
        ),
    ))
}

fn spec_type_line(content: &str, type_name: &str) -> Option<usize> {
    let backticked = format!("`{type_name}`");
    let list_prefix = format!("- {type_name}:");
    content
        .lines()
        .position(|line| line.contains(&backticked) || line.trim_start().starts_with(&list_prefix))
        .or_else(|| spec_type_fallback_line(content, type_name))
        .or_else(|| content.lines().position(|line| line.contains(type_name)))
}

fn spec_type_fallback_line(content: &str, type_name: &str) -> Option<usize> {
    let needle = match type_name {
        "Number" => "Numeric auto-promotion",
        "Tuple" => "Tuple (comma-separated)",
        "Optional" => "Optional:",
        "Bytes" | "Slice" => "typeof(value)",
        _ => return None,
    };
    content.lines().position(|line| line.contains(needle))
}

fn repo_root_from_manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

struct SourceScanner<'a> {
    lines: Vec<LineInfo<'a>>,
}

#[derive(Debug, Clone)]
struct LineInfo<'a> {
    text: &'a str,
    start: usize,
    line: u32,
}

impl<'a> SourceScanner<'a> {
    fn new(content: &'a str) -> Self {
        let mut lines = Vec::new();
        let mut start = 0usize;
        for (line, segment) in content.split_inclusive('\n').enumerate() {
            let text = segment.strip_suffix('\n').unwrap_or(segment);
            lines.push(LineInfo {
                text,
                start,
                line: line as u32,
            });
            start += segment.chars().count();
        }
        if content.is_empty() || !content.ends_with('\n') {
            let line = lines.len() as u32;
            lines.push(LineInfo { text: "", start, line });
        }
        Self { lines }
    }

    fn take_crate_doc(&mut self) -> Option<String> {
        let mut docs = Vec::new();
        let mut idx = 0usize;
        while idx < self.lines.len() {
            let trimmed = self.lines[idx].text.trim_start();
            if trimmed.is_empty() {
                if docs.is_empty() {
                    idx += 1;
                    continue;
                }
                break;
            }
            if let Some(text) = trimmed.strip_prefix("//!") {
                docs.push(text.trim_start().to_string());
                idx += 1;
                continue;
            }
            if let Some(text) = trimmed.strip_prefix("/*!") {
                let (block, next_idx) = self.collect_block_doc(idx, text);
                docs.push(block);
                idx = next_idx;
                continue;
            }
            break;
        }
        join_doc_lines(docs)
    }

    fn scan_declarations(&self) -> Vec<LkDecl> {
        let mut decls = Vec::new();
        let mut pending_doc: Option<Vec<String>> = None;
        let mut last_doc_end_line: Option<u32> = None;
        let mut idx = 0usize;
        while idx < self.lines.len() {
            let line = &self.lines[idx];
            let trimmed = line.text.trim_start();
            if let Some(text) = trimmed.strip_prefix("///") {
                let mut docs = pending_doc.take().unwrap_or_default();
                docs.push(text.trim_start().to_string());
                pending_doc = Some(docs);
                last_doc_end_line = Some(line.line);
                idx += 1;
                continue;
            }
            if let Some(text) = trimmed.strip_prefix("/**") {
                let (block, next_idx) = self.collect_block_doc(idx, text);
                pending_doc = Some(vec![block]);
                last_doc_end_line = Some(self.lines[next_idx.saturating_sub(1)].line);
                idx = next_idx;
                continue;
            }
            if trimmed.is_empty() {
                idx += 1;
                continue;
            }
            if let Some(caps) = DECL_RE.captures(trimmed) {
                let doc = if last_doc_end_line.is_some_and(|doc_line| doc_line + 1 == line.line) {
                    pending_doc.take().and_then(join_doc_lines)
                } else {
                    pending_doc = None;
                    None
                };
                if let Some(decl) = self.build_decl(idx, line, &caps, doc) {
                    decls.push(decl);
                }
            } else {
                pending_doc = None;
            }
            idx += 1;
        }
        decls
    }

    fn build_decl(
        &self,
        line_idx: usize,
        line: &LineInfo<'_>,
        caps: &regex::Captures<'_>,
        doc: Option<String>,
    ) -> Option<LkDecl> {
        let kind_text = caps.get(1)?.as_str();
        let name_match = caps.get(2)?;
        let kind = match kind_text {
            "fn" => LkDeclKind::Function,
            "struct" => LkDeclKind::Struct,
            "trait" => LkDeclKind::Trait,
            "type" => LkDeclKind::Type,
            _ => return None,
        };
        let name = name_match.as_str().to_string();
        let signature = self.collect_signature(line_idx, kind);
        let start = Position::new(line.line, 0);
        let end = signature_end_position(line.line, &signature);
        let leading_chars = line.text.chars().count() - line.text.trim_start().chars().count();
        let name_start = leading_chars + name_match.start();
        let name_end = leading_chars + name_match.end();
        let name_start_offset = line.start + name_start;
        let name_end_offset = line.start + name_end;
        Some(LkDecl {
            name,
            kind,
            signature,
            doc,
            range: Range::new(start, end),
            name_range: Range::new(
                Position::new(line.line, name_start as u32),
                Position::new(line.line, name_end as u32),
            ),
            name_start_offset,
            name_end_offset,
        })
    }

    fn collect_signature(&self, line_idx: usize, kind: LkDeclKind) -> String {
        let mut signature = String::new();
        let mut brace_depth = 0i32;
        for line in self.lines.iter().skip(line_idx) {
            let visible = if kind == LkDeclKind::Function {
                strip_trailing_body(line.text)
            } else {
                line.text
            };
            if !signature.is_empty() {
                signature.push('\n');
            }
            signature.push_str(visible.trim_end());
            for ch in visible.chars() {
                match ch {
                    '{' => brace_depth += 1,
                    '}' => brace_depth -= 1,
                    _ => {}
                }
            }
            match kind {
                LkDeclKind::Function if visible.contains('{') || visible.trim_end().ends_with(';') => break,
                LkDeclKind::Struct | LkDeclKind::Trait | LkDeclKind::Type
                    if brace_depth <= 0 && visible.contains('}') =>
                {
                    break
                }
                LkDeclKind::Type if visible.trim_end().ends_with(';') => break,
                _ => {}
            }
        }
        signature.trim().to_string()
    }

    fn collect_block_doc(&self, start_idx: usize, first_after_marker: &str) -> (String, usize) {
        let mut docs = Vec::new();
        let mut idx = start_idx;
        let mut rest = first_after_marker;
        loop {
            if let Some(end) = rest.find("*/") {
                docs.push(clean_block_doc_line(&rest[..end]));
                return (docs.join("\n").trim().to_string(), idx + 1);
            }
            docs.push(clean_block_doc_line(rest));
            idx += 1;
            let Some(next_line) = self.lines.get(idx) else {
                return (docs.join("\n").trim().to_string(), idx);
            };
            rest = next_line.text.trim_start();
        }
    }
}

fn strip_trailing_body(line: &str) -> &str {
    if let Some(pos) = line.find('{') {
        &line[..=pos]
    } else {
        line
    }
}

fn signature_end_position(start_line: u32, signature: &str) -> Position {
    let mut line = start_line;
    let mut character = 0u32;
    for ch in signature.chars() {
        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += 1;
        }
    }
    Position::new(line, character)
}

fn clean_block_doc_line(line: &str) -> String {
    line.trim_start().trim_start_matches('*').trim_start().to_string()
}

fn join_doc_lines(lines: Vec<String>) -> Option<String> {
    let doc = lines.join("\n").trim().to_string();
    (!doc.is_empty()).then_some(doc)
}

fn offset_for_position(content: &str, position: Position) -> Option<usize> {
    let mut line = 0u32;
    let mut character = 0u32;
    for (offset, ch) in content.chars().enumerate() {
        if line == position.line && character == position.character {
            return Some(offset);
        }
        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += 1;
        }
    }
    if line == position.line && character == position.character {
        return Some(content.chars().count());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_markdown_function_docs_and_ignores_plain_comments() {
        let index = scan_lk_docs(
            r#"// ordinary comment
/// **Runs** a user query.
fn should_run(name: String) -> Bool {
  return name == "a";
}
"#,
        );

        assert_eq!(index.decls.len(), 1);
        assert_eq!(index.decls[0].name, "should_run");
        assert_eq!(index.decls[0].doc.as_deref(), Some("**Runs** a user query."));
        assert!(index.decls[0]
            .signature
            .contains("fn should_run(name: String) -> Bool {"));
    }

    #[test]
    fn scans_block_docs_and_crate_docs() {
        let index = scan_lk_docs(
            r#"//! Package docs
//! with markdown.

/**
 * User model.
 */
struct User { id: Int, name: String }
"#,
        );

        assert_eq!(index.crate_doc.as_deref(), Some("Package docs\nwith markdown."));
        assert_eq!(index.decls[0].doc.as_deref(), Some("User model."));
        assert_eq!(index.decls[0].signature, "struct User { id: Int, name: String }");
    }

    #[test]
    fn links_local_and_builtin_types() {
        let uri = Url::parse("file:///tmp/test.lk").expect("uri");
        let content = "struct User { id: Int }\n/// Loads user\nfn load(id: Int) -> User {\n}\n";
        let index = scan_lk_docs(content);
        let fn_decl = index.decls.iter().find(|decl| decl.name == "load").expect("load decl");
        let rendered = render_decl_markdown(fn_decl, content, &uri, &index);

        assert!(rendered.contains("[Go to User](command:lk.openLocation?"));
        assert!(rendered.contains("[Go to Int](command:lk.openLocation?"));
    }

    #[test]
    fn stdlib_function_hover_renders_markdown_signature_docs_and_links() {
        let uri = Url::parse("file:///tmp/test.lk").expect("uri");
        let content = "math.floor(1.2)";
        let index = scan_lk_docs(content);
        let tokens = vec![
            Token::Id("math".to_string()),
            Token::Dot,
            Token::Id("floor".to_string()),
            Token::LParen,
        ];

        let hover = stdlib_hover(content, &uri, &tokens, 2, &index).expect("math.floor hover");
        let HoverContents::Markup(markup) = hover.contents else {
            panic!("expected markdown hover");
        };

        assert_eq!(markup.kind, MarkupKind::Markdown);
        assert!(markup.value.contains("`math.floor`"));
        assert!(markup.value.contains("```lk\nmath.floor(value: Number) -> Int\n```"));
        assert!(markup.value.contains("Go to Number"));
        assert!(markup.value.contains("Go to Int"));
        assert!(markup.value.contains("Go to implementation"));
    }

    #[test]
    fn stdlib_hover_preserves_markdown_docs() {
        let uri = Url::parse("file:///tmp/test.lk").expect("uri");
        let content = "env.get(\"HOME\")";
        let index = scan_lk_docs(content);
        let tokens = vec![
            Token::Id("env".to_string()),
            Token::Dot,
            Token::Id("get".to_string()),
            Token::LParen,
        ];

        let hover = stdlib_hover(content, &uri, &tokens, 2, &index).expect("env.get hover");
        let HoverContents::Markup(markup) = hover.contents else {
            panic!("expected markdown hover");
        };

        assert!(markup.value.contains("Returns an environment variable"));
    }

    #[test]
    fn command_uri_for_external_location_includes_target_file() {
        let path = repo_root_from_manifest()
            .join("website")
            .join("src")
            .join("spec")
            .join("LANG.md");
        let uri = Url::from_file_path(&path).expect("spec uri");
        let location = Location::new(uri, Range::new(Position::new(13, 3), Position::new(13, 9)));
        let command = command_uri_for_location(&location);

        assert!(command.contains("command:lk.openLocation?"));
        assert!(command.contains("website%2Fsrc%2Fspec%2FLANG.md"));
    }

    #[test]
    fn detects_package_alias_hover_target() {
        let tokens = vec![
            Token::Use,
            Token::Id("mathlib".to_string()),
            Token::As,
            Token::Id("ml".to_string()),
        ];
        assert_eq!(import_package_name_for_token(&tokens, 1).as_deref(), Some("mathlib"));
        assert_eq!(import_package_name_for_token(&tokens, 3).as_deref(), Some("mathlib"));
    }
}
