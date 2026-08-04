use voir_types::{Span, VoirError};

use crate::token::{Token, TokenKind};

pub struct Lexer<'a> {
    src: &'a str,
    chars: Vec<(u32, char)>,
    pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        let chars = src.char_indices().map(|(i, c)| (i as u32, c)).collect();
        Self { src, chars, pos: 0 }
    }

    pub fn tokenize(mut self) -> Result<Vec<Token>, VoirError> {
        let mut tokens = Vec::new();
        loop {
            self.skip_trivia()?;
            let start = self.byte_offset();
            let Some(ch) = self.peek() else {
                tokens.push(Token {
                    kind: TokenKind::Eof,
                    span: Span::new(start, start),
                });
                break;
            };
            let token = match ch {
                '\'' => self.lex_string(start)?,
                '"' => self.lex_quoted_ident(start)?,
                c if c.is_ascii_digit() => self.lex_number(start),
                c if c.is_alphabetic() || c == '_' => self.lex_ident_or_keyword(start),
                _ => self.lex_operator(start)?,
            };
            tokens.push(token);
        }
        Ok(tokens)
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).map(|&(_, c)| c)
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).map(|&(_, c)| c)
    }

    fn byte_offset(&self) -> u32 {
        self.chars.get(self.pos).map(|&(i, _)| i).unwrap_or(self.src.len() as u32)
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn skip_trivia(&mut self) -> Result<(), VoirError> {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => {
                    self.advance();
                }
                Some('-') if self.peek_at(1) == Some('-') => {
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.advance();
                    }
                }
                Some('/') if self.peek_at(1) == Some('*') => {
                    let start = self.byte_offset();
                    self.advance();
                    self.advance();
                    loop {
                        match (self.peek(), self.peek_at(1)) {
                            (Some('*'), Some('/')) => {
                                self.advance();
                                self.advance();
                                break;
                            }
                            (Some(_), _) => {
                                self.advance();
                            }
                            (None, _) => {
                                return Err(VoirError::Parse {
                                    message: "unterminated block comment".into(),
                                    span: Span::new(start, self.byte_offset()),
                                });
                            }
                        }
                    }
                }
                _ => break,
            }
        }
        Ok(())
    }

    fn lex_string(&mut self, start: u32) -> Result<Token, VoirError> {
        self.advance(); // opening '
        let mut value = String::new();
        loop {
            match self.advance() {
                Some('\'') => {
                    if self.peek() == Some('\'') {
                        self.advance();
                        value.push('\'');
                    } else {
                        break;
                    }
                }
                Some(c) => value.push(c),
                None => {
                    return Err(VoirError::Parse {
                        message: "unterminated string literal".into(),
                        span: Span::new(start, self.byte_offset()),
                    });
                }
            }
        }
        let end = self.byte_offset();
        Ok(Token {
            kind: TokenKind::StringLiteral(value),
            span: Span::new(start, end),
        })
    }

    fn lex_quoted_ident(&mut self, start: u32) -> Result<Token, VoirError> {
        self.advance(); // opening "
        let mut value = String::new();
        loop {
            match self.advance() {
                Some('"') => {
                    if self.peek() == Some('"') {
                        self.advance();
                        value.push('"');
                    } else {
                        break;
                    }
                }
                Some(c) => value.push(c),
                None => {
                    return Err(VoirError::Parse {
                        message: "unterminated quoted identifier".into(),
                        span: Span::new(start, self.byte_offset()),
                    });
                }
            }
        }
        let end = self.byte_offset();
        Ok(Token {
            kind: TokenKind::QuotedIdent(value),
            span: Span::new(start, end),
        })
    }

    fn lex_number(&mut self, start: u32) -> Token {
        let mut text = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                text.push(c);
                self.advance();
            } else {
                break;
            }
        }
        if self.peek() == Some('.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            text.push('.');
            self.advance();
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    text.push(c);
                    self.advance();
                } else {
                    break;
                }
            }
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            let save = self.pos;
            let mut exp = String::new();
            exp.push(self.peek().unwrap());
            self.advance();
            if matches!(self.peek(), Some('+' | '-')) {
                exp.push(self.peek().unwrap());
                self.advance();
            }
            if self.peek().is_some_and(|c| c.is_ascii_digit()) {
                while let Some(c) = self.peek() {
                    if c.is_ascii_digit() {
                        exp.push(c);
                        self.advance();
                    } else {
                        break;
                    }
                }
                text.push_str(&exp);
            } else {
                self.pos = save;
            }
        }
        let end = self.byte_offset();
        Token {
            kind: TokenKind::Number(text),
            span: Span::new(start, end),
        }
    }

    fn lex_ident_or_keyword(&mut self, start: u32) -> Token {
        let mut text = String::new();
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' {
                text.push(c);
                self.advance();
            } else {
                break;
            }
        }
        let end = self.byte_offset();
        let kind = TokenKind::keyword_from_str(&text).unwrap_or(TokenKind::Ident(text));
        Token {
            kind,
            span: Span::new(start, end),
        }
    }

    fn lex_operator(&mut self, start: u32) -> Result<Token, VoirError> {
        let c = self.advance().unwrap();
        let kind = match c {
            ',' => TokenKind::Comma,
            '*' => TokenKind::Star,
            '.' => TokenKind::Dot,
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            ';' => TokenKind::Semicolon,
            '+' => TokenKind::Plus,
            '-' => TokenKind::Minus,
            '/' => TokenKind::Slash,
            '%' => TokenKind::Percent,
            '=' => TokenKind::Eq,
            '<' => {
                if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::LtEq
                } else if self.peek() == Some('>') {
                    self.advance();
                    TokenKind::NotEq
                } else {
                    TokenKind::Lt
                }
            }
            '>' => {
                if self.peek() == Some('=') {
                    self.advance();
                    TokenKind::GtEq
                } else {
                    TokenKind::Gt
                }
            }
            '!' if self.peek() == Some('=') => {
                self.advance();
                TokenKind::NotEq
            }
            other => {
                return Err(VoirError::Parse {
                    message: format!("unexpected character '{other}'"),
                    span: Span::new(start, self.byte_offset()),
                });
            }
        };
        let end = self.byte_offset();
        Ok(Token {
            kind,
            span: Span::new(start, end),
        })
    }
}
