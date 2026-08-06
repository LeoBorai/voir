use reedline::{ValidationResult, Validator};

use crate::statement;

/// Multiline continuation: a buffer is complete once it contains a
/// top-level `;`, except a `\`-prefixed meta-command line, which has no `;`
/// and must not trap the user in a continuation prompt.
pub struct VoirValidator;

impl Validator for VoirValidator {
    fn validate(&self, line: &str) -> ValidationResult {
        if line.trim_start().starts_with('\\') || statement::is_complete(line) {
            ValidationResult::Complete
        } else {
            ValidationResult::Incomplete
        }
    }
}
