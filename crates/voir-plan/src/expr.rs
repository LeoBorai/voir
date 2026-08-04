use std::fmt;

use voir_sql::{BinaryOperator, UnaryOperator};
use voir_types::{DataType, Schema, Value};

/// A bound expression: column names have been resolved to positional indices
/// against a known input schema.
#[derive(Debug, Clone, PartialEq)]
pub enum PlanExpr {
    Column {
        index: usize,
        name: String,
    },
    Literal(Value),
    BinaryOp {
        left: Box<PlanExpr>,
        op: BinaryOperator,
        right: Box<PlanExpr>,
    },
    UnaryOp {
        op: UnaryOperator,
        expr: Box<PlanExpr>,
    },
}

impl PlanExpr {
    #[must_use]
    pub fn infer_type(&self, schema: &Schema) -> DataType {
        match self {
            PlanExpr::Column { index, .. } => schema.fields[*index].data_type,
            PlanExpr::Literal(v) => v.data_type(),
            PlanExpr::UnaryOp { op, expr } => match op {
                UnaryOperator::Not => DataType::Boolean,
                UnaryOperator::Negate | UnaryOperator::Plus => expr.infer_type(schema),
            },
            PlanExpr::BinaryOp { left, op, right } => match op {
                BinaryOperator::And
                | BinaryOperator::Or
                | BinaryOperator::Eq
                | BinaryOperator::NotEq
                | BinaryOperator::Lt
                | BinaryOperator::LtEq
                | BinaryOperator::Gt
                | BinaryOperator::GtEq => DataType::Boolean,
                BinaryOperator::Plus
                | BinaryOperator::Minus
                | BinaryOperator::Multiply
                | BinaryOperator::Divide
                | BinaryOperator::Modulo => {
                    let lt = left.infer_type(schema);
                    let rt = right.infer_type(schema);
                    if lt == DataType::Float64 || rt == DataType::Float64 {
                        DataType::Float64
                    } else {
                        DataType::Int64
                    }
                }
            },
        }
    }
}

impl fmt::Display for PlanExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanExpr::Column { name, .. } => write!(f, "{name}"),
            PlanExpr::Literal(v) => write!(f, "{v:?}"),
            PlanExpr::BinaryOp { left, op, right } => write!(f, "({left} {op:?} {right})"),
            PlanExpr::UnaryOp { op, expr } => write!(f, "({op:?} {expr})"),
        }
    }
}
