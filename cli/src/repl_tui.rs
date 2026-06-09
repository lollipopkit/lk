use std::borrow::Cow;

use nu_ansi_term::{Color as AnsiColor, Style};
use reedline::{
    Color as ReedlineColor, ColumnarMenu, Completer, Emacs, Hinter, History, KeyCode, KeyModifiers, MenuBuilder,
    Prompt, PromptEditMode, PromptHistorySearch, Reedline, ReedlineEvent, ReedlineMenu, Signal, Span, Suggestion,
    Validator,
};

use crate::{
    repl::{ReplInput, should_continue_multiline},
    repl_completion::{ReplCompletion, ReplCompletionState, ghost_suffix},
};

const COMPLETION_MENU: &str = "completion_menu";

fn prompt_style() -> Style {
    Style::new().fg(prompt_ansi_color())
}

fn prompt_ansi_color() -> AnsiColor {
    if terminal_supports_xterm256() {
        AnsiColor::Fixed(169)
    } else {
        AnsiColor::Cyan
    }
}

fn prompt_reedline_color() -> ReedlineColor {
    if terminal_supports_xterm256() {
        ReedlineColor::AnsiValue(169)
    } else {
        ReedlineColor::Cyan
    }
}

fn terminal_supports_xterm256() -> bool {
    supports_xterm256_color(
        std::env::var("TERM").ok().as_deref(),
        std::env::var("COLORTERM").ok().as_deref(),
    )
}

fn supports_xterm256_color(term: Option<&str>, colorterm: Option<&str>) -> bool {
    term.is_some_and(|value| value.contains("256color"))
        || colorterm.is_some_and(|value| matches!(value, "truecolor" | "24bit"))
}

pub(crate) fn new_editor(state: ReplCompletionState) -> anyhow::Result<Reedline> {
    let completer = ReedlineReplCompleter::new(state.clone())?;
    let hinter = ReplHinter::new(state.clone())?;
    let validator = ReplValidator;
    let completion_menu = Box::new(ColumnarMenu::default().with_name(COMPLETION_MENU));
    let edit_mode = Box::new(Emacs::new(repl_keybindings()));

    Ok(Reedline::create()
        .with_completer(Box::new(completer))
        .with_hinter(Box::new(hinter))
        .with_validator(Box::new(validator))
        .with_menu(ReedlineMenu::EngineCompleter(completion_menu))
        .with_partial_completions(true)
        .with_edit_mode(edit_mode))
}

pub(crate) fn read_input(editor: &mut Reedline) -> anyhow::Result<ReplInput> {
    match editor.read_line(&prompt())? {
        Signal::Success(source) => Ok(ReplInput::Submit(source.trim_end().to_string())),
        Signal::CtrlC => Ok(ReplInput::Continue),
        Signal::CtrlD => Ok(ReplInput::Exit),
        _ => Ok(ReplInput::Continue),
    }
}

fn prompt() -> LkPrompt {
    LkPrompt
}

struct LkPrompt;

impl Prompt for LkPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Owned(prompt_style().paint("> ").to_string())
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_indicator(&self, _prompt_mode: PromptEditMode) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Owned(prompt_style().paint("... ").to_string())
    }

    fn render_prompt_history_search_indicator(&self, history_search: PromptHistorySearch) -> Cow<'_, str> {
        Cow::Owned(format!("(reverse-search: {}) ", history_search.term))
    }

    fn get_prompt_color(&self) -> ReedlineColor {
        prompt_reedline_color()
    }

    fn get_prompt_multiline_color(&self) -> AnsiColor {
        prompt_ansi_color()
    }

    fn get_indicator_color(&self) -> ReedlineColor {
        prompt_reedline_color()
    }
}

fn repl_keybindings() -> reedline::Keybindings {
    let mut keybindings = reedline::default_emacs_keybindings();
    keybindings.add_binding(
        KeyModifiers::NONE,
        KeyCode::Tab,
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu(COMPLETION_MENU.to_string()),
            ReedlineEvent::MenuNext,
        ]),
    );
    keybindings
}

struct ReedlineReplCompleter {
    completion: ReplCompletion,
}

