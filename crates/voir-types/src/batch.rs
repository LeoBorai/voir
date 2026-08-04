use crate::schema::SchemaRef;
use crate::value::Value;

#[derive(Debug, Clone)]
pub struct Row(pub Vec<Value>);

#[derive(Debug, Clone)]
pub struct Batch {
    pub schema: SchemaRef,
    pub rows: Vec<Row>,
}

impl Batch {
    /// Row-at-a-time execution loses most of its time to per-call overhead;
    /// batching at this size recovers it while keeping operators simple.
    pub const MAX_ROWS: usize = 1024;

    pub fn new(schema: SchemaRef, rows: Vec<Row>) -> Self {
        Self { schema, rows }
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}
