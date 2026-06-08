use lk_completion::{CompletionEngine, CompletionMode, CompletionRequest};
use rustyline::{
    Context, Helper, Result,
    completion::{Completer, Pair},
    highlight::Highlighter,
    hint::Hinter,
    validate::Validator,
};
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
}

pub(crate) struct ReplHelper {
    engine: CompletionEngine,
    state: ReplCompletionState,
    base_dir: Option<PathBuf>,
}

impl ReplHelper {
    pub(crate) fn new(state: ReplCompletionState) -> anyhow::Result<Self> {
        Ok(Self {
            engine: CompletionEngine::new()?,
            state,
            base_dir: std::env::current_dir().ok(),
        })
    }
}

impl Helper for ReplHelper {}
impl Highlighter for ReplHelper {}
impl Validator for ReplHelper {}

impl Hinter for ReplHelper {
    type Hint = String;
}

impl Completer for ReplHelper {
    type Candidate = Pair;

    fn complete(&self, line: &str, pos: usize, _ctx: &Context<'_>) -> Result<(usize, Vec<Self::Candidate>)> {
        let session_guard = self.state.source.lock().ok();
        let session_source = session_guard.as_ref().map(|source| source.as_str());
        let candidates = self.engine.complete(CompletionRequest {
            source: line,
            cursor: pos.min(line.len()),
            mode: CompletionMode::Repl,
            session_source,
            base_dir: self.base_dir.as_deref(),
        });
        let start = candidates.first().map(|item| item.replace_start).unwrap_or(pos);
        let pairs = candidates
            .into_iter()
            .map(|item| Pair {
                display: item
                    .detail
                    .as_ref()
                    .map(|detail| format!("{}\t{}", item.label, detail))
                    .unwrap_or_else(|| item.label.clone()),
                replacement: item.replacement,
            })
            .collect();
        Ok((start, pairs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustyline::completion::Completer;
    use rustyline::history::DefaultHistory;

    fn test_context(history: &DefaultHistory) -> Context<'_> {
        Context::new(history)
    }

    #[test]
    fn repl_completes_commands() {
        let state = ReplCompletionState::new();
        let helper = ReplHelper::new(state).unwrap();
        let history = DefaultHistory::new();
        let (_start, pairs) = helper.complete(":h", 2, &test_context(&history)).unwrap();
        assert!(pairs.iter().any(|pair| pair.replacement == ":help"));
    }

    #[test]
    fn repl_completes_persistent_session_symbols() {
        let state = ReplCompletionState::new();
        state.append_successful_input("let user_name = 1;");
        let helper = ReplHelper::new(state).unwrap();
        let history = DefaultHistory::new();
        let (_start, pairs) = helper.complete("use", 3, &test_context(&history)).unwrap();
        assert!(pairs.iter().any(|pair| pair.replacement == "user_name"));
    }
}