impl ReedlineReplCompleter {
    fn new(state: ReplCompletionState) -> anyhow::Result<Self> {
        Ok(Self {
            completion: ReplCompletion::new(state)?,
        })
    }
}

impl Completer for ReedlineReplCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        self.completion
            .complete(line, pos)
            .into_iter()
            .map(|candidate| Suggestion {
                value: candidate.replacement,
                display_override: Some(candidate.label),
                description: candidate.detail,
                extra: None,
                span: Span::new(candidate.replace_start, candidate.replace_end),
                append_whitespace: false,
                style: None,
                match_indices: None,
            })
            .collect()
    }
}

struct ReplHinter {
    completion: ReplCompletion,
    current_hint: String,
    style: nu_ansi_term::Style,
}

impl ReplHinter {
    fn new(state: ReplCompletionState) -> anyhow::Result<Self> {
        Ok(Self {
            completion: ReplCompletion::new(state)?,
            current_hint: String::new(),
            style: nu_ansi_term::Style::new().fg(nu_ansi_term::Color::LightGray),
        })
    }
}

impl Hinter for ReplHinter {
    fn handle(
        &mut self,
        line: &str,
        pos: usize,
        _history: &dyn History,
        use_ansi_coloring: bool,
        _cwd: &str,
    ) -> String {
        let candidates = self.completion.complete(line, pos);
        self.current_hint = ghost_suffix(line, pos, candidates.first());
        if use_ansi_coloring && !self.current_hint.is_empty() {
            self.style.paint(&self.current_hint).to_string()
        } else {
            self.current_hint.clone()
        }
    }

    fn complete_hint(&self) -> String {
        self.current_hint.clone()
    }

    fn next_hint_token(&self) -> String {
        first_hint_token(&self.current_hint)
    }
}

fn first_hint_token(hint: &str) -> String {
    hint.split_whitespace().next().unwrap_or_default().to_string()
}

struct ReplValidator;

impl Validator for ReplValidator {
    fn validate(&self, line: &str) -> reedline::ValidationResult {
        if should_continue_multiline(line) {
            reedline::ValidationResult::Incomplete
        } else {
            reedline::ValidationResult::Complete
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repl_completion::ReplCompletionState;
    use reedline::ValidationResult;

    #[test]
    fn completer_skips_empty_input() {
        let state = ReplCompletionState::new();
        let mut completer = ReedlineReplCompleter::new(state).unwrap();
        assert!(completer.complete("", 0).is_empty());
        assert!(completer.complete("   ", 3).is_empty());
    }

    #[test]
    fn completer_returns_print_candidates_with_replacement_span() {
        let state = ReplCompletionState::new();
        let mut completer = ReedlineReplCompleter::new(state).unwrap();
        let suggestions = completer.complete("pri", 3);

        let print = suggestions
            .iter()
            .find(|suggestion| suggestion.value == "print")
            .expect("print should be suggested");
        assert_eq!(print.span, Span::new(0, 3));
        assert!(suggestions.iter().any(|suggestion| suggestion.value == "println"));
    }

    #[test]
    fn hinter_returns_first_candidate_suffix() {
        let state = ReplCompletionState::new();
        let mut hinter = ReplHinter::new(state).unwrap();
        let history = reedline::FileBackedHistory::default();
        assert_eq!(hinter.handle("pri", 3, &history, false, ""), "nt");
        assert_eq!(hinter.complete_hint(), "nt");
    }

    #[test]
    fn validator_keeps_incomplete_input_open() {
        let validator = ReplValidator;
        assert!(matches!(
            validator.validate("println((1)"),
            ValidationResult::Incomplete
        ));
        assert!(matches!(validator.validate("println(1)"), ValidationResult::Complete));
    }

    #[test]
    fn prompt_color_supports_xterm256_detection() {
        assert!(supports_xterm256_color(Some("xterm-256color"), None));
        assert!(supports_xterm256_color(None, Some("truecolor")));
        assert!(supports_xterm256_color(None, Some("24bit")));
        assert!(!supports_xterm256_color(Some("xterm"), None));
    }
}
