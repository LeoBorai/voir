use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use voir_plan::{AggExpr, AggFunc, PlanExpr};
use voir_types::{Batch, Row, SchemaRef, Value, VoirError};

use crate::expr::{EvalFn, compile};
use crate::operator::Operator;

/// `Value` can't derive `Eq`/`Hash` (it holds an `f64`), but `GroupKey` needs
/// both to key a `HashMap`/`HashSet` — hash by `f64::to_bits()` and treat
/// `Value`'s existing `PartialEq` as total. `NaN != NaN` under that impl,
/// which would put every `NaN` group in its own bucket; acceptable, since
/// `NaN` can't come from CSV input today (see plan.md open question on
/// `Decimal`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GroupKey(pub(crate) Vec<Value>);

impl Eq for GroupKey {}

impl Hash for GroupKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for v in &self.0 {
            hash_value(v, state);
        }
    }
}

fn hash_value<H: Hasher>(v: &Value, state: &mut H) {
    std::mem::discriminant(v).hash(state);
    match v {
        Value::Null => {}
        Value::Boolean(b) => b.hash(state),
        Value::Int64(i) => i.hash(state),
        Value::Float64(f) => f.to_bits().hash(state),
        Value::Utf8(s) => s.hash(state),
        Value::Date(d) => d.hash(state),
        Value::Timestamp(t) => t.hash(state),
    }
}

struct AccState {
    func: AggFunc,
    distinct_seen: Option<HashSet<GroupKey>>,
    count: i64,
    sum: f64,
    saw_value: bool,
    extreme: Option<Value>,
}

impl AccState {
    fn new(agg: &AggExpr) -> Self {
        Self {
            func: agg.func,
            distinct_seen: agg.distinct.then(HashSet::new),
            count: 0,
            sum: 0.0,
            saw_value: false,
            extreme: None,
        }
    }

    fn update(&mut self, arg: Option<Value>) -> Result<(), VoirError> {
        match self.func {
            AggFunc::Count => match arg {
                None => self.count += 1, // COUNT(*): every row counts
                Some(v) => {
                    if !v.is_null() && self.admit(&v) {
                        self.count += 1;
                    }
                }
            },
            AggFunc::Sum | AggFunc::Avg => {
                let v = arg.expect("SUM/AVG always bind an argument");
                if !v.is_null() && self.admit(&v) {
                    self.sum += v.as_f64()?;
                    self.count += 1;
                    self.saw_value = true;
                }
            }
            AggFunc::Min | AggFunc::Max => {
                let v = arg.expect("MIN/MAX always bind an argument");
                if v.is_null() {
                    return Ok(());
                }
                let keep = match &self.extreme {
                    None => true,
                    Some(cur) => {
                        let cmp = if self.func == AggFunc::Min {
                            v.sql_lt(cur)?
                        } else {
                            v.sql_gt(cur)?
                        };
                        matches!(cmp, Value::Boolean(true))
                    }
                };
                if keep {
                    self.extreme = Some(v);
                }
            }
        }
        Ok(())
    }

    /// For `DISTINCT` aggregates, only the first occurrence of a value
    /// counts. Returns `true` when `v` should be admitted (non-distinct
    /// aggregates always admit).
    fn admit(&mut self, v: &Value) -> bool {
        match &mut self.distinct_seen {
            None => true,
            Some(seen) => seen.insert(GroupKey(vec![v.clone()])),
        }
    }

    fn finish(self) -> Value {
        match self.func {
            AggFunc::Count => Value::Int64(self.count),
            AggFunc::Sum => {
                if self.saw_value {
                    Value::Float64(self.sum)
                } else {
                    Value::Null
                }
            }
            AggFunc::Avg => {
                if self.count == 0 {
                    Value::Null
                } else {
                    #[allow(clippy::cast_precision_loss)]
                    let count = self.count as f64;
                    Value::Float64(self.sum / count)
                }
            }
            AggFunc::Min | AggFunc::Max => self.extreme.unwrap_or(Value::Null),
        }
    }
}

pub struct AggregateOperator {
    input: Box<dyn Operator>,
    schema: SchemaRef,
    group_by: Vec<EvalFn>,
    aggregates: Vec<AggExpr>,
    arg_evals: Vec<Option<EvalFn>>,
    done: bool,
}

impl AggregateOperator {
    /// # Errors
    ///
    /// Returns an error if compiling a group-by key or aggregate argument
    /// expression fails.
    pub fn new(
        input: Box<dyn Operator>,
        schema: SchemaRef,
        group_by: &[PlanExpr],
        aggregates: Vec<AggExpr>,
    ) -> Result<Self, VoirError> {
        let arg_evals = aggregates
            .iter()
            .map(|a| a.arg.as_ref().map(compile).transpose())
            .collect::<Result<_, _>>()?;
        let group_by = group_by.iter().map(compile).collect::<Result<_, _>>()?;
        Ok(Self {
            input,
            schema,
            group_by,
            aggregates,
            arg_evals,
            done: false,
        })
    }
}

impl Operator for AggregateOperator {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn next_batch(&mut self) -> Result<Option<Batch>, VoirError> {
        if self.done {
            return Ok(None);
        }
        self.done = true;

        let mut groups: HashMap<GroupKey, Vec<AccState>> = HashMap::new();
        if self.group_by.is_empty() {
            // A whole-table aggregate always produces exactly one row, even
            // over zero input rows (COUNT -> 0, SUM/AVG/MIN/MAX -> NULL).
            groups
                .entry(GroupKey(Vec::new()))
                .or_insert_with(|| self.aggregates.iter().map(AccState::new).collect());
        }

        while let Some(batch) = self.input.next_batch()? {
            for row in &batch.rows {
                let key_values: Vec<Value> = self
                    .group_by
                    .iter()
                    .map(|eval| eval(row))
                    .collect::<Result<_, _>>()?;
                let accs = groups
                    .entry(GroupKey(key_values))
                    .or_insert_with(|| self.aggregates.iter().map(AccState::new).collect());
                for (acc, eval) in accs.iter_mut().zip(&self.arg_evals) {
                    let arg = eval.as_ref().map(|e| e(row)).transpose()?;
                    acc.update(arg)?;
                }
            }
        }

        if groups.is_empty() {
            return Ok(None);
        }

        let rows: Vec<Row> = groups
            .into_iter()
            .map(|(key, accs)| {
                let mut values = key.0;
                values.extend(accs.into_iter().map(AccState::finish));
                Row(values)
            })
            .collect();
        Ok(Some(Batch::new(self.schema.clone(), rows)))
    }
}
