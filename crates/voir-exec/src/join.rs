use std::collections::HashMap;

use voir_plan::{LogicalPlan, PlanExpr};
use voir_sql::{BinaryOperator, JoinType};
use voir_types::{Batch, Row, SchemaRef, Value, VoirError};

use crate::aggregate::GroupKey;
use crate::expr::{EvalFn, compile};
use crate::operator::{Operator, build};

/// # Errors
///
/// Returns an error if either side fails to build.
pub fn build_join(
    left: &LogicalPlan,
    right: &LogicalPlan,
    join_type: JoinType,
    on: Option<&PlanExpr>,
    schema: SchemaRef,
) -> Result<Box<dyn Operator>, VoirError> {
    let left_op = build(left)?;
    let right_op = build(right)?;
    let Some(predicate) = on else {
        return Ok(Box::new(NestedLoopJoinOperator {
            schema,
            left: left_op,
            right: right_op,
            join_type,
            predicate: None,
            rows: None,
        }));
    };

    let left_width = left_op.schema().fields.len();
    let (keys, residual) = extract_equi_keys(predicate, left_width);
    if keys.is_empty() {
        return Ok(Box::new(NestedLoopJoinOperator {
            schema,
            left: left_op,
            right: right_op,
            join_type,
            predicate: Some(compile(predicate)?),
            rows: None,
        }));
    }
    let mut left_keys = Vec::with_capacity(keys.len());
    let mut right_keys = Vec::with_capacity(keys.len());
    for (l, r) in keys {
        left_keys.push(compile(&l)?);
        right_keys.push(compile(&r)?);
    }
    let residual = residual.map(|r| compile(&r)).transpose()?;
    Ok(Box::new(HashJoinOperator {
        schema,
        left: left_op,
        right: right_op,
        join_type,
        left_keys,
        right_keys,
        residual,
        rows: None,
    }))
}

/// Splits a bound `ON` predicate into equi-join key pairs (`Column`-to-
/// `Column`, one side wholly under `left_width`, the other wholly at or
/// above it) and a leftover residual predicate — `AND`-ed conjuncts that
/// don't have that shape, applied as a post-join filter over the
/// concatenated row. Every recognized key pair's right-hand side is
/// rewritten to a column index local to the right input alone (subtracting
/// `left_width`), since it's evaluated against the right row on its own.
fn extract_equi_keys(
    predicate: &PlanExpr,
    left_width: usize,
) -> (Vec<(PlanExpr, PlanExpr)>, Option<PlanExpr>) {
    let mut conjuncts = Vec::new();
    collect_and_conjuncts(predicate, &mut conjuncts);
    let mut keys = Vec::new();
    let mut residual: Option<PlanExpr> = None;
    for conjunct in conjuncts {
        if let PlanExpr::BinaryOp {
            left,
            op: BinaryOperator::Eq,
            right,
        } = &conjunct
        {
            if let (Some(li), Some(ri)) = (column_index(left), column_index(right)) {
                if li < left_width && ri >= left_width {
                    keys.push((local(li), local(ri - left_width)));
                    continue;
                }
                if ri < left_width && li >= left_width {
                    keys.push((local(ri), local(li - left_width)));
                    continue;
                }
            }
        }
        residual = Some(match residual {
            None => conjunct,
            Some(r) => PlanExpr::BinaryOp {
                left: Box::new(r),
                op: BinaryOperator::And,
                right: Box::new(conjunct),
            },
        });
    }
    (keys, residual)
}

fn local(index: usize) -> PlanExpr {
    PlanExpr::Column {
        index,
        name: String::new(),
    }
}

fn column_index(expr: &PlanExpr) -> Option<usize> {
    match expr {
        PlanExpr::Column { index, .. } => Some(*index),
        _ => None,
    }
}

fn collect_and_conjuncts(expr: &PlanExpr, out: &mut Vec<PlanExpr>) {
    if let PlanExpr::BinaryOp {
        left,
        op: BinaryOperator::And,
        right,
    } = expr
    {
        collect_and_conjuncts(left, out);
        collect_and_conjuncts(right, out);
    } else {
        out.push(expr.clone());
    }
}

fn concat_row(left: &Row, right: &Row) -> Row {
    Row(left.0.iter().chain(right.0.iter()).cloned().collect())
}

fn pad_right(left: &Row, right_width: usize) -> Row {
    Row(left
        .0
        .iter()
        .cloned()
        .chain(std::iter::repeat_n(Value::Null, right_width))
        .collect())
}

fn pad_left(left_width: usize, right: &Row) -> Row {
    Row(std::iter::repeat_n(Value::Null, left_width)
        .chain(right.0.iter().cloned())
        .collect())
}

