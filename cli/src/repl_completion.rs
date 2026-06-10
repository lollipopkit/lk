use lk_completion::{CompletionCandidate, CompletionEngine, CompletionMode, CompletionRequest};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub(crate) struct ReplCompletionState {
    source: Arc<Mutex<String>>,
}

impl ReplCompletionState {
    pub(crate) fn new() -> Self {
        Self {
            source: Arc::new(Mutex::new(String::new())),
        }
    }

    pub(crate) fn append_successful_input(&self, input: &str) {
        let Ok(mut source) = self.source.lock() else {
            return;
        };
        source.push_str(input);
        source.push('\n');
    }

    fn snapshot(&self) -> Option<String> {
        self.source.lock().ok().map(|source| source.clone())
    }
}

pub(crate) struct ReplCompletion {
    engine: CompletionEngine,
    state: ReplCompletionState,
    base_dir: Option<PathBuf>,
}

impl ReplCompletion {
    pub(crate) fn new(state: ReplCompletionState) -> anyhow::Result<Self> {
        Ok(Self {
            engine: CompletionEngine::new()?,
            state,
            base_dir: std::env::current_dir().ok(),
        })
    }

    pub(crate) fn complete(&self, source: &str, cursor: usize) -> Vec<CompletionCandidate> {
        let cursor = cursor.min(source.len());
        if !should_complete(source, cursor) {
            return Vec::new();
        }
        let session_source = self.state.snapshot();
        self.engine.complete(CompletionRequest {
            source,
            cursor,
            mode: CompletionMode::Repl,
            trigger: lk_completion::CompletionTrigger::Invoked,
            session_source: session_source.as_deref(),
            base_dir: self.base_dir.as_deref(),
        })
    }
}

fn should_complete(source: &str, cursor: usize) -> bool {
    if source[..cursor].trim().is_empty() {
        return false;
    }
    let line_start = source[..cursor].rfind('\n').map_or(0, |idx| idx + 1);
    let line_prefix = &source[line_start..cursor];
    let trimmed = line_prefix.trim_start();
    if trimmed.starts_with(':') {
        return trimmed.len() > 1;
    }
    if import_path_completion_active(line_prefix) {
        return true;
    }
    if line_prefix.ends_with('.') {
        return true;
    }
    if line_prefix.ends_with("use ") || line_prefix.ends_with("from ") {
        return true;
    }
    current_identifier_prefix(line_prefix).is_some_and(|prefix| !prefix.is_empty())
}

fn import_path_completion_active(line_prefix: &str) -> bool {
    let Some(start_quote) = line_prefix.rfind("use \"") else {
        return false;
    };
    !line_prefix[start_quote + "use \"".len()..].contains('"')
}

fn current_identifier_prefix(line_prefix: &str) -> Option<&str> {
    let start = line_prefix
        .char_indices()
        .rev()
        .find_map(|(idx, ch)| (!is_ident_continue(ch)).then_some(idx + ch.len_utf8()))
        .unwrap_or(0);
    Some(&line_prefix[start..])
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch == '-' || ch.is_ascii_alphanumeric()
}

pub(crate) fn ghost_suffix(source: &str, cursor: usize, candidate: Option<&CompletionCandidate>) -> String {
    let Some(candidate) = candidate else {
        return String::new();
    };
    if candidate.replace_end != cursor || candidate.replace_start > cursor || cursor > source.len() {
        return String::new();
    }
    let typed = &source[candidate.replace_start..cursor];
    candidate
        .replacement
        .strip_prefix(typed)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
fn apply_candidate(source: &str, candidate: &CompletionCandidate) -> (String, usize) {
    let start = candidate.replace_start.min(source.len());
    let end = candidate.replace_end.min(source.len()).max(start);
    let mut out = String::with_capacity(source.len() + candidate.replacement.len());
    out.push_str(&source[..start]);
    out.push_str(&candidate.replacement);
    out.push_str(&source[end..]);
    let cursor = start + candidate.replacement.len();
    (out, cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repl_completes_commands() {
        let state = ReplCompletionState::new();
        let completion = ReplCompletion::new(state).unwrap();
        let candidates = completion.complete(":h", 2);
        assert!(candidates.iter().any(|candidate| candidate.replacement == ":help"));
    }

    #[test]
    fn repl_completes_persistent_session_symbols() {
        let state = ReplCompletionState::new();
        state.append_successful_input("let user_name = 1;");
        state.append_successful_input("fn user_score() { return 1; }");
        let completion = ReplCompletion::new(state).unwrap();
        let candidates = completion.complete("use", 3);
        assert!(candidates.iter().any(|candidate| candidate.replacement == "user_name"));
        assert!(candidates.iter().any(|candidate| candidate.replacement == "user_score"));
    }

    #[test]
    fn repl_completes_persistent_session_types() {
        let state = ReplCompletionState::new();
        state.append_successful_input("trait Drawable {}");
        state.append_successful_input("struct Point {}");
        state.append_successful_input("type UserId = Int;");
        let completion = ReplCompletion::new(state).unwrap();

        let drawable = completion.complete("Dra", 3);
        assert!(drawable.iter().any(|candidate| candidate.replacement == "Drawable"));

        let point = completion.complete("Poi", 3);
        assert!(point.iter().any(|candidate| candidate.replacement == "Point"));

        let user_id = completion.complete("User", 4);
        assert!(user_id.iter().any(|candidate| candidate.replacement == "UserId"));
    }

    #[test]
    fn repl_does_not_complete_empty_input() {
        let state = ReplCompletionState::new();
        let completion = ReplCompletion::new(state).unwrap();
        assert!(completion.complete("", 0).is_empty());
        assert!(completion.complete("   ", 3).is_empty());
    }

    #[test]
    fn repl_does_not_complete_after_closed_expression() {
        let state = ReplCompletionState::new();
        let completion = ReplCompletion::new(state).unwrap();
        assert!(completion.complete("print(1)", "print(1)".len()).is_empty());
        assert!(completion.complete("print(1) ", "print(1) ".len()).is_empty());
    }

    #[test]
    fn ghost_suffix_uses_first_candidate_remainder() {
        let candidate = CompletionCandidate {
            label: "println".to_string(),
            replacement: "println".to_string(),
            detail: None,
            kind: lk_completion::CompletionKind::Function,
            replace_start: 0,
            replace_end: 3,
        };
        assert_eq!(ghost_suffix("pri", 3, Some(&candidate)), "ntln");
    }

    #[test]
    fn apply_candidate_replaces_engine_range() {
        let candidate = CompletionCandidate {
            label: "println".to_string(),
            replacement: "println".to_string(),
            detail: None,
            kind: lk_completion::CompletionKind::Function,
            replace_start: 4,
            replace_end: 7,
        };
        assert_eq!(apply_candidate("let pri", &candidate), ("let println".to_string(), 11));
    }
}
