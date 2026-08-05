use nu_ansi_term::{Color, Style};
use reedline::{Highlighter, StyledText};
use voir_sql::{Token, TokenKind};

/// `reedline::Highlighter` over [`colorize`]. `color` is decided once at
/// startup (TTY + `NO_COLOR`), matching the old `VoirHelper`'s behavior.
pub struct VoirHighlighter {
    pub color: bool,
}

impl Highlighter for VoirHighlighter {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        if !self.color {
            let mut out = StyledText::new();
            out.push((Style::new(), line.to_string()));
            return out;
        }
        colorize(line).unwrap_or_else(|| {
            let mut out = StyledText::new();
            out.push((Style::new(), line.to_string()));
            out
        })
    }
}

/// Colorizes `line` by walking `voir_sql`'s token stream — the same lexer
/// `statement::is_complete` uses, so highlighting never drifts from what the
/// dialect actually accepts. Returns `None` on a lex error (most commonly an
/// unterminated string while the user is still typing); the caller falls
/// back to the raw line in that case.
pub fn colorize(line: &str) -> Option<StyledText> {
    let tokens = voir_sql::tokenize(line).ok()?;
    let mut out = StyledText::new();
    let mut cursor = 0usize;
    for token in &tokens {
        if matches!(token.kind, TokenKind::Eof) {
            break;
        }
        let start = token.span.start as usize;
        let end = token.span.end as usize;
        let gap = line.get(cursor..start)?;
        let text = line.get(start..end)?;
        if !gap.is_empty() {
            out.push((Style::new(), gap.to_string()));
        }
        out.push((style_for(token), text.to_string()));
        cursor = end;
    }
    let tail = line.get(cursor..)?;
    if !tail.is_empty() {
        out.push((Style::new(), tail.to_string()));
    }
    Some(out)
}

fn style_for(token: &Token) -> Style {
    match &token.kind {
        kind if is_keyword(kind) => Style::new().fg(Color::LightGreen),
        TokenKind::Ident(name) if is_known_function(name) => {
            Style::new().fg(Color::LightCyan).bold()
        }
        TokenKind::StringLiteral(_) | TokenKind::QuotedIdent(_) => {
            Style::new().fg(Color::Yellow).italic()
        }
        TokenKind::Number(_) => Style::new().fg(Color::LightMagenta),
        TokenKind::Star
        | TokenKind::Comma
        | TokenKind::LParen
        | TokenKind::RParen
        | TokenKind::Dot => Style::new().fg(Color::Purple),
        TokenKind::Semicolon => Style::new().fg(Color::White).bold(),
        _ => Style::new(),
    }
}

fn is_known_function(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    voir_plan::ScalarFunc::from_name(&lower).is_some()
        || voir_plan::agg_func_from_name(&lower).is_some()
}

fn is_keyword(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Select
            | TokenKind::From
            | TokenKind::Where
            | TokenKind::And
            | TokenKind::Or
            | TokenKind::Not
            | TokenKind::As
            | TokenKind::Limit
            | TokenKind::Offset
            | TokenKind::Explain
            | TokenKind::Null
            | TokenKind::True
            | TokenKind::False
            | TokenKind::Distinct
            | TokenKind::Group
            | TokenKind::By
            | TokenKind::Having
            | TokenKind::Order
            | TokenKind::Asc
            | TokenKind::Desc
            | TokenKind::Case
            | TokenKind::When
            | TokenKind::Then
            | TokenKind::Else
            | TokenKind::End
            | TokenKind::Cast
            | TokenKind::Join
            | TokenKind::Inner
            | TokenKind::Left
            | TokenKind::Right
            | TokenKind::Full
            | TokenKind::Outer
            | TokenKind::Cross
            | TokenKind::On
            | TokenKind::Using
            | TokenKind::With
            | TokenKind::Union
            | TokenKind::Except
            | TokenKind::Intersect
            | TokenKind::All
            | TokenKind::Exists
            | TokenKind::In
    )
}

#[cfg(test)]
mod tests {
    use nu_ansi_term::Style;

    use super::colorize;

    /// Concatenates a `StyledText`'s segments back into plain text, for
    /// asserting the underlying content survived styling unchanged.
    fn plain(styled: &reedline::StyledText) -> String {
        styled.buffer.iter().map(|(_, s)| s.as_str()).collect()
    }

    #[test]
    fn colorizes_keywords() {
        let out = colorize("SELECT a FROM t").expect("valid SQL colorizes");
        assert!(plain(&out).contains("SELECT"));
        let segments: Vec<&str> = out.buffer.iter().map(|(_, s)| s.as_str()).collect();
        assert!(
            out.buffer.iter().any(|(style, _)| *style != Style::new()),
            "expected at least one styled segment in {segments:?}"
        );
    }

    #[test]
    fn lex_error_returns_none() {
        assert!(colorize("select * from '").is_none());
    }

    #[test]
    fn round_trips_plain_text() {
        let cases = [
            "SELECT a, b FROM t WHERE a = 1",
            "SELECT COUNT(*) FROM t -- trailing comment",
            "SELECT café FROM t", // multi-byte identifier
            "SELECT 1.5e10, 'it''s a string'",
        ];
        for src in cases {
            let out = colorize(src).unwrap_or_else(|| panic!("{src:?} should colorize"));
            assert_eq!(plain(&out), src, "round-trip mismatch for {src:?}");
        }
    }
}
