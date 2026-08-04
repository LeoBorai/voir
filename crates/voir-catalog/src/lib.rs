use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use voir_types::{Batch, SchemaRef, VoirError};

mod filter;

pub use filter::{CompareOp, FilterExpr};

pub trait TableProvider: Send + Sync {
    fn schema(&self) -> SchemaRef;

    /// `projection` is a list of column indices; `filters` are a best-effort
    /// hint. Returning fewer filters as applied is always correct — the
    /// engine re-checks anything the source didn't handle.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying source cannot be opened or scanned.
    fn scan(
        &self,
        projection: Option<&[usize]>,
        filters: &[FilterExpr],
    ) -> Result<Box<dyn BatchStream>, VoirError>;
}

pub trait BatchStream: Send {
    /// # Errors
    ///
    /// Returns an error if the next batch cannot be read from the underlying source.
    fn next_batch(&mut self) -> Result<Option<Batch>, VoirError>;
}

type Constructor = dyn Fn(&Path) -> Result<Arc<dyn TableProvider>, VoirError> + Send + Sync;

/// Maps a file extension to the adapter that opens it. `voir-cli` wires up
/// available adapters here at startup based on enabled features.
#[derive(Default)]
pub struct SourceRegistry {
    constructors: HashMap<String, Box<Constructor>>,
}

impl SourceRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        extension: &str,
        ctor: impl Fn(&Path) -> Result<Arc<dyn TableProvider>, VoirError> + Send + Sync + 'static,
    ) {
        self.constructors
            .insert(extension.to_ascii_lowercase(), Box::new(ctor));
    }

    /// # Errors
    ///
    /// Returns an error if no adapter is registered for the path's extension.
    pub fn resolve(&self, path: &Path) -> Result<Arc<dyn TableProvider>, VoirError> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let ctor = self.constructors.get(&ext).ok_or_else(|| {
            VoirError::NotFound(format!(
                "no source adapter registered for extension '.{ext}' (path: {})",
                path.display()
            ))
        })?;
        ctor(path)
    }
}

/// A file registered under an explicit name (`\open <path> AS <name>` in the
/// REPL), kept alongside its source path so `\dt`/`\d` can describe it
/// without re-opening the file.
pub struct RegisteredTable {
    pub path: PathBuf,
    pub provider: Arc<dyn TableProvider>,
}

/// Named tables layered over a `SourceRegistry`. A `FROM` reference that
/// matches a registered name resolves to that table directly; anything else
/// falls back to treating the reference as a file path.
#[derive(Default)]
pub struct Catalog {
    registry: SourceRegistry,
    tables: HashMap<String, RegisteredTable>,
}

impl Catalog {
    #[must_use]
    pub fn new(registry: SourceRegistry) -> Self {
        Self {
            registry,
            tables: HashMap::new(),
        }
    }

    pub fn register_table(&mut self, name: &str, path: PathBuf, provider: Arc<dyn TableProvider>) {
        self.tables.insert(
            name.to_ascii_lowercase(),
            RegisteredTable { path, provider },
        );
    }

    #[must_use]
    pub fn table(&self, name: &str) -> Option<&RegisteredTable> {
        self.tables.get(&name.to_ascii_lowercase())
    }

    pub fn tables(&self) -> impl Iterator<Item = (&str, &RegisteredTable)> {
        self.tables
            .iter()
            .map(|(name, table)| (name.as_str(), table))
    }

    /// Opens `path` through the extension registry without registering it as
    /// a named table — used by `\open` to validate before committing a name.
    pub fn open(&self, path: &Path) -> Result<Arc<dyn TableProvider>, VoirError> {
        self.registry.resolve(path)
    }

    pub fn resolve(&self, reference: &str) -> Result<Arc<dyn TableProvider>, VoirError> {
        if let Some(table) = self.table(reference) {
            return Ok(table.provider.clone());
        }
        self.registry.resolve(Path::new(reference))
    }
}
