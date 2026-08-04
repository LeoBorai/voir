use miette::{Diagnostic, SourceSpan};
use thiserror::Error;
use voir_types::VoirError;

#[derive(Debug, Error, Diagnostic)]
#[error("{message}")]
struct SqlDiagnostic {
    message: String,
    #[source_code]
    src: String,
    #[label("here")]
    span: SourceSpan,
}

/// Prints `err` to stderr — parse errors get a `miette` rendering with a
/// caret under the offending span; everything else prints plainly.
pub fn report(query: &str, err: &VoirError) {
    if let VoirError::Parse { message, span } = err {
        let len = (span.end - span.start).max(1) as usize;
        let diagnostic = SqlDiagnostic {
            message: message.clone(),
            src: query.to_string(),
            span: (span.start as usize, len).into(),
        };
        eprintln!("{:?}", miette::Report::new(diagnostic));
    } else {
        eprintln!("error: {err}");
    }
}
