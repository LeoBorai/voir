use nu_ansi_term::{Color, Style};
use voir_sql::{Token, TokenKind};

/// Colorizes `line` by walking `voir_sql`'s token stream — the same lexer
/// `statement::is_complete` uses, so highlighting never drifts from what the
/// dialect actually accepts. Returns `None` on a lex error (most commonly an
/// unterminated string while the user is still typing); the caller falls
/// back to the raw line in that case.
pub fn colorize(line: &str) -> Option<String> {
    let tokens = voir_sql::tokenize(line).ok()?;
    let mut out = String::with_capacity(line.len() * 2);
    let mut cursor = 0usize;
    for token in &tokens {
        if matches!(token.kind, TokenKind::Eof) {
            break;
        }
        let start = token.span.start as usize;
        let end = token.span.end as usize;
        let gap = line.get(cursor..start)?;
        let text = line.get(start..end)?;
        out.push_str(gap);
        out.push_str(&style_for(token).paint(text).to_string());
        cursor = end;
    }
    out.push_str(line.get(cursor..)?);
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
    use super::colorize;

    /// Strips ANSI SGR sequences (`\x1b[...m`) back out, for asserting the
    /// underlying text survived styling unchanged.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                for c in chars.by_ref() {
                    if c == 'm' {
                        break;
                    }
                }
                continue;
            }
            out.push(c);
        }
        out
    }

    #[test]
    fn colorizes_keywords() {
        let out = colorize("SELECT a FROM t").expect("valid SQL colorizes");
        assert!(out.contains("SELECT"));
        assert!(out.contains("\u{1b}["), "expected ANSI escapes in {out:?}");
    }

    #[test]
    fn lex_error_returns_none() {
        assert_eq!(colorize("select * from '"), None);
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
            assert_eq!(strip_ansi(&out), src, "round-trip mismatch for {src:?}");
        }
    }
}
