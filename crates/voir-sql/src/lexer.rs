//! Text-to-token lexing built on `nom`: numbers, identifiers/keywords and
//! operators are recognized with combinators; string/quoted-ident literals
//! use a small hand loop since SQL's doubled-quote escaping (`''` -> `'`)
//! isn't a natural fit for `nom`'s escape combinators.

use nom::branch::alt;
use nom::bytes::complete::{tag, take_while, take_while1};
use nom::character::complete::{char, digit1, multispace1, one_of};
use nom::combinator::{map, opt, recognize, value};
use nom::sequence::pair;
use nom::{Offset, Parser};

use voir_types::{Span, VoirError};

use crate::token::{Token, TokenKind};

type LResult<'a, O> = nom::IResult<&'a str, O, nom::error::Error<&'a str>>;

pub(crate) fn tokenize(src: &str) -> Result<Vec<Token>, VoirError> {
    let mut tokens = Vec::new();
    let mut rest = src;
    loop {
        rest = skip_trivia(src, rest)?;
        let start = src.offset(rest);
        if rest.is_empty() {
            let pos = as_u32(start);
            tokens.push(Token {
                kind: TokenKind::Eof,
                span: Span::new(pos, pos),
            });
            break;
        }
        let (next, kind) = lex_token(src, rest, start)?;
        let end = src.offset(next);
        tokens.push(Token {
            kind,
            span: Span::new(as_u32(start), as_u32(end)),
        });
        rest = next;
    }
    Ok(tokens)
}

// Span offsets are u32 by design (plan.md section 4) to keep tokens small; a
// SQL query anywhere near 4 GiB isn't a real scenario.
#[allow(clippy::cast_possible_truncation)]
fn as_u32(n: usize) -> u32 {
    n as u32
}

fn ws1(input: &str) -> LResult<'_, &str> {
    multispace1.parse(input)
}

fn skip_trivia<'a>(full: &str, mut input: &'a str) -> Result<&'a str, VoirError> {
    loop {
        if let Ok((rest, _)) = ws1(input) {
            input = rest;
            continue;
        }
        if let Some(rest) = input.strip_prefix("--") {
            let len = rest.find('\n').unwrap_or(rest.len());
            input = &rest[len..];
            continue;
        }
        if let Some(rest) = input.strip_prefix("/*") {
            let start = full.offset(input);
            if let Some(idx) = rest.find("*/") {
                input = &rest[idx + 2..];
                continue;
            }
            return Err(VoirError::Parse {
                message: "unterminated block comment".into(),
                span: Span::new(as_u32(start), as_u32(full.len())),
            });
        }
        break;
    }
    Ok(input)
}

fn lex_token<'a>(
    full: &str,
    input: &'a str,
    start: usize,
) -> Result<(&'a str, TokenKind), VoirError> {
    let c = input
        .chars()
        .next()
        .expect("caller checked input is non-empty");
    match c {
        '\'' => {
            let (rest, value) = lex_quoted(full, input, '\'', start, "string literal")?;
            Ok((rest, TokenKind::StringLiteral(value)))
        }
        '"' => {
            let (rest, value) = lex_quoted(full, input, '"', start, "quoted identifier")?;
            Ok((rest, TokenKind::QuotedIdent(value)))
        }
        c if c.is_ascii_digit() => {
            let (rest, kind) =
                number_token(input).expect("leading digit guarantees a number token");
            Ok((rest, kind))
        }
        c if c.is_alphabetic() || c == '_' => {
            let (rest, kind) = ident_or_keyword(input)
                .expect("leading alpha/underscore guarantees an identifier token");
            Ok((rest, kind))
        }
        _ => operator_token(input).map_err(|_| VoirError::Parse {
            message: format!("unexpected character '{c}'"),
            span: Span::new(as_u32(start), as_u32(start + c.len_utf8())),
        }),
    }
}

/// Consumes a `quote`-delimited literal starting at `input[0] == quote`,
/// unescaping doubled quotes (`''` inside `'...'`, `""` inside `"..."`).
fn lex_quoted<'a>(
    full: &str,
    input: &'a str,
    quote: char,
    start: usize,
    what: &str,
) -> Result<(&'a str, String), VoirError> {
    let mut rest = &input[quote.len_utf8()..];
    let mut value = String::new();
    loop {
        match rest.chars().next() {
            Some(c) if c == quote => {
                let after = &rest[quote.len_utf8()..];
                if after.starts_with(quote) {
                    value.push(quote);
                    rest = &after[quote.len_utf8()..];
                } else {
                    rest = after;
                    break;
                }
            }
            Some(c) => {
                value.push(c);
                rest = &rest[c.len_utf8()..];
            }
            None => {
                let end = full.offset(rest);
                return Err(VoirError::Parse {
                    message: format!("unterminated {what}"),
                    span: Span::new(as_u32(start), as_u32(end)),
                });
            }
        }
    }
    Ok((rest, value))
}

fn number_token(input: &str) -> LResult<'_, TokenKind> {
    map(
        recognize(pair(
            digit1,
            pair(
                opt(pair(char('.'), digit1)),
                opt((one_of("eE"), opt(one_of("+-")), digit1)),
            ),
        )),
        |s: &str| TokenKind::Number(s.to_string()),
    )
    .parse(input)
}

