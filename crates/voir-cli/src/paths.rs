//! File-path completion for [`suggest::Context::PathLiteral`] and
//! [`suggest::Context::MetaArgPath`] — the piece lost when `rustyline`'s
//! `FilenameCompleter` went away with the `reedline` migration.

use std::fs;
use std::path::Path;

use reedline::{Span, Suggestion};

use crate::suggest::Kind;

/// Extensions a `voir` `\open`/`FROM` can actually load — ranked above other
/// files so the list leads with what will work.
const PREFERRED_EXTENSIONS: &[&str] = &["csv", "json", "jsonc"];

/// Lists `prefix`'s parent directory, filtered to entries whose name starts
/// with the fragment after the last `/`. `pos` is the cursor's absolute byte
/// offset in the line, used to build a replacement span covering exactly
/// `prefix` (which the caller sliced out of the line ending at `pos`).
#[must_use]
pub fn complete(prefix: &str, pos: usize) -> Vec<Suggestion> {
    let expanded = expand_tilde(prefix);
    let (dir_str, fragment) = match expanded.rfind('/') {
        Some(idx) => (&expanded[..=idx], &expanded[idx + 1..]),
        None => ("", expanded.as_str()),
    };
    let read_dir = if dir_str.is_empty() {
        Path::new(".")
    } else {
        Path::new(dir_str)
    };
    let Ok(entries) = fs::read_dir(read_dir) else {
        return Vec::new();
    };
    let frag_lower = fragment.to_ascii_lowercase();

    let mut matches: Vec<(bool, bool, String)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            if !name.to_ascii_lowercase().starts_with(&frag_lower) {
                return None;
            }
            let is_dir = entry.path().is_dir();
            let preferred = is_dir
                || Path::new(&name)
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| {
                        PREFERRED_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str())
                    });
            Some((preferred, is_dir, name))
        })
        .collect();
    matches.sort_by_key(|a| (!a.0, a.2.to_ascii_lowercase()));

    let Some(start) = pos.checked_sub(prefix.len()) else {
        return Vec::new();
    };
    matches
        .into_iter()
        .map(|(_, is_dir, name)| {
            let mut value = format!("{dir_str}{name}");
            if is_dir {
                value.push('/');
            }
            let label = if is_dir { "dir" } else { Kind::File.label() };
            Suggestion {
                display_override: Some(format!("{value:<24} {label}")),
                value,
                span: Span::new(start, pos),
                append_whitespace: !is_dir,
                ..Default::default()
            }
        })
        .collect()
}

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix('~') {
        if rest.is_empty() || rest.starts_with('/') {
            if let Some(home) = std::env::var_os("HOME") {
                return format!("{}{rest}", home.to_string_lossy());
            }
        }
    }
    path.to_string()
}
