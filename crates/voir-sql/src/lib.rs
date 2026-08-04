//! Lexer, AST and parser for `voir`'s SQL dialect. No I/O, no evaluation —
//! this crate never learns what a file is.

pub mod ast;
mod lexer;
mod parser;
mod token;

pub use ast::*;
pub use parser::parse;
pub use token::{Token, TokenKind};
