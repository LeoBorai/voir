//! Lexer, AST and parser for `voir`'s SQL dialect. No I/O, no evaluation —
//! this crate never learns what a file is.

pub mod ast;
mod lexer;
mod parser;
mod token;

use voir_types::VoirError;

pub use ast::*;
pub use parser::parse;
pub use token::{KEYWORDS, Token, TokenKind};

/// Lexes `src` without parsing it — used by the REPL to detect statement
/// boundaries (a `;` outside a string or comment) ahead of a full parse.
pub fn tokenize(src: &str) -> Result<Vec<Token>, VoirError> {
    lexer::tokenize(src)
}
