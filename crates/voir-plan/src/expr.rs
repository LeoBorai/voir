use std::fmt;

use voir_sql::{BinaryOperator, UnaryOperator};
use voir_types::{DataType, Schema, Value};

/// A bound expression: column names have been resolved to positional indices
/// against a known input schema, and function calls to a known `ScalarFunc`.
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
    Function {
        func: ScalarFunc,
        args: Vec<PlanExpr>,
    },
    Case {
        operand: Option<Box<PlanExpr>>,
        branches: Vec<(PlanExpr, PlanExpr)>,
        else_expr: Option<Box<PlanExpr>>,
    },
    Cast {
        expr: Box<PlanExpr>,
        target: DataType,
    },
}

/// The non-aggregate function library: string, math, and null-handling
/// helpers. Date/time functions are deferred — nothing in `voir` infers a
/// `Date`/`Timestamp` value yet (see plan.md open question 4), so functions
/// over them would be untestable dead code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarFunc {
    Upper,
    Lower,
    Trim,
    Length,
    Concat,
    Abs,
    Round,
    Ceil,
    Floor,
    Power,
    Sqrt,
    Mod,
    Coalesce,
    NullIf,
}

impl ScalarFunc {
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "upper" => Some(Self::Upper),
            "lower" => Some(Self::Lower),
            "trim" => Some(Self::Trim),
            "length" => Some(Self::Length),
            "concat" => Some(Self::Concat),
            "abs" => Some(Self::Abs),
            "round" => Some(Self::Round),
            "ceil" | "ceiling" => Some(Self::Ceil),
            "floor" => Some(Self::Floor),
            "power" | "pow" => Some(Self::Power),
            "sqrt" => Some(Self::Sqrt),
            "mod" => Some(Self::Mod),
            "coalesce" => Some(Self::Coalesce),
            "nullif" => Some(Self::NullIf),
            _ => None,
        }
    }

    /// # Errors
    ///
    /// Returns an error message if `argc` isn't valid for this function.
    pub fn check_arity(self, argc: usize) -> Result<(), String> {
        let name = self.name();
        let ok = match self {
            Self::Upper
            | Self::Lower
            | Self::Trim
            | Self::Length
            | Self::Abs
            | Self::Ceil
            | Self::Floor
            | Self::Sqrt => argc == 1,
            Self::Round => argc == 1 || argc == 2,
            Self::Power | Self::Mod | Self::NullIf => argc == 2,
            Self::Concat | Self::Coalesce => argc >= 1,
        };
        if ok {
            Ok(())
        } else {
            Err(format!("{name}() called with {argc} argument(s)"))
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Upper => "upper",
            Self::Lower => "lower",
            Self::Trim => "trim",
            Self::Length => "length",
            Self::Concat => "concat",
            Self::Abs => "abs",
            Self::Round => "round",
            Self::Ceil => "ceil",
            Self::Floor => "floor",
            Self::Power => "power",
            Self::Sqrt => "sqrt",
            Self::Mod => "mod",
            Self::Coalesce => "coalesce",
            Self::NullIf => "nullif",
        }
    }

    #[must_use]
    fn result_type(self, args: &[PlanExpr], schema: &Schema) -> DataType {
        match self {
            Self::Upper | Self::Lower | Self::Trim | Self::Concat => DataType::Utf8,
            Self::Length => DataType::Int64,
            Self::Abs => args
                .first()
                .map_or(DataType::Float64, |a| a.infer_type(schema)),
            Self::Round | Self::Ceil | Self::Floor | Self::Power | Self::Sqrt | Self::Mod => {
                DataType::Float64
            }
            Self::Coalesce | Self::NullIf => args
                .first()
                .map_or(DataType::Null, |a| a.infer_type(schema)),
        }
    }
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
            PlanExpr::Function { func, args } => func.result_type(args, schema),
            PlanExpr::Case {
                branches,
                else_expr,
                ..
            } => branches
                .first()
                .map(|(_, then)| then)
                .or(else_expr.as_deref())
                .map_or(DataType::Null, |e| e.infer_type(schema)),
            PlanExpr::Cast { target, .. } => *target,
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
            PlanExpr::Function { func, args } => {
                let args_str = args
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "{}({args_str})", func.name())
            }
            PlanExpr::Case { .. } => write!(f, "CASE ... END"),
            PlanExpr::Cast { expr, target } => write!(f, "CAST({expr} AS {target:?})"),
        }
    }
}