fn keyed(evals: &[EvalFn], row: &Row) -> Result<Option<GroupKey>, VoirError> {
    let mut values = Vec::with_capacity(evals.len());
    for eval in evals {
        let v = eval(row)?;
        // A `NULL` in any key column means "unknown", not "equal to
        // anything" — an equi-join predicate never matches on it, same as
        // plain `=` under three-valued logic.
        if v.is_null() {
            return Ok(None);
        }
        values.push(v);
    }
    Ok(Some(GroupKey(values)))
}

/// Blocking: builds a `HashMap` over the right input, then streams the left
/// input as the probe side, buffering the whole result before serving
/// batches — the same eager-materialize shape as `SortOperator`.
pub struct HashJoinOperator {
    schema: SchemaRef,
    left: Box<dyn Operator>,
    right: Box<dyn Operator>,
    join_type: JoinType,
    left_keys: Vec<EvalFn>,
    right_keys: Vec<EvalFn>,
    residual: Option<EvalFn>,
    rows: Option<std::vec::IntoIter<Row>>,
}

impl HashJoinOperator {
    fn materialize(&mut self) -> Result<Vec<Row>, VoirError> {
        let left_width = self.left.schema().fields.len();
        let right_width = self.right.schema().fields.len();

        let mut right_rows = Vec::new();
        while let Some(batch) = self.right.next_batch()? {
            right_rows.extend(batch.rows);
        }
        let mut build_index: HashMap<GroupKey, Vec<usize>> = HashMap::new();
        for (index, row) in right_rows.iter().enumerate() {
            if let Some(key) = keyed(&self.right_keys, row)? {
                build_index.entry(key).or_default().push(index);
            }
        }
        let mut right_matched = vec![false; right_rows.len()];

        let mut out = Vec::new();
        while let Some(batch) = self.left.next_batch()? {
            for left_row in batch.rows {
                let mut any_match = false;
                if let Some(key) = keyed(&self.left_keys, &left_row)? {
                    if let Some(indices) = build_index.get(&key) {
                        for &index in indices {
                            let combined = concat_row(&left_row, &right_rows[index]);
                            let keep = match &self.residual {
                                Some(f) => matches!(f(&combined)?, Value::Boolean(true)),
                                None => true,
                            };
                            if keep {
                                any_match = true;
                                right_matched[index] = true;
                                out.push(combined);
                            }
                        }
                    }
                }
                if !any_match && matches!(self.join_type, JoinType::Left | JoinType::Full) {
                    out.push(pad_right(&left_row, right_width));
                }
            }
        }

        if matches!(self.join_type, JoinType::Right | JoinType::Full) {
            for (index, row) in right_rows.into_iter().enumerate() {
                if !right_matched[index] {
                    out.push(pad_left(left_width, &row));
                }
            }
        }
        Ok(out)
    }
}

impl Operator for HashJoinOperator {
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

/// Fallback for `CROSS JOIN` (`predicate: None`, always matches) and
/// non-equi `ON` predicates that `extract_equi_keys` couldn't turn into a
/// hash key. O(n*m); correct for every join type, just not fast.
pub struct NestedLoopJoinOperator {
    schema: SchemaRef,
    left: Box<dyn Operator>,
    right: Box<dyn Operator>,
    join_type: JoinType,
    predicate: Option<EvalFn>,
    rows: Option<std::vec::IntoIter<Row>>,
}

impl NestedLoopJoinOperator {
    fn materialize(&mut self) -> Result<Vec<Row>, VoirError> {
        let left_width = self.left.schema().fields.len();
        let right_width = self.right.schema().fields.len();

        let mut left_rows = Vec::new();
        while let Some(batch) = self.left.next_batch()? {
            left_rows.extend(batch.rows);
        }
        let mut right_rows = Vec::new();
        while let Some(batch) = self.right.next_batch()? {
            right_rows.extend(batch.rows);
        }
        let mut right_matched = vec![false; right_rows.len()];

        let mut out = Vec::new();
        for left_row in &left_rows {
            let mut any_match = false;
            for (index, right_row) in right_rows.iter().enumerate() {
                let combined = concat_row(left_row, right_row);
                let keep = match &self.predicate {
                    Some(f) => matches!(f(&combined)?, Value::Boolean(true)),
                    None => true,
                };
                if keep {
                    any_match = true;
                    right_matched[index] = true;
                    out.push(combined);
                }
            }
            if !any_match && matches!(self.join_type, JoinType::Left | JoinType::Full) {
                out.push(pad_right(left_row, right_width));
            }
        }
        if matches!(self.join_type, JoinType::Right | JoinType::Full) {
            for (index, right_row) in right_rows.iter().enumerate() {
                if !right_matched[index] {
                    out.push(pad_left(left_width, right_row));
                }
            }
        }
        Ok(out)
    }
}

impl Operator for NestedLoopJoinOperator {
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
