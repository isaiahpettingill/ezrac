use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceLocation {
    pub file: PathBuf,
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourcePosition {
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceSpan {
    pub file: PathBuf,
    pub start: SourcePosition,
    pub end: SourcePosition,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub span: Option<SourceSpan>,
    pub message: String,
}

impl Diagnostic {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            span: None,
            message: message.into(),
        }
    }

    pub fn at(location: SourceLocation, message: impl Into<String>) -> Self {
        let start = SourcePosition {
            line: location.line,
            column: location.column,
        };
        Self {
            span: Some(SourceSpan {
                file: location.file,
                start,
                end: SourcePosition {
                    line: start.line,
                    column: start.column.saturating_add(1),
                },
            }),
            message: message.into(),
        }
    }

    pub fn at_span(span: SourceSpan, message: impl Into<String>) -> Self {
        Self {
            span: Some(span),
            message: message.into(),
        }
    }

    pub fn with_location_if_missing(mut self, location: SourceLocation) -> Self {
        if self.span.is_none() {
            let start = SourcePosition {
                line: location.line,
                column: location.column,
            };
            self.span = Some(SourceSpan {
                file: location.file,
                start,
                end: SourcePosition {
                    line: start.line,
                    column: start.column.saturating_add(1),
                },
            });
        }
        self
    }

    pub fn with_span_if_missing(mut self, span: SourceSpan) -> Self {
        if self.span.is_none() {
            self.span = Some(span);
        }
        self
    }

    pub fn location(&self) -> Option<SourceLocation> {
        self.span.as_ref().map(|span| SourceLocation {
            file: span.file.clone(),
            line: span.start.line,
            column: span.start.column,
        })
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(span) = &self.span {
            write!(
                f,
                "{}:{}:{}: {}",
                span.file.display(),
                span.start.line,
                span.start.column,
                self.message
            )
        } else {
            f.write_str(&self.message)
        }
    }
}

impl std::error::Error for Diagnostic {}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceToken {
    text: String,
    span: SourceSpan,
}

/// Locate the source construct named by a semantic diagnostic.
///
/// This is kept in the compiler diagnostic layer so every consumer receives a
/// structured range. The LSP never needs to reverse-engineer diagnostic text.
pub fn diagnostic_span(file: &Path, source: &str, message: &str) -> Option<SourceSpan> {
    let tokens = source_tokens(file, source);
    let quoted = message
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();

    if message.contains("has no field") {
        return quoted
            .last()
            .and_then(|name| token_span(&tokens, name, false));
    }
    if message.contains("duplicate") {
        return quoted
            .first()
            .and_then(|name| token_span(&tokens, name, true));
    }
    if message.contains("inline asm output") || message.contains("array index") {
        return quoted
            .first()
            .and_then(|name| token_span(&tokens, name, true));
    }
    if (message.contains("cast") || message.contains("pointer-to-integer"))
        && let Some(span) = token_span(&tokens, "cast", false)
    {
        return Some(span);
    }
    if message.contains("pointer arithmetic") || message.contains("subtract a pointer") {
        return token_span(&tokens, "+", false).or_else(|| token_span(&tokens, "-", false));
    }
    if message.contains("type mismatch") {
        for operator in ["==", "!=", "<=", ">=", "<", ">", "="] {
            if let Some(span) = token_span(&tokens, operator, false) {
                return Some(span);
            }
        }
    }
    if let Some(value) = message
        .strip_prefix("value ")
        .and_then(|message| message.split_whitespace().next())
        .and_then(|value| value.parse::<i64>().ok())
        && let Some(token) = tokens
            .iter()
            .find(|token| source_integer_value(&token.text) == Some(value))
    {
        return Some(token.span.clone());
    }
    for name in quoted {
        if let Some(span) = token_span(&tokens, name, false) {
            return Some(span);
        }
    }
    message
        .split_whitespace()
        .find_map(|word| {
            let word = word.trim_matches(|ch: char| !ch.is_ascii_alphanumeric());
            (!word.is_empty()).then_some(word)
        })
        .and_then(|word| token_span(&tokens, word, false))
}

pub fn source_token_spans(file: &Path, source: &str, text: &str) -> Vec<SourceSpan> {
    token_spans(&source_tokens(file, source), text)
}

