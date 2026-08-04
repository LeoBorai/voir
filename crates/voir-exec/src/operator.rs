use voir_catalog::BatchStream;
use voir_plan::LogicalPlan;
use voir_types::{Batch, Row, SchemaRef, Value, VoirError};

use crate::aggregate::AggregateOperator;
use crate::distinct::DistinctOperator;
use crate::expr::{EvalFn, compile};
use crate::sort::SortOperator;

pub trait Operator: Send {
    fn schema(&self) -> SchemaRef;

    /// # Errors
    ///
    /// Returns an error if the underlying data cannot be read or evaluated.
    fn next_batch(&mut self) -> Result<Option<Batch>, VoirError>;
}

pub struct ScanOperator {
    schema: SchemaRef,
    stream: Box<dyn BatchStream>,
}

impl Operator for ScanOperator {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn next_batch(&mut self) -> Result<Option<Batch>, VoirError> {
        self.stream.next_batch()
    }
}

pub struct FilterOperator {
    input: Box<dyn Operator>,
    predicate: EvalFn,
}

impl Operator for FilterOperator {
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
                // NULL is not false, but WHERE filters the same way: only
                // an exact `true` keeps the row.
                if let Value::Boolean(true) = (self.predicate)(&row)? {
                    rows.push(row);
                }
            }
            if !rows.is_empty() {
                return Ok(Some(Batch::new(schema, rows)));
            }
        }
    }
}

pub struct ProjectOperator {
    input: Box<dyn Operator>,
    schema: SchemaRef,
    evaluators: Vec<EvalFn>,
}

impl Operator for ProjectOperator {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn next_batch(&mut self) -> Result<Option<Batch>, VoirError> {
        let Some(batch) = self.input.next_batch()? else {
            return Ok(None);
        };
        let mut rows = Vec::with_capacity(batch.rows.len());
        for row in &batch.rows {
            let mut values = Vec::with_capacity(self.evaluators.len());
            for eval in &self.evaluators {
                values.push(eval(row)?);
            }
            rows.push(Row(values));
        }
        Ok(Some(Batch::new(self.schema.clone(), rows)))
    }
}

pub struct LimitOperator {
    input: Box<dyn Operator>,
    skip: usize,
    /// `None` means unbounded — `OFFSET` with no `LIMIT`.
    remaining: Option<usize>,
}

impl Operator for LimitOperator {
    fn schema(&self) -> SchemaRef {
        self.input.schema()
    }

    fn next_batch(&mut self) -> Result<Option<Batch>, VoirError> {
        if self.remaining == Some(0) {
            return Ok(None);
        }
        loop {
            let Some(mut batch) = self.input.next_batch()? else {
                return Ok(None);
            };
            if self.skip > 0 {
                let drop = self.skip.min(batch.rows.len());
                batch.rows.drain(..drop);
                self.skip -= drop;
                if batch.rows.is_empty() {
                    continue;
                }
            }
            if let Some(remaining) = self.remaining {
                if batch.rows.len() > remaining {
                    batch.rows.truncate(remaining);
                }
                self.remaining = Some(remaining - batch.rows.len());
            }
            return Ok(Some(batch));
        }
    }
}

/// # Errors
///
/// Returns an error if any operator in the plan fails to construct (e.g. the
/// scanned source cannot be opened).
pub fn build(plan: &LogicalPlan) -> Result<Box<dyn Operator>, VoirError> {
    match plan {
        LogicalPlan::Scan {
            source,
            projection,
            filters,
            ..
        } => {
            let stream = source.scan(projection.as_deref(), filters)?;
            Ok(Box::new(ScanOperator {
                schema: plan.schema(),
                stream,
            }))
        }
        LogicalPlan::Filter { input, predicate } => {
            let input_op = build(input)?;
            Ok(Box::new(FilterOperator {
                input: input_op,
                predicate: compile(predicate),
            }))
        }
        LogicalPlan::Aggregate {
            input,
            group_by,
            aggregates,
            ..
        } => {
            let input_op = build(input)?;
            Ok(Box::new(AggregateOperator::new(
                input_op,
                plan.schema(),
                group_by,
                aggregates.clone(),
            )))
        }
        LogicalPlan::Sort { input, keys } => {
            let input_op = build(input)?;
            Ok(Box::new(SortOperator::new(input_op, plan.schema(), keys)))
        }
        LogicalPlan::Projection { input, exprs } => {
            let input_op = build(input)?;
            let evaluators = exprs.iter().map(|(e, _)| compile(e)).collect();
            Ok(Box::new(ProjectOperator {
                input: input_op,
                schema: plan.schema(),
                evaluators,
            }))
        }
        LogicalPlan::Distinct { input } => {
            let input_op = build(input)?;
            Ok(Box::new(DistinctOperator::new(input_op)))
        }
        LogicalPlan::Limit { input, skip, fetch } => {
            let input_op = build(input)?;
            Ok(Box::new(LimitOperator {
                input: input_op,
                skip: *skip,
                remaining: *fetch,
            }))
        }
    }
}
