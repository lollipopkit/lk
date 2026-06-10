use lk_completion::{CompletionCandidate, CompletionKind, CompletionMode, CompletionRequest, CompletionTrigger};
use ropey::Rope;
use tower_lsp::lsp_types::{
    CompletionContext as LspCompletionContext, CompletionItem, CompletionItemKind, CompletionList, CompletionResponse,
    CompletionTextEdit, CompletionTriggerKind, Position, Range, TextEdit, Url,
};

use super::{state::LkLanguageServer, text::position_to_char_idx};

impl LkLanguageServer {
    pub(crate) fn completion_response(
        &self,
        uri: &Url,
        position: Position,
        context: Option<&LspCompletionContext>,
    ) -> Option<CompletionResponse> {
        let doc = self.documents.get(uri)?;
        let content = doc.content.to_string();
        let cursor_char = position_to_char_idx(&doc.content, position);
        let base_dir = uri.to_file_path().ok().and_then(|mut path| path.pop().then_some(path));
        Some(completion_response_for_source(
            &self.completion_engine,
            &content,
            cursor_char,
            completion_trigger_from_lsp(context),
            base_dir.as_deref(),
        ))
    }
}

pub(crate) fn completion_response_for_source(
    engine: &lk_completion::CompletionEngine,
    content: &str,
    cursor_char: usize,
    trigger: CompletionTrigger,
    base_dir: Option<&std::path::Path>,
) -> CompletionResponse {
    let cursor = char_to_byte_idx(content, cursor_char);
    let result = engine.complete_with_metadata(CompletionRequest {
        source: content,
        cursor,
        mode: CompletionMode::Lsp,
        trigger,
        session_source: None,
        base_dir,
    });
    let items = result
        .candidates
        .into_iter()
        .map(|candidate| completion_item(content, candidate))
        .collect();
    if result.is_incomplete {
        CompletionResponse::List(CompletionList {
            is_incomplete: true,
            items,
        })
    } else {
        CompletionResponse::Array(items)
    }
}

#[cfg(test)]
pub(crate) fn completion_items_for_source(
    engine: &lk_completion::CompletionEngine,
    content: &str,
    cursor_char: usize,
    trigger: CompletionTrigger,
    base_dir: Option<&std::path::Path>,
) -> Vec<CompletionItem> {
    completion_items_from_response(completion_response_for_source(
        engine,
        content,
        cursor_char,
        trigger,
        base_dir,
    ))
}

#[cfg(test)]
fn completion_items_from_response(response: CompletionResponse) -> Vec<CompletionItem> {
    match response {
        CompletionResponse::Array(items) => items,
        CompletionResponse::List(list) => list.items,
    }
}

fn completion_trigger_from_lsp(context: Option<&LspCompletionContext>) -> CompletionTrigger {
    let Some(context) = context else {
        return CompletionTrigger::Invoked;
    };
    if context.trigger_kind == CompletionTriggerKind::TRIGGER_CHARACTER {
        return context
            .trigger_character
            .as_deref()
            .and_then(|value| value.chars().next())
            .map(CompletionTrigger::TriggerCharacter)
            .unwrap_or(CompletionTrigger::Invoked);
    }
    if context.trigger_kind == CompletionTriggerKind::TRIGGER_FOR_INCOMPLETE_COMPLETIONS {
        CompletionTrigger::Incomplete
    } else {
        CompletionTrigger::Invoked
    }
}

#[cfg(test)]
fn completion_items_for_source_invoked(
    engine: &lk_completion::CompletionEngine,
    content: &str,
    cursor_char: usize,
    base_dir: Option<&std::path::Path>,
) -> Vec<CompletionItem> {
    completion_items_for_source(engine, content, cursor_char, CompletionTrigger::Invoked, base_dir)
}

fn completion_item(content: &str, candidate: CompletionCandidate) -> CompletionItem {
    let range = Range::new(
        byte_to_position(content, candidate.replace_start),
        byte_to_position(content, candidate.replace_end),
    );
    CompletionItem {
        label: candidate.label,
        kind: Some(completion_item_kind(candidate.kind)),
        detail: candidate.detail,
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range,
            new_text: candidate.replacement,
        })),
        ..Default::default()
    }
}

