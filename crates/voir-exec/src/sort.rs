use std::cmp::Ordering;

use voir_plan::SortKey as PlanSortKey;
use voir_types::{Batch, Row, SchemaRef, Value, VoirError};

use crate::expr::{EvalFn, compile};
use crate::operator::Operator;

/// `NULL` sorts as greater than everything, matching Postgres's default
/// (`NULLS LAST` ascending, `NULLS FIRST` descending — the latter falls out
/// naturally from negating the comparison for `DESC`).
fn compare_values(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Greater,
        (_, Value::Null) => Ordering::Less,
        (Value::Boolean(x), Value::Boolean(y)) => x.cmp(y),
        (Value::Utf8(x), Value::Utf8(y)) => x.as_ref().cmp(y.as_ref()),
        (Value::Date(x), Value::Date(y)) => x.cmp(y),
        (Value::Timestamp(x), Value::Timestamp(y)) => x.cmp(y),
        _ => a
            .as_f64()
            .ok()
            .zip(b.as_f64().ok())
            .and_then(|(x, y)| x.partial_cmp(&y))
            .unwrap_or(Ordering::Equal),
    }
}

struct SortKey {
    eval: EvalFn,
    desc: bool,
}

pub struct SortOperator {
    input: Box<dyn Operator>,
    schema: SchemaRef,
    keys: Vec<SortKey>,
    buffered: Option<std::vec::IntoIter<Row>>,
}

impl SortOperator {
    /// # Errors
    ///
    /// Returns an error if compiling a sort key expression fails.
    pub fn new(
        input: Box<dyn Operator>,
        schema: SchemaRef,
        keys: &[PlanSortKey],
    ) -> Result<Self, VoirError> {
        let keys = keys
            .iter()
            .map(|k| {
                Ok(SortKey {
                    eval: compile(&k.expr)?,
                    desc: k.desc,
                })
            })
            .collect::<Result<_, VoirError>>()?;
        Ok(Self {
            input,
            schema,
            keys,
            buffered: None,
        })
    }

    fn sort_all(&mut self) -> Result<Vec<Row>, VoirError> {
        let mut rows = Vec::new();
        while let Some(batch) = self.input.next_batch()? {
            rows.extend(batch.rows);
        }
        let mut keyed: Vec<(Vec<Value>, Row)> = rows
            .into_iter()
            .map(|row| {
                let key = self
                    .keys
                    .iter()
                    .map(|k| (k.eval)(&row))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok((key, row))
            })
            .collect::<Result<_, VoirError>>()?;
        let descs: Vec<bool> = self.keys.iter().map(|k| k.desc).collect();
        keyed.sort_by(|(a, _), (b, _)| {
            for (i, desc) in descs.iter().enumerate() {
                let ord = compare_values(&a[i], &b[i]);
                let ord = if *desc { ord.reverse() } else { ord };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            Ordering::Equal
        });
        Ok(keyed.into_iter().map(|(_, row)| row).collect())
    }
}

impl Operator for SortOperator {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn next_batch(&mut self) -> Result<Option<Batch>, VoirError> {
        if self.buffered.is_none() {
            self.buffered = Some(self.sort_all()?.into_iter());
        }
        let iter = self.buffered.as_mut().expect("just initialized");
        let rows: Vec<Row> = iter.by_ref().take(Batch::MAX_ROWS).collect();
        if rows.is_empty() {
            Ok(None)
        } else {
            Ok(Some(Batch::new(self.schema.clone(), rows)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_sorts_last_ascending_first_descending() {
        assert_eq!(
            compare_values(&Value::Null, &Value::Int64(1)),
            Ordering::Greater
        );
        assert_eq!(
            compare_values(&Value::Int64(1), &Value::Null),
            Ordering::Less
        );
        // DESC negates the ascending comparison, so NULL (Greater ascending)
        // becomes Less — i.e. sorts first — matching Postgres's NULLS FIRST
        // default for descending order.
        assert_eq!(
            compare_values(&Value::Null, &Value::Int64(1)).reverse(),
            Ordering::Less
        );
    }

    #[test]
    fn numeric_comparison_crosses_int_and_float() {
        assert_eq!(
            compare_values(&Value::Int64(1), &Value::Float64(1.5)),
            Ordering::Less
        );
    }
}
