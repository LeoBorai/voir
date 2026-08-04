use thiserror::Error;

use crate::span::Span;

#[derive(Debug, Error)]
pub enum VoirError {
    #[error("parse error at {span:?}: {message}")]
    Parse { message: String, span: Span },

    #[error("type error: {0}")]
    Type(String),

    #[error("plan error: {0}")]
    Plan(String),

    #[error("execution error: {0}")]
    Exec(String),

    #[error("source error: {0}")]
    Source(String),

    #[error("{0}")]
    NotFound(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}
