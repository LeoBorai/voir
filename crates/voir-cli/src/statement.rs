use voir_sql::TokenKind;
use voir_types::VoirError;

/// Whether `buffer` contains a `;` outside of a string or comment — the
/// signal the REPL uses to stop accumulating continuation lines. Reuses the
/// lexer rather than a separate heuristic so the two never drift apart.
pub fn is_complete(buffer: &str) -> bool {
    match voir_sql::tokenize(buffer) {
        Ok(tokens) => tokens
            .iter()
            .any(|t| matches!(t.kind, TokenKind::Semicolon)),
        Err(_) => false,
    }
}

/// Splits `src` into individual statement texts on top-level `;` boundaries,
/// for `\i <file>`.
pub fn split(src: &str) -> Result<Vec<String>, VoirError> {
    let tokens = voir_sql::tokenize(src)?;
    let mut statements = Vec::new();
    let mut start = 0usize;
    for token in &tokens {
        if matches!(token.kind, TokenKind::Semicolon) {
            let text = src[start..token.span.end as usize].trim();
            if !text.is_empty() {
                statements.push(text.to_string());
            }
            start = token.span.end as usize;
        }
    }
    let tail = src[start..].trim();
    if !tail.is_empty() {
        statements.push(tail.to_string());
    }
    Ok(statements)
}
