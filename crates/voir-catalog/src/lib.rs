use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use voir_types::{Batch, SchemaRef, VoirError};

mod filter;

pub use filter::{CompareOp, FilterExpr};

pub trait TableProvider: Send + Sync {
    fn schema(&self) -> SchemaRef;

    /// `projection` is a list of column indices; `filters` are a best-effort
    /// hint. Returning fewer filters as applied is always correct — the
    /// engine re-checks anything the source didn't handle.
    fn scan(&self, projection: Option<&[usize]>, filters: &[FilterExpr]) -> Result<Box<dyn BatchStream>, VoirError>;
}

pub trait BatchStream: Send {
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
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        extension: &str,
        ctor: impl Fn(&Path) -> Result<Arc<dyn TableProvider>, VoirError> + Send + Sync + 'static,
    ) {
        self.constructors.insert(extension.to_ascii_lowercase(), Box::new(ctor));
    }

    pub fn resolve(&self, path: &Path) -> Result<Arc<dyn TableProvider>, VoirError> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
        let ctor = self.constructors.get(&ext).ok_or_else(|| {
            VoirError::NotFound(format!(
                "no source adapter registered for extension '.{ext}' (path: {})",
                path.display()
            ))
        })?;
        ctor(path)
    }
}
