//! Spans and diagnostics.

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Span { Span { start, end } }
    pub fn to(self, other: Span) -> Span { Span { start: self.start, end: other.end.max(self.end) } }
}

#[derive(Debug, Clone)]
pub struct Diag {
    pub msg: String,
    pub span: Span,
}

impl Diag {
    pub fn new(msg: impl Into<String>, span: Span) -> Diag { Diag { msg: msg.into(), span } }

    /// Render as `file:line:col: error: msg` plus the offending source line with a caret.
    pub fn render(&self, file: &str, src: &str) -> String {
        let (line, col) = line_col(src, self.span.start);
        let text = src.lines().nth(line - 1).unwrap_or("");
        let width = (self.span.end.saturating_sub(self.span.start)).max(1).min(text.len().saturating_sub(col - 1).max(1));
        format!(
            "{}:{}:{}: error: {}\n {:>4} | {}\n      | {}{}\n",
            file, line, col, self.msg, line, text, " ".repeat(col - 1), "^".repeat(width)
        )
    }
}

pub fn line_col(src: &str, pos: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    for (i, ch) in src.char_indices() {
        if i >= pos { break; }
        if ch == '\n' { line += 1; col = 1; } else { col += 1; }
    }
    (line, col)
}
