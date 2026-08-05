use voir_plan::{LogicalPlan, PlanExpr};
use voir_sql::{BinaryOperator, UnaryOperator};
use voir_types::{Row, Value, VoirError};

use crate::scalar::eval_scalar;

/// Compiling an expression into a closure tree once at plan time — instead
/// of matching on the AST per row — moves type dispatch and column lookup
/// out of the hot loop. Highest-leverage perf decision in the engine.
pub type EvalFn = Box<dyn Fn(&Row) -> Result<Value, VoirError> + Send>;

/// # Errors
///
/// Returns an error if compiling fails — the only way that happens today is
/// a subquery expression (`ScalarSubquery`/`InSubquery`/`Exists`), which
/// runs its nested plan eagerly right here (once, not per row: correct only
/// because `voir` supports uncorrelated subqueries) and can fail the same
/// way running any query can.
#[allow(clippy::too_many_lines)]
pub fn compile(expr: &PlanExpr) -> Result<EvalFn, VoirError> {
    Ok(match expr {
        PlanExpr::Column { index, .. } => {
            let index = *index;
            Box::new(move |row: &Row| Ok(row.0[index].clone()))
        }
        PlanExpr::Literal(value) => {
            let value = value.clone();
            Box::new(move |_row: &Row| Ok(value.clone()))
        }
        PlanExpr::UnaryOp { op, expr } => {
            let inner = compile(expr)?;
            match op {
                UnaryOperator::Not => Box::new(move |row| inner(row)?.sql_not()),
                UnaryOperator::Negate => Box::new(move |row| inner(row)?.sql_negate()),
                UnaryOperator::Plus => inner,
            }
        }
        PlanExpr::BinaryOp { left, op, right } => {
            let l = compile(left)?;
            let r = compile(right)?;
            let op = *op;
            Box::new(move |row| {
                let lv = l(row)?;
                let rv = r(row)?;
                apply_binary(op, &lv, &rv)
            })
        }
        PlanExpr::Function { func, args } => {
            let arg_evals: Vec<EvalFn> = args.iter().map(compile).collect::<Result<_, _>>()?;
            let func = *func;
            Box::new(move |row| {
                let values = arg_evals
                    .iter()
                    .map(|eval| eval(row))
                    .collect::<Result<Vec<_>, _>>()?;
                eval_scalar(func, &values)
            })
        }
        PlanExpr::Case {
            operand,
            branches,
            else_expr,
        } => {
            let operand_compiled = operand.as_deref().map(compile).transpose()?;
            let branch_evals: Vec<(EvalFn, EvalFn)> = branches
                .iter()
                .map(|(when, then)| Ok((compile(when)?, compile(then)?)))
                .collect::<Result<_, VoirError>>()?;
            let else_eval = else_expr.as_deref().map(compile).transpose()?;
            Box::new(move |row| {
                let subject = operand_compiled.as_ref().map(|e| e(row)).transpose()?;
                for (when_eval, then_eval) in &branch_evals {
                    let when_val = when_eval(row)?;
                    let matched = match &subject {
                        Some(subject) => {
                            matches!(subject.sql_eq(&when_val)?, Value::Boolean(true))
                        }
                        None => matches!(when_val, Value::Boolean(true)),
                    };
                    if matched {
                        return then_eval(row);
                    }
                }
                match &else_eval {
                    Some(eval) => eval(row),
                    None => Ok(Value::Null),
                }
            })
        }
        PlanExpr::Cast { expr, target } => {
            let inner = compile(expr)?;
            let target = *target;
            Box::new(move |row| inner(row)?.cast_to(target))
        }
        PlanExpr::InList { expr, list } => {
            let inner = compile(expr)?;
            let list_evals: Vec<EvalFn> = list.iter().map(compile).collect::<Result<_, _>>()?;
            Box::new(move |row| {
                let v = inner(row)?;
                let mut candidates = Vec::with_capacity(list_evals.len());
                for eval in &list_evals {
                    candidates.push(eval(row)?);
                }
                eval_in(&v, &candidates)
            })
        }
        PlanExpr::InSubquery { expr, plan } => {
            let inner = compile(expr)?;
            let candidates = drain_column(plan)?;
            Box::new(move |row| {
                let v = inner(row)?;
                eval_in(&v, &candidates)
            })
        }
        PlanExpr::Exists(plan) => {
            let exists = drain_exists(plan)?;
            Box::new(move |_row: &Row| Ok(Value::Boolean(exists)))
        }
        PlanExpr::ScalarSubquery(plan) => {
            let value = drain_scalar(plan)?;
            Box::new(move |_row: &Row| Ok(value.clone()))
        }
    })
}

