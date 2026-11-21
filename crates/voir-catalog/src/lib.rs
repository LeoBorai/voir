use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use voir_types::{Batch, DataType, Field, Row, SchemaRef, Value, VoirError};

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

    /// For a column holding nested objects (e.g. a JSON object), the fields
    /// found inside it, in first-seen order. `None` when the column has no
    /// known structure. Powers `<column>.*` expansion.
    fn nested_fields(&self, _column: usize) -> Option<Vec<Field>> {
        None
    }
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
    /// Created by `CREATE TABLE ... AS`: lives only in memory, has no
    /// backing file to re-open.
    pub in_memory: bool,
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
            RegisteredTable {
                path,
                provider,
                in_memory: false,
            },
        );
    }

    /// Registers a table materialized by `CREATE TABLE ... AS`, replacing any
    /// table of the same name.
    pub fn register_memory_table(&mut self, name: &str, provider: Arc<dyn TableProvider>) {
        self.tables.insert(
            name.to_ascii_lowercase(),
            RegisteredTable {
                path: PathBuf::from("(memory)"),
                provider,
                in_memory: true,
            },
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

/// A fully materialized table, e.g. the result of `CREATE TABLE ... AS`.
pub struct MemTable {
    schema: SchemaRef,
    rows: Arc<Vec<Row>>,
}

impl MemTable {
    #[must_use]
    pub fn new(schema: SchemaRef, rows: Vec<Row>) -> Self {
        Self {
            schema,
            rows: Arc::new(rows),
        }
    }
}

impl TableProvider for MemTable {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn nested_fields(&self, column: usize) -> Option<Vec<Field>> {
        let parsed: Vec<serde_json::Value> = self
            .rows
            .iter()
            .filter_map(|row| match row.0.get(column) {
                Some(Value::Utf8(s)) if s.trim_start().starts_with('{') => {
                    serde_json::from_str(s).ok()
                }
                _ => None,
            })
            .collect();
        infer_object_fields(parsed.iter().filter_map(serde_json::Value::as_object))
    }

    fn scan(
        &self,
        projection: Option<&[usize]>,
        _filters: &[FilterExpr],
    ) -> Result<Box<dyn BatchStream>, VoirError> {
        let schema = match projection {
            None => self.schema.clone(),
            Some(indices) => Arc::new(voir_types::Schema::new(
                indices
                    .iter()
                    .map(|&i| self.schema.fields[i].clone())
                    .collect(),
            )),
        };
        Ok(Box::new(MemStream {
            rows: self.rows.clone(),
            schema,
            projection: projection.map(<[usize]>::to_vec),
            pos: 0,
        }))
    }
}

struct MemStream {
    rows: Arc<Vec<Row>>,
    schema: SchemaRef,
    projection: Option<Vec<usize>>,
    pos: usize,
}

impl BatchStream for MemStream {
    fn next_batch(&mut self) -> Result<Option<Batch>, VoirError> {
        let end = (self.pos + Batch::MAX_ROWS).min(self.rows.len());
        if self.pos >= end {
            return Ok(None);
        }
        let rows = self.rows[self.pos..end]
            .iter()
            .map(|row| match &self.projection {
                None => row.clone(),
                Some(indices) => Row(indices.iter().map(|&i| row.0[i].clone()).collect()),
            })
            .collect();
        self.pos = end;
        Ok(Some(Batch::new(self.schema.clone(), rows)))
    }
}

/// Infers the fields of a column holding JSON objects: the union of keys in
/// first-seen order, each typed by widening over the values seen. `None` if
/// no object was seen. Shared by every provider whose nested values are JSON.
pub fn infer_object_fields<'a>(
    objects: impl IntoIterator<Item = &'a serde_json::Map<String, serde_json::Value>>,
) -> Option<Vec<Field>> {
    use serde_json::Value as Json;
    let mut fields: Vec<(String, Option<DataType>, bool)> = Vec::new();
    let mut seen = false;
    for obj in objects {
        seen = true;
        for (key, value) in obj {
            let pos = fields
                .iter()
                .position(|(n, _, _)| n == key)
                .unwrap_or_else(|| {
                    fields.push((key.clone(), None, false));
                    fields.len() - 1
                });
            let entry = &mut fields[pos];
            let t = match value {
                Json::Null => {
                    entry.2 = true;
                    continue;
                }
                Json::Bool(_) => DataType::Boolean,
                Json::Number(n) if n.is_i64() => DataType::Int64,
                Json::Number(_) => DataType::Float64,
                _ => DataType::Utf8,
            };
            entry.1 = Some(match entry.1 {
                None => t,
                Some(c) if c == t => c,
                Some(DataType::Int64 | DataType::Float64)
                    if matches!(t, DataType::Int64 | DataType::Float64) =>
                {
                    DataType::Float64
                }
                Some(_) => DataType::Utf8,
            });
        }
    }
    seen.then(|| {
        fields
            .into_iter()
            .map(|(n, t, nullable)| Field::new(n, t.unwrap_or(DataType::Utf8), nullable))
            .collect()
    })
}
