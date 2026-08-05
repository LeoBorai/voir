use std::collections::{HashMap, HashSet};

use voir_sql::SetOperator;
use voir_types::{Batch, Row, SchemaRef, VoirError};

use crate::aggregate::GroupKey;
use crate::operator::Operator;

/// `EXCEPT`/`INTERSECT` (`ALL` and distinct) and `UNION ALL`. Plain `UNION`
/// (distinct) never reaches this operator — the binder desugars it into
/// `Distinct(SetOp { op: Union, all: true, .. })`, reusing the existing
/// streaming-dedup operator instead of a bespoke one.
pub struct SetOpOperator {
    schema: SchemaRef,
    left: Box<dyn Operator>,
    right: Box<dyn Operator>,
    op: SetOperator,
    all: bool,
    rows: Option<std::vec::IntoIter<Row>>,
}

impl SetOpOperator {
    #[must_use]
    pub fn new(
        left: Box<dyn Operator>,
        right: Box<dyn Operator>,
        op: SetOperator,
        all: bool,
        schema: SchemaRef,
    ) -> Self {
        Self {
            schema,
            left,
            right,
            op,
            all,
            rows: None,
        }
    }

    fn materialize(&mut self) -> Result<Vec<Row>, VoirError> {
        if matches!(self.op, SetOperator::Union) {
            let mut rows = Vec::new();
            while let Some(batch) = self.left.next_batch()? {
                rows.extend(batch.rows);
            }
            while let Some(batch) = self.right.next_batch()? {
                rows.extend(batch.rows);
            }
            return Ok(rows);
        }

        let mut right_counts: HashMap<GroupKey, i64> = HashMap::new();
        while let Some(batch) = self.right.next_batch()? {
            for row in batch.rows {
                *right_counts.entry(GroupKey(row.0)).or_insert(0) += 1;
            }
        }
        let mut remaining_right = if self.all {
            right_counts.clone()
        } else {
            HashMap::new()
        };
        let mut seen: HashSet<GroupKey> = HashSet::new();

        let mut out = Vec::new();
        while let Some(batch) = self.left.next_batch()? {
            for row in batch.rows {
                let key = GroupKey(row.0.clone());
                let take = if self.all {
                    // A remaining right occurrence "cancels" one left
                    // occurrence: EXCEPT ALL drops it, INTERSECT ALL keeps
                    // it — either way each right row can cancel/match only
                    // one left row, hence the decrement.
                    let cancel = remaining_right.get(&key).copied().unwrap_or(0) > 0;
                    if cancel {
                        *remaining_right.get_mut(&key).expect("checked above") -= 1;
                    }
                    match self.op {
                        SetOperator::Except => !cancel,
                        SetOperator::Intersect => cancel,
                        SetOperator::Union => unreachable!("handled above"),
                    }
                } else {
                    let in_right = right_counts.get(&key).copied().unwrap_or(0) > 0;
                    let keep = match self.op {
                        SetOperator::Except => !in_right,
                        SetOperator::Intersect => in_right,
                        SetOperator::Union => unreachable!("handled above"),
                    };
                    keep && seen.insert(key)
                };
                if take {
                    out.push(row);
                }
            }
        }
        Ok(out)
    }
}

impl Operator for SetOpOperator {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn next_batch(&mut self) -> Result<Option<Batch>, VoirError> {
        if self.rows.is_none() {
            self.rows = Some(self.materialize()?.into_iter());
        }
        let iter = self.rows.as_mut().expect("just initialized");
        let rows: Vec<Row> = iter.by_ref().take(Batch::MAX_ROWS).collect();
        if rows.is_empty() {
            Ok(None)
        } else {
            Ok(Some(Batch::new(self.schema.clone(), rows)))
        }
    }
}
