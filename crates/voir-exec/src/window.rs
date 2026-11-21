use std::cmp::Ordering;

use voir_plan::WindowExpr;
use voir_types::{Batch, Row, SchemaRef, Value, VoirError};

use crate::expr::{EvalFn, compile};
use crate::operator::Operator;
use crate::sort::compare_values;

struct CompiledWindow {
    partition_by: Vec<EvalFn>,
    order_by: Vec<(EvalFn, bool)>,
}

/// Buffers its whole input, then appends one `row_number()` column per window
/// expression. Rows keep their input order; only the numbers depend on the
/// window's `PARTITION BY`/`ORDER BY`. Ties in `ORDER BY` are numbered in
/// input order.
pub struct WindowOperator {
    input: Box<dyn Operator>,
    schema: SchemaRef,
    windows: Vec<CompiledWindow>,
    buffered: Option<std::vec::IntoIter<Row>>,
}

impl WindowOperator {
    pub fn new(
        input: Box<dyn Operator>,
        schema: SchemaRef,
        windows: &[WindowExpr],
    ) -> Result<Self, VoirError> {
        let windows = windows
            .iter()
            .map(|w| {
                Ok(CompiledWindow {
                    partition_by: w
                        .partition_by
                        .iter()
                        .map(compile)
                        .collect::<Result<_, VoirError>>()?,
                    order_by: w
                        .order_by
                        .iter()
                        .map(|k| Ok((compile(&k.expr)?, k.desc)))
                        .collect::<Result<_, VoirError>>()?,
                })
            })
            .collect::<Result<_, VoirError>>()?;
        Ok(Self {
            input,
            schema,
            windows,
            buffered: None,
        })
    }

    fn compute(&mut self) -> Result<Vec<Row>, VoirError> {
        let mut rows: Vec<Row> = Vec::new();
        while let Some(batch) = self.input.next_batch()? {
            rows.extend(batch.rows);
        }
        let mut numbers: Vec<Vec<i64>> = Vec::with_capacity(self.windows.len());
        for window in &self.windows {
            numbers.push(row_numbers(window, &rows)?);
        }
        for (i, row) in rows.iter_mut().enumerate() {
            for column in &numbers {
                row.0.push(Value::Int64(column[i]));
            }
        }
        Ok(rows)
    }
}

fn row_numbers(window: &CompiledWindow, rows: &[Row]) -> Result<Vec<i64>, VoirError> {
    let mut keyed: Vec<(usize, Vec<Value>, Vec<Value>)> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let part = window
                .partition_by
                .iter()
                .map(|e| e(row))
                .collect::<Result<Vec<_>, _>>()?;
            let order = window
                .order_by
                .iter()
                .map(|(e, _)| e(row))
                .collect::<Result<Vec<_>, _>>()?;
            Ok((i, part, order))
        })
        .collect::<Result<_, VoirError>>()?;
    // `sort_by` is stable, so ties keep input order.
    keyed.sort_by(|a, b| {
        for (x, y) in a.1.iter().zip(&b.1) {
            let ord = compare_values(x, y);
            if ord != Ordering::Equal {
                return ord;
            }
        }
        for ((x, y), (_, desc)) in a.2.iter().zip(&b.2).zip(&window.order_by) {
            let ord = compare_values(x, y);
            let ord = if *desc { ord.reverse() } else { ord };
            if ord != Ordering::Equal {
                return ord;
            }
        }
        Ordering::Equal
    });
    let mut out = vec![0i64; rows.len()];
    let mut counter = 0i64;
    let mut prev: Option<&Vec<Value>> = None;
    for (i, part, _) in &keyed {
        let same = prev.is_some_and(|p| {
            p.iter()
                .zip(part)
                .all(|(x, y)| compare_values(x, y) == Ordering::Equal)
        });
        counter = if same { counter + 1 } else { 1 };
        out[*i] = counter;
        prev = Some(part);
    }
    Ok(out)
}

impl Operator for WindowOperator {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn next_batch(&mut self) -> Result<Option<Batch>, VoirError> {
        if self.buffered.is_none() {
            self.buffered = Some(self.compute()?.into_iter());
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
