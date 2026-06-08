use std::fmt;
use std::io::IsTerminal;

use lk_core::token::ParseError;

pub(crate) fn error(message: impl fmt::Display) {
    eprintln!("{} {}", error_label(), message);
}

pub(crate) fn warning(message: impl fmt::Display) {
    eprintln!("{} {}", warning_label(), message);
}

pub(crate) fn parse_error(error: &ParseError, source: &str) {
    eprintln!(
        "{} {}",
        error_label(),
        error.display_with_source_color(source, color_enabled())
    );
}

fn color_enabled() -> bool {
    match std::env::var("LK_COLOR") {
        Ok(value) if value.eq_ignore_ascii_case("always") => true,
        Ok(value) if value.eq_ignore_ascii_case("never") => false,
        Ok(value) if value.eq_ignore_ascii_case("auto") => std::io::stderr().is_terminal(),
        _ if std::env::var_os("NO_COLOR").is_some() => false,
        _ => std::io::stderr().is_terminal(),
    }
}

fn error_label() -> &'static str {
    if color_enabled() {
        "\x1b[31;1mError:\x1b[0m"
    } else {
        "Error:"
    }
}

fn warning_label() -> &'static str {
    if color_enabled() {
        "\x1b[33;1mWarning:\x1b[0m"
    } else {
        "Warning:"
    }
}
