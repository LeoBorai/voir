use voir_plan::PlanExpr;
use voir_sql::{BinaryOperator, UnaryOperator};
use voir_types::{Row, Value, VoirError};

use crate::scalar::eval_scalar;

/// Compiling an expression into a closure tree once at plan time — instead
/// of matching on the AST per row — moves type dispatch and column lookup
/// out of the hot loop. Highest-leverage perf decision in the engine.
pub type EvalFn = Box<dyn Fn(&Row) -> Result<Value, VoirError> + Send>;

#[must_use]
pub fn compile(expr: &PlanExpr) -> EvalFn {
    match expr {
        PlanExpr::Column { index, .. } => {
            let index = *index;
            Box::new(move |row: &Row| Ok(row.0[index].clone()))
        }
        PlanExpr::Literal(value) => {
            let value = value.clone();
            Box::new(move |_row: &Row| Ok(value.clone()))
        }
        PlanExpr::UnaryOp { op, expr } => {
            let inner = compile(expr);
            match op {
                UnaryOperator::Not => Box::new(move |row| inner(row)?.sql_not()),
                UnaryOperator::Negate => Box::new(move |row| inner(row)?.sql_negate()),
                UnaryOperator::Plus => inner,
            }
        }
        PlanExpr::BinaryOp { left, op, right } => {
            let l = compile(left);
            let r = compile(right);
            let op = *op;
            Box::new(move |row| {
                let lv = l(row)?;
                let rv = r(row)?;
                apply_binary(op, &lv, &rv)
            })
        }
        PlanExpr::Function { func, args } => {
            let arg_evals: Vec<EvalFn> = args.iter().map(compile).collect();
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
            let operand_eval = operand.as_deref().map(compile);
            let branch_evals: Vec<(EvalFn, EvalFn)> = branches
                .iter()
                .map(|(when, then)| (compile(when), compile(then)))
                .collect();
            let else_eval = else_expr.as_deref().map(compile);
            Box::new(move |row| {
                let operand_val = operand_eval.as_ref().map(|e| e(row)).transpose()?;
                for (when_eval, then_eval) in &branch_evals {
                    let when_val = when_eval(row)?;
                    let matched = match &operand_val {
                        Some(operand_val) => {
                            matches!(operand_val.sql_eq(&when_val)?, Value::Boolean(true))
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
            let inner = compile(expr);
            let target = *target;
            Box::new(move |row| inner(row)?.cast_to(target))
        }
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
