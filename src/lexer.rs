//! Chimera lexer.

use crate::diag::{Diag, Span};

#[derive(Debug, Clone, PartialEq)]
pub enum Tk {
    Ident(String),
    Kw(&'static str),
    Int(String),
    Float(String),
    /// exact decimal literal such as `1.0999` (floats need an exponent or an `f` suffix)
    Decimal(String),
    Str(Vec<StrPart>),
    Op(&'static str),
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StrPart {
    Lit(String),
    Expr(String, usize), // source text, absolute offset
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tk: Tk,
    pub span: Span,
    /// a newline separates this token from the previous one
    pub nl_before: bool,
}

pub const KEYWORDS: &[&str] = &[
    "fun", "val", "var", "class", "trait", "data", "enum", "struct", "type", "abstract", "override", "open",
    "generic", "method", "before", "after", "around", "require", "ensure", "invariant", "old", "result",
    "if", "else", "while", "for", "in", "return", "break", "continue", "match", "true", "false", "nil",
    "this", "super", "is", "keyof", "sheet",
];

// longest first
const OPS: &[&str] = &[
    "->>", "...", "?.", "?:", "!!", "->", "=>", "==", "!=", "<>", "<=", ">=", "&&", "||", "+=", "-=", "*=", "/=",
    "..", "+", "-", "*", "/", "%", "^", "&", "<", ">", "=", "!", "?", ".", ",", ":", ";", "(", ")", "[", "]",
    "{", "}", "|", "@",
];

pub fn lex(src: &str, base: usize) -> Result<Vec<Token>, Diag> {
    let b = src.as_bytes();
    let mut toks = Vec::new();
    let mut i = 0usize;
    let mut nl = true;
    macro_rules! push {
        ($tk:expr, $start:expr) => {{
            toks.push(Token { tk: $tk, span: Span::new(base + $start, base + i), nl_before: nl });
            nl = false;
        }};
    }
    let is_id = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    while i < b.len() {
        let c = b[i];
        if c == b'\n' { nl = true; i += 1; continue; }
        if c == b' ' || c == b'\t' || c == b'\r' { i += 1; continue; }
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' { i += 1; }
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') { if b[i] == b'\n' { nl = true; } i += 1; }
            i += 2;
            continue;
        }
        let start = i;
        if c.is_ascii_digit() {
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') { i += 1; }
            let mut float = false;
            let mut decimal = false;
            if i + 1 < b.len() && b[i] == b'.' && b[i + 1].is_ascii_digit() {
                decimal = true;
                i += 1;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') { i += 1; }
            }
            if i + 1 < b.len() && (b[i] == b'e' || b[i] == b'E') && (b[i + 1].is_ascii_digit() || b[i + 1] == b'-' || b[i + 1] == b'+') {
                float = true;
                i += 2;
                while i < b.len() && b[i].is_ascii_digit() { i += 1; }
            }
            let text = src[start..i].replace('_', "");
            // `1.5f` / `2f` => Float
            if i < b.len() && b[i] == b'f' && !b.get(i + 1).map_or(false, |c| is_id(*c)) { float = true; i += 1; }
            push!(if float { Tk::Float(text) } else if decimal { Tk::Decimal(text) } else { Tk::Int(text) }, start);
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            while i < b.len() && is_id(b[i]) { i += 1; }
            let w = &src[start..i];
            match KEYWORDS.iter().find(|k| **k == w) {
                Some(k) => push!(Tk::Kw(k), start),
                None => push!(Tk::Ident(w.to_string()), start),
            }
            continue;
        }
        if c == b'"' {
            i += 1;
            let mut parts = Vec::new();
            let mut lit = String::new();
            loop {
                if i >= b.len() || b[i] == b'\n' {
                    return Err(Diag::new("unterminated string literal", Span::new(base + start, base + i)));
                }
                let ch = b[i];
                if ch == b'"' { i += 1; break; }
                if ch == b'\\' {
                    let e = *b.get(i + 1).unwrap_or(&b' ');
                    let r = match e { b'n' => '\n', b't' => '\t', b'r' => '\r', b'"' => '"', b'\\' => '\\', b'$' => '$', b'0' => '\0',
                        _ => return Err(Diag::new(format!("unknown escape '\\{}'", e as char), Span::new(base + i, base + i + 2))) };
                    lit.push(r);
                    i += 2;
                    continue;
                }
                if ch == b'$' && b.get(i + 1) == Some(&b'{') {
                    if !lit.is_empty() { parts.push(StrPart::Lit(std::mem::take(&mut lit))); }
                    i += 2;
                    let es = i;
                    let mut depth = 1;
                    while i < b.len() {
                        match b[i] {
                            b'{' => depth += 1,
                            b'}' => { depth -= 1; if depth == 0 { break; } }
                            b'"' => { i += 1; while i < b.len() && b[i] != b'"' { if b[i] == b'\\' { i += 1; } i += 1; } }
                            _ => {}
                        }
                        i += 1;
                    }
                    parts.push(StrPart::Expr(src[es..i].to_string(), base + es));
                    i += 1;
                    continue;
                }
                if ch == b'$' && b.get(i + 1).map_or(false, |c| c.is_ascii_alphabetic() || *c == b'_') {
                    if !lit.is_empty() { parts.push(StrPart::Lit(std::mem::take(&mut lit))); }
                    i += 1;
                    let es = i;
                    while i < b.len() && is_id(b[i]) { i += 1; }
                    parts.push(StrPart::Expr(src[es..i].to_string(), base + es));
                    continue;
                }
                // copy one UTF-8 char
                let ch_len = utf8_len(ch);
                lit.push_str(&src[i..i + ch_len]);
                i += ch_len;
            }
            if !lit.is_empty() || parts.is_empty() { parts.push(StrPart::Lit(lit)); }
            push!(Tk::Str(parts), start);
            continue;
        }
        match OPS.iter().find(|o| src[i..].starts_with(**o)) {
            Some(op) => { i += op.len(); push!(Tk::Op(op), start); }
            None => {
                let ch = src[i..].chars().next().unwrap();
                return Err(Diag::new(format!("unexpected character '{}'", ch), Span::new(base + i, base + i + ch.len_utf8())));
            }
        }
    }
    toks.push(Token { tk: Tk::Eof, span: Span::new(base + i, base + i), nl_before: true });
    Ok(toks)
}

fn utf8_len(first: u8) -> usize {
    if first < 0x80 { 1 } else if first >> 5 == 0b110 { 2 } else if first >> 4 == 0b1110 { 3 } else { 4 }
}
