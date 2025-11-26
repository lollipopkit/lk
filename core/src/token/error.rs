use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct Position {
    pub line: u32,
    pub column: u32,
    pub offset: usize,
}

impl Position {
    pub fn new(line: u32, column: u32, offset: usize) -> Self {
        Self { line, column, offset }
    }

    pub fn start() -> Self {
        Self {
            line: 1,
            column: 1,
            offset: 0,
        }
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub start: Position,
    pub end: Position,
}

impl Span {
    pub fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }

    pub fn single(pos: Position) -> Self {
        Self {
            start: pos.clone(),
            end: pos,
        }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.start.line == self.end.line {
            write!(f, "{}:{}-{}", self.start.line, self.start.column, self.end.column)
        } else {
            write!(f, "{}-{}", self.start, self.end)
        }
    }
}

/// Enhanced parse error with position information
#[derive(Debug)]
pub struct ParseError {
    pub message: String,
    pub span: Option<Span>,
}

impl ParseError {
    pub fn new(message: String) -> Self {
        Self { message, span: None }
    }

    pub fn with_span(message: String, span: Span) -> Self {
        Self {
            message,
            span: Some(span),
        }
    }

    pub fn with_position(message: String, position: Position) -> Self {
        Self {
            message,
            span: Some(Span::single(position)),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(span) = &self.span {
            write!(f, "{} at {}", self.message, span)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

impl std::error::Error for ParseError {}

/// Helper to convert character index to line/column position
pub fn offset_to_position(text: &str, offset: usize) -> Position {
    let mut line = 1;
    let mut column = 1;

    for (i, ch) in text.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }

    Position::new(line, column, offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_offset_to_position() {
        let text = "line1\nline2\nline3";

        assert_eq!(offset_to_position(text, 0), Position::new(1, 1, 0));
        assert_eq!(offset_to_position(text, 5), Position::new(1, 6, 5)); // at '\n'
        assert_eq!(offset_to_position(text, 6), Position::new(2, 1, 6)); // start of line2
        assert_eq!(offset_to_position(text, 11), Position::new(2, 6, 11)); // at second '\n'
        assert_eq!(offset_to_position(text, 12), Position::new(3, 1, 12)); // start of line3
    }

    #[test]
    fn test_position_display() {
        let pos = Position::new(10, 25, 100);
        assert_eq!(pos.to_string(), "10:25");
    }

    #[test]
    fn test_span_display() {
        let span1 = Span::new(Position::new(1, 5, 4), Position::new(1, 10, 9));
        assert_eq!(span1.to_string(), "1:5-10");

        let span2 = Span::new(Position::new(1, 5, 4), Position::new(3, 2, 20));
        assert_eq!(span2.to_string(), "1:5-3:2");
    }

    #[test]
    fn test_parse_error_display() {
        let err1 = ParseError::new("simple error".to_string());
        assert_eq!(err1.to_string(), "simple error");

        let pos = Position::new(2, 10, 15);
        let err2 = ParseError::with_position("syntax error".to_string(), pos);
        assert_eq!(err2.to_string(), "syntax error at 2:10-10");
    }
}
