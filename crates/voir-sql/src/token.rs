use voir_types::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Keywords
    Select,
    From,
    Where,
    And,
    Or,
    Not,
    As,
    Limit,
    Offset,
    Explain,
    Null,
    True,
    False,

    // Identifiers & literals
    Ident(String),
    QuotedIdent(String),
    StringLiteral(String),
    Number(String),

    // Punctuation
    Comma,
    Star,
    Dot,
    LParen,
    RParen,
    Semicolon,

    // Operators
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Plus,
    Minus,
    Slash,
    Percent,

    Eof,
}

/// The dialect's reserved words, exposed for completion in the REPL.
pub const KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "AND", "OR", "NOT", "AS", "LIMIT", "OFFSET", "EXPLAIN", "NULL",
    "TRUE", "FALSE",
];

impl TokenKind {
    #[must_use]
    pub fn keyword_from_str(s: &str) -> Option<TokenKind> {
        match s.to_ascii_uppercase().as_str() {
            "SELECT" => Some(TokenKind::Select),
            "FROM" => Some(TokenKind::From),
            "WHERE" => Some(TokenKind::Where),
            "AND" => Some(TokenKind::And),
            "OR" => Some(TokenKind::Or),
            "NOT" => Some(TokenKind::Not),
            "AS" => Some(TokenKind::As),
            "LIMIT" => Some(TokenKind::Limit),
            "OFFSET" => Some(TokenKind::Offset),
            "EXPLAIN" => Some(TokenKind::Explain),
            "NULL" => Some(TokenKind::Null),
            "TRUE" => Some(TokenKind::True),
            "FALSE" => Some(TokenKind::False),
            _ => None,
        }
    }
}

pub(crate) fn describe(kind: &TokenKind) -> String {
    match kind {
        TokenKind::Select => "SELECT".into(),
        TokenKind::From => "FROM".into(),
        TokenKind::Where => "WHERE".into(),
        TokenKind::And => "AND".into(),
        TokenKind::Or => "OR".into(),
        TokenKind::Not => "NOT".into(),
        TokenKind::As => "AS".into(),
        TokenKind::Limit => "LIMIT".into(),
        TokenKind::Offset => "OFFSET".into(),
        TokenKind::Explain => "EXPLAIN".into(),
        TokenKind::Null => "NULL".into(),
        TokenKind::True => "TRUE".into(),
        TokenKind::False => "FALSE".into(),
        TokenKind::Ident(s) | TokenKind::QuotedIdent(s) => format!("identifier `{s}`"),
        TokenKind::StringLiteral(s) => format!("string '{s}'"),
        TokenKind::Number(n) => format!("number {n}"),
        TokenKind::Comma => "','".into(),
        TokenKind::Star => "'*'".into(),
        TokenKind::Dot => "'.'".into(),
        TokenKind::LParen => "'('".into(),
        TokenKind::RParen => "')'".into(),
        TokenKind::Semicolon => "';'".into(),
        TokenKind::Eq => "'='".into(),
        TokenKind::NotEq => "'<>'".into(),
        TokenKind::Lt => "'<'".into(),
        TokenKind::LtEq => "'<='".into(),
        TokenKind::Gt => "'>'".into(),
        TokenKind::GtEq => "'>='".into(),
        TokenKind::Plus => "'+'".into(),
        TokenKind::Minus => "'-'".into(),
        TokenKind::Slash => "'/'".into(),
        TokenKind::Percent => "'%'".into(),
        TokenKind::Eof => "end of input".into(),
    }
}
