//! Caches inferred schemas for `FROM '<file>'` targets that aren't
//! registered tables, keyed by `(path, mtime, len)` so a 5 MB CSV isn't
//! re-scanned on every keystroke. Bounded: files above [`MAX_INFER_BYTES`]
//! are skipped, and the cache is consulted only from `Expression` context —
//! never while the path itself is still being typed (`PathLiteral`).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use voir_catalog::TableProvider;
use voir_source_csv::CsvTableProvider;

/// `testdata/earthquakes.csv` is 5.4 MB; this leaves headroom for that
/// fixture while still bounding the hot-path work for anything larger.
const MAX_INFER_BYTES: u64 = 16 * 1024 * 1024;

struct Cached {
    mtime_secs: u64,
    len: u64,
    columns: Vec<String>,
}

#[derive(Default)]
pub struct SchemaCache {
    entries: HashMap<PathBuf, Cached>,
}

impl SchemaCache {
    /// Column names for `path`, inferring (and caching) only if it's a CSV
    /// file within the size bound and not already cached at its current
    /// `(mtime, len)`. Any I/O or inference failure yields `None` rather
    /// than surfacing an error on the completion hot path.
    pub fn columns_for(&mut self, path: &Path) -> Option<Vec<String>> {
        if path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
            != Some("csv")
        {
            return None;
        }
        let meta = fs::metadata(path).ok()?;
        if !meta.is_file() || meta.len() > MAX_INFER_BYTES {
            return None;
        }
        let mtime_secs = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());

        if let Some(cached) = self.entries.get(path) {
            if cached.mtime_secs == mtime_secs && cached.len == meta.len() {
                return Some(cached.columns.clone());
            }
        }

        let provider = CsvTableProvider::open(path).ok()?;
        let columns: Vec<String> = provider
            .schema()
            .fields
            .iter()
            .map(|f| f.name.clone())
            .collect();
        self.entries.insert(
            path.to_path_buf(),
            Cached {
                mtime_secs,
                len: meta.len(),
                columns: columns.clone(),
            },
        );
        Some(columns)
    }
}