fn source_integer_value(text: &str) -> Option<i64> {
    let text = text
        .strip_suffix("u24")
        .or_else(|| text.strip_suffix("i24"))
        .or_else(|| text.strip_suffix("u16"))
        .or_else(|| text.strip_suffix("i16"))
        .or_else(|| text.strip_suffix("u8"))
        .or_else(|| text.strip_suffix("i8"))
        .unwrap_or(text);
    if let Some(hex) = text.strip_prefix("0x") {
        i64::from_str_radix(hex, 16).ok()
    } else if let Some(binary) = text.strip_prefix("0b") {
        i64::from_str_radix(binary, 2).ok()
    } else {
        text.parse().ok()
    }
}

fn token_span(tokens: &[SourceToken], text: &str, last: bool) -> Option<SourceSpan> {
    let spans = token_spans(tokens, text);
    if last {
        spans.last().cloned()
    } else {
        spans.first().cloned()
    }
}

fn token_spans(tokens: &[SourceToken], text: &str) -> Vec<SourceSpan> {
    let direct = tokens
        .iter()
        .filter(|token| token.text == text)
        .map(|token| token.span.clone())
        .collect::<Vec<_>>();
    if !direct.is_empty() {
        return direct;
    }
    let parts = text.split('.').collect::<Vec<_>>();
    if parts.len() < 2 {
        return Vec::new();
    }
    let width = parts.len() * 2 - 1;
    tokens
        .windows(width)
        .filter_map(|window| {
            let matches = parts.iter().enumerate().all(|(index, part)| {
                window[index * 2].text == *part
                    && (index + 1 == parts.len() || window[index * 2 + 1].text == ".")
            });
            matches.then(|| SourceSpan {
                file: window.first().unwrap().span.file.clone(),
                start: window.first().unwrap().span.start,
                end: window.last().unwrap().span.end,
            })
        })
        .collect()
}

fn source_tokens(file: &Path, source: &str) -> Vec<SourceToken> {
    let mut tokens = Vec::new();
    let mut chars = source.char_indices().peekable();
    let mut line = 1usize;
    let mut column = 1usize;
    while let Some((_, ch)) = chars.next() {
        if ch == '\n' {
            line += 1;
            column = 1;
            continue;
        }
        if ch.is_whitespace() {
            column += 1;
            continue;
        }
        if ch == '/' && chars.peek().is_some_and(|(_, next)| *next == '/') {
            chars.next();
            column += 2;
            for (_, next) in chars.by_ref() {
                if next == '\n' {
                    line += 1;
                    column = 1;
                    break;
                }
                column += 1;
            }
            continue;
        }
        if matches!(ch, '"' | '\'') {
            let quote = ch;
            let mut escaped = false;
            column += 1;
            for (_, next) in chars.by_ref() {
                column += 1;
                if escaped {
                    escaped = false;
                } else if next == '\\' {
                    escaped = true;
                } else if next == quote {
                    break;
                } else if next == '\n' {
                    line += 1;
                    column = 1;
                }
            }
            continue;
        }

        let start = SourcePosition { line, column };
        let mut text = ch.to_string();
        if ch.is_ascii_alphanumeric() || ch == '_' {
            while let Some((_, next)) = chars.peek() {
                if !next.is_ascii_alphanumeric() && *next != '_' {
                    break;
                }
                text.push(*next);
                chars.next();
            }
        } else if matches!(
            ch,
            '=' | '!' | '<' | '>' | '+' | '-' | '*' | '/' | '%' | '&' | '|' | '^'
        ) {
            if chars.peek().is_some_and(|(_, next)| *next == '=') {
                text.push('=');
                chars.next();
            } else if matches!(ch, '<' | '>') && chars.peek().is_some_and(|(_, next)| *next == ch) {
                text.push(ch);
                chars.next();
                if chars.peek().is_some_and(|(_, next)| *next == '=') {
                    text.push('=');
                    chars.next();
                }
            }
        }
        let width = text.chars().count();
        column += width;
        tokens.push(SourceToken {
            text,
            span: SourceSpan {
                file: file.to_path_buf(),
                start,
                end: SourcePosition { line, column },
            },
        });
    }
    tokens
}

#[cfg(test)]
mod tests;