fn completion_item_kind(kind: CompletionKind) -> CompletionItemKind {
    match kind {
        CompletionKind::Keyword => CompletionItemKind::KEYWORD,
        CompletionKind::Operator => CompletionItemKind::OPERATOR,
        CompletionKind::Type => CompletionItemKind::TYPE_PARAMETER,
        CompletionKind::Function => CompletionItemKind::FUNCTION,
        CompletionKind::Module => CompletionItemKind::MODULE,
        CompletionKind::Method => CompletionItemKind::METHOD,
        CompletionKind::Field => CompletionItemKind::FIELD,
        CompletionKind::Variable => CompletionItemKind::VARIABLE,
        CompletionKind::Value => CompletionItemKind::VALUE,
        CompletionKind::File => CompletionItemKind::FILE,
        CompletionKind::Folder => CompletionItemKind::FOLDER,
        CompletionKind::Command => CompletionItemKind::TEXT,
    }
}

fn char_to_byte_idx(content: &str, char_idx: usize) -> usize {
    content
        .char_indices()
        .nth(char_idx)
        .map(|(idx, _)| idx)
        .unwrap_or(content.len())
}

fn byte_to_position(content: &str, byte_idx: usize) -> Position {
    let byte_idx = byte_idx.min(content.len());
    let prefix = &content[..byte_idx];
    let line = prefix.as_bytes().iter().filter(|byte| **byte == b'\n').count() as u32;
    let line_start = prefix.rfind('\n').map_or(0, |idx| idx + 1);
    let character = Rope::from_str(&content[line_start..byte_idx]).len_utf16_cu() as u32;
    Position::new(line, character)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(items: Vec<CompletionItem>) -> Vec<String> {
        items.into_iter().map(|item| item.label).collect()
    }

    #[test]
    fn lsp_completion_maps_nested_stdlib_exports() {
        let engine = lk_completion::CompletionEngine::new().unwrap();
        let content = "io.file.read";
        let got = labels(completion_items_for_source_invoked(
            &engine,
            content,
            content.chars().count(),
            None,
        ));
        assert!(got.contains(&"read_to_string".to_string()));
    }

    #[test]
    fn lsp_completion_maps_named_arg_text_edit() {
        let engine = lk_completion::CompletionEngine::new().unwrap();
        let content = "fn draw({width: Int}) { }\ndraw(w";
        let items = completion_items_for_source_invoked(&engine, content, content.chars().count(), None);
        let width = items
            .iter()
            .find(|item| item.label == "width:")
            .expect("width completion");
        let Some(CompletionTextEdit::Edit(edit)) = &width.text_edit else {
            panic!("expected text edit");
        };
        assert_eq!(edit.new_text, "width: ");
    }

    #[test]
    fn lsp_completion_maps_string_argument_values() {
        let engine = lk_completion::CompletionEngine::new().unwrap();
        let content = "if should_run(\"gcd_batch\") {}\nif should_run(\"\") {}";
        let cursor = content.rfind("\"\"").unwrap() + 1;
        let items =
            completion_items_for_source(&engine, content, cursor, CompletionTrigger::TriggerCharacter('"'), None);
        let item = items
            .iter()
            .find(|item| item.label == "gcd_batch")
            .expect("workload completion");
        assert_eq!(item.kind, Some(CompletionItemKind::VALUE));
        let Some(CompletionTextEdit::Edit(edit)) = &item.text_edit else {
            panic!("expected text edit");
        };
        assert_eq!(edit.new_text, "gcd_batch");
    }

    #[test]
    fn lsp_completion_maps_normal_identifier_function_after_block() {
        let engine = lk_completion::CompletionEngine::new().unwrap();
        let content = "fn should_run(name) { return true; }\nif should_run(\"\") {\nsho";
        let items = completion_items_for_source(
            &engine,
            content,
            content.chars().count(),
            CompletionTrigger::Incomplete,
            None,
        );
        assert!(items.iter().any(|item| item.label == "should_run"));
    }

    #[test]
    fn lsp_completion_suppresses_empty_prefix_on_brace_trigger() {
        let engine = lk_completion::CompletionEngine::new().unwrap();
        let content = "let a0 = 1;\nif should_run(\"\") {";
        let response = completion_response_for_source(
            &engine,
            content,
            content.chars().count(),
            CompletionTrigger::TriggerCharacter('{'),
            None,
        );
        let CompletionResponse::List(list) = response else {
            panic!("expected incomplete completion list");
        };
        assert!(list.items.is_empty());
        assert!(list.is_incomplete);
    }
}
