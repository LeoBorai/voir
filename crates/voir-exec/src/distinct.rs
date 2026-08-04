use std::collections::HashSet;

use voir_types::{Batch, SchemaRef, VoirError};

use crate::aggregate::GroupKey;
use crate::operator::Operator;

/// Streaming dedup: memory grows with the number of distinct rows seen, not
/// with total input size, so it never needs to buffer the whole input like
/// `Sort` does.
pub struct DistinctOperator {
    input: Box<dyn Operator>,
    seen: HashSet<GroupKey>,
}

impl DistinctOperator {
    pub fn new(input: Box<dyn Operator>) -> Self {
        Self {
            input,
            seen: HashSet::new(),
        }
    }
}

impl Operator for DistinctOperator {
    fn schema(&self) -> SchemaRef {
        self.input.schema()
    }

    fn next_batch(&mut self) -> Result<Option<Batch>, VoirError> {
        loop {
            let Some(batch) = self.input.next_batch()? else {
                return Ok(None);
            };
            let schema = batch.schema.clone();
            let mut rows = Vec::new();
            for row in batch.rows {
                if self.seen.insert(GroupKey(row.0.clone())) {
                    rows.push(row);
                }
            }
            if !rows.is_empty() {
                return Ok(Some(Batch::new(schema, rows)));
            }
        }
    }
}
