use voir_types::{DataType, Schema};

use crate::expr::PlanExpr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggFunc {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

impl AggFunc {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::Sum => "sum",
            Self::Avg => "avg",
            Self::Min => "min",
            Self::Max => "max",
        }
    }
}

/// Maps a lowercase function name to an `AggFunc`, if it names one — used
/// both to recognize aggregate calls during binding and to reject them
/// outside an aggregate context (`WHERE`, scalar expressions).
#[must_use]
pub fn agg_func_from_name(name: &str) -> Option<AggFunc> {
    match name {
        "count" => Some(AggFunc::Count),
        "sum" => Some(AggFunc::Sum),
        "avg" => Some(AggFunc::Avg),
        "min" => Some(AggFunc::Min),
        "max" => Some(AggFunc::Max),
        _ => None,
    }
}

/// A single aggregate computed by a `LogicalPlan::Aggregate` — `arg` is
/// `None` only for `COUNT(*)`. `alias` names this aggregate's column in the
/// operator's output row.
#[derive(Debug, Clone, PartialEq)]
pub struct AggExpr {
    pub func: AggFunc,
    pub arg: Option<PlanExpr>,
    pub distinct: bool,
    pub alias: String,
}

impl AggExpr {
    #[must_use]
    pub fn result_type(&self, input_schema: &Schema) -> DataType {
        match self.func {
            AggFunc::Count => DataType::Int64,
            AggFunc::Sum | AggFunc::Avg => DataType::Float64,
            AggFunc::Min | AggFunc::Max => self
                .arg
                .as_ref()
                .map_or(DataType::Null, |e| e.infer_type(input_schema)),
        }
    }
}