/// `x IN (v1, v2, ...)` under three-valued logic: `true` on any match;
/// `NULL` if no match but `x` or some candidate was `NULL` (their equality
/// is itself unknown, not false); `false` only once every comparison is
/// known to fail — including the empty-list/empty-subquery case, which is
/// unconditionally `false` even for a `NULL` `x`.
fn eval_in(v: &Value, candidates: &[Value]) -> Result<Value, VoirError> {
    let mut null_seen = false;
    for candidate in candidates {
        match v.sql_eq(candidate)? {
            Value::Boolean(true) => return Ok(Value::Boolean(true)),
            Value::Null => null_seen = true,
            _ => {}
        }
    }
    Ok(if null_seen {
        Value::Null
    } else {
        Value::Boolean(false)
    })
}

/// Eagerly drains a bound, uncorrelated subquery plan into its single output
/// column — the building block for `IN (subquery)`, `EXISTS`, and scalar
/// subqueries. Run once at compile time, not per row: correct only because
/// `voir` doesn't support correlated subqueries (nothing in `plan` can
/// depend on the enclosing query's current row).
fn drain_column(plan: &LogicalPlan) -> Result<Vec<Value>, VoirError> {
    let mut op = crate::operator::build(plan)?;
    let mut values = Vec::new();
    while let Some(batch) = op.next_batch()? {
        for row in batch.rows {
            let mut cols = row.0.into_iter();
            let value = cols
                .next()
                .ok_or_else(|| VoirError::Exec("subquery must return exactly one column".into()))?;
            if cols.next().is_some() {
                return Err(VoirError::Exec(
                    "subquery must return exactly one column".into(),
                ));
            }
            values.push(value);
        }
    }
    Ok(values)
}

fn drain_exists(plan: &LogicalPlan) -> Result<bool, VoirError> {
    let mut op = crate::operator::build(plan)?;
    Ok(op.next_batch()?.is_some())
}

fn drain_scalar(plan: &LogicalPlan) -> Result<Value, VoirError> {
    let mut values = drain_column(plan)?;
    match values.len() {
        0 => Ok(Value::Null),
        1 => Ok(values.pop().expect("length checked above")),
        _ => Err(VoirError::Exec(
            "more than one row returned by a subquery used as an expression".into(),
        )),
    }
}

fn apply_binary(op: BinaryOperator, l: &Value, r: &Value) -> Result<Value, VoirError> {
    match op {
        BinaryOperator::Or => l.sql_or(r),
        BinaryOperator::And => l.sql_and(r),
        BinaryOperator::Eq => l.sql_eq(r),
        BinaryOperator::NotEq => l.sql_not_eq(r),
        BinaryOperator::Lt => l.sql_lt(r),
        BinaryOperator::LtEq => l.sql_lt_eq(r),
        BinaryOperator::Gt => l.sql_gt(r),
        BinaryOperator::GtEq => l.sql_gt_eq(r),
        BinaryOperator::Plus => l.sql_add(r),
        BinaryOperator::Minus => l.sql_sub(r),
        BinaryOperator::Multiply => l.sql_mul(r),
        BinaryOperator::Divide => l.sql_div(r),
        BinaryOperator::Modulo => l.sql_rem(r),
    }
}