fn ident_or_keyword(input: &str) -> LResult<'_, TokenKind> {
    map(
        recognize(pair(
            take_while1(|c: char| c.is_alphabetic() || c == '_'),
            take_while(|c: char| c.is_alphanumeric() || c == '_'),
        )),
        |s: &str| TokenKind::keyword_from_str(s).unwrap_or_else(|| TokenKind::Ident(s.to_string())),
    )
    .parse(input)
}

fn operator_token(input: &str) -> LResult<'_, TokenKind> {
    alt((multi_char_operator, single_char_operator)).parse(input)
}

fn multi_char_operator(input: &str) -> LResult<'_, TokenKind> {
    alt((
        value(TokenKind::LtEq, tag("<=")),
        value(TokenKind::NotEq, tag("<>")),
        value(TokenKind::GtEq, tag(">=")),
        value(TokenKind::NotEq, tag("!=")),
    ))
    .parse(input)
}

fn single_char_operator(input: &str) -> LResult<'_, TokenKind> {
    alt((
        value(TokenKind::Comma, char(',')),
        value(TokenKind::Star, char('*')),
        value(TokenKind::Dot, char('.')),
        value(TokenKind::LParen, char('(')),
        value(TokenKind::RParen, char(')')),
        value(TokenKind::Semicolon, char(';')),
        value(TokenKind::Plus, char('+')),
        value(TokenKind::Minus, char('-')),
        value(TokenKind::Slash, char('/')),
        value(TokenKind::Percent, char('%')),
        value(TokenKind::Eq, char('=')),
        value(TokenKind::Lt, char('<')),
        value(TokenKind::Gt, char('>')),
    ))
    .parse(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        tokenize(src).unwrap().into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn tokenizes_simple_select() {
        assert_eq!(
            kinds("SELECT a FROM t"),
            vec![
                TokenKind::Select,
                TokenKind::Ident("a".into()),
                TokenKind::From,
                TokenKind::Ident("t".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn keywords_are_case_insensitive() {
        assert_eq!(kinds("select")[0], TokenKind::Select);
        assert_eq!(kinds("Select")[0], TokenKind::Select);
        assert_eq!(kinds("SELECT")[0], TokenKind::Select);
    }

    #[test]
    fn identifier_with_digits_and_underscore() {
        assert_eq!(kinds("_a1_b2")[0], TokenKind::Ident("_a1_b2".into()));
    }

    #[test]
    fn lexes_string_literal_with_escaped_quote() {
        assert_eq!(kinds("'it''s'")[0], TokenKind::StringLiteral("it's".into()));
    }

    #[test]
    fn lexes_quoted_ident_with_escaped_quote() {
        assert_eq!(
            kinds("\"a\"\"b\"")[0],
            TokenKind::QuotedIdent("a\"b".into())
        );
    }

    #[test]
    fn unterminated_string_errors() {
        let err = tokenize("'abc").unwrap_err();
        assert!(err.to_string().contains("unterminated string literal"));
    }

    #[test]
    fn unterminated_quoted_ident_errors() {
        let err = tokenize("\"abc").unwrap_err();
        assert!(err.to_string().contains("unterminated quoted identifier"));
    }

    #[test]
    fn unterminated_block_comment_errors() {
        let err = tokenize("/* comment").unwrap_err();
        assert!(err.to_string().contains("unterminated block comment"));
    }

    #[test]
    fn line_comment_runs_to_newline() {
        assert_eq!(kinds("-- skip\nSELECT")[0], TokenKind::Select);
    }

    #[test]
    fn block_comment_is_skipped() {
        assert_eq!(kinds("/* skip */ SELECT")[0], TokenKind::Select);
    }

    #[test]
    fn lexes_integer_float_and_exponent_forms() {
        assert_eq!(kinds("1")[0], TokenKind::Number("1".into()));
        assert_eq!(kinds("1.5")[0], TokenKind::Number("1.5".into()));
        assert_eq!(kinds("1e10")[0], TokenKind::Number("1e10".into()));
        assert_eq!(kinds("1.5e-10")[0], TokenKind::Number("1.5e-10".into()));
        assert_eq!(kinds("1E+3")[0], TokenKind::Number("1E+3".into()));
    }

    #[test]
    fn trailing_e_without_digits_is_not_an_exponent() {
        let ks = kinds("1e");
        assert_eq!(ks[0], TokenKind::Number("1".into()));
        assert_eq!(ks[1], TokenKind::Ident("e".into()));
    }

    #[test]
    fn lexes_multi_char_operators_before_single_char_prefixes() {
        assert_eq!(
            kinds("<= <> >= != < >"),
            vec![
                TokenKind::LtEq,
                TokenKind::NotEq,
                TokenKind::GtEq,
                TokenKind::NotEq,
                TokenKind::Lt,
                TokenKind::Gt,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn unexpected_character_errors() {
        let err = tokenize("@").unwrap_err();
        assert!(err.to_string().contains("unexpected character '@'"));
    }

    #[test]
    fn lone_bang_errors() {
        assert!(tokenize("!").is_err());
    }

    #[test]
    fn semicolon_inside_string_is_not_a_statement_boundary() {
        let ks = kinds("SELECT ';' FROM t");
        assert!(!ks.contains(&TokenKind::Semicolon));
    }

    #[test]
    fn every_token_span_is_ordered_and_within_source_bounds() {
        let src = "SELECT a FROM t WHERE a = 1";
        for tok in tokenize(src).unwrap() {
            assert!(tok.span.start as usize <= src.len());
            assert!(tok.span.end as usize <= src.len());
            assert!(tok.span.start <= tok.span.end);
        }
    }
}
