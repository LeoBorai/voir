use std::fmt;
use std::sync::Arc;

use voir_catalog::{FilterExpr, TableProvider};
use voir_types::{Field, Schema, SchemaRef};

use crate::agg::AggExpr;
use crate::expr::PlanExpr;

pub struct SortKey {
    pub expr: PlanExpr,
    pub desc: bool,
}

pub enum LogicalPlan {
    Scan {
        table_name: String,
        source: Arc<dyn TableProvider>,
        projection: Option<Vec<usize>>,
        filters: Vec<FilterExpr>,
    },
    Filter {
        input: Box<LogicalPlan>,
        predicate: PlanExpr,
    },
    Aggregate {
        input: Box<LogicalPlan>,
        group_by: Vec<PlanExpr>,
        group_by_names: Vec<String>,
        aggregates: Vec<AggExpr>,
    },
    Sort {
        input: Box<LogicalPlan>,
        keys: Vec<SortKey>,
    },
    Projection {
        input: Box<LogicalPlan>,
        exprs: Vec<(PlanExpr, String)>,
    },
    Distinct {
        input: Box<LogicalPlan>,
    },
    Limit {
        input: Box<LogicalPlan>,
        skip: usize,
        fetch: Option<usize>,
    },
}

impl LogicalPlan {
    #[must_use]
    pub fn schema(&self) -> SchemaRef {
        match self {
            LogicalPlan::Scan {
                source, projection, ..
            } => {
                let full = source.schema();
                match projection {
                    None => full,
                    Some(indices) => Arc::new(Schema::new(
                        indices.iter().map(|&i| full.fields[i].clone()).collect(),
                    )),
                }
            }
            LogicalPlan::Filter { input, .. }
            | LogicalPlan::Sort { input, .. }
            | LogicalPlan::Distinct { input }
            | LogicalPlan::Limit { input, .. } => input.schema(),
            LogicalPlan::Aggregate {
                input,
                group_by,
                group_by_names,
                aggregates,
            } => {
                let input_schema = input.schema();
                let mut fields: Vec<Field> = group_by
                    .iter()
                    .zip(group_by_names)
                    .map(|(expr, name)| {
                        Field::new(name.clone(), expr.infer_type(&input_schema), true)
                    })
                    .collect();
                fields.extend(aggregates.iter().map(|agg| {
                    Field::new(agg.alias.clone(), agg.result_type(&input_schema), true)
                }));
                Arc::new(Schema::new(fields))
            }
            LogicalPlan::Projection { input, exprs } => {
                let input_schema = input.schema();
                let fields = exprs
                    .iter()
                    .map(|(expr, name)| {
                        Field::new(name.clone(), expr.infer_type(&input_schema), true)
                    })
                    .collect();
                Arc::new(Schema::new(fields))
            }
        }
    }

    fn fmt_indent(&self, f: &mut fmt::Formatter<'_>, depth: usize) -> fmt::Result {
        let pad = "  ".repeat(depth);
        match self {
            LogicalPlan::Scan {
                table_name,
                projection,
                filters,
                ..
            } => {
                writeln!(
                    f,
                    "{pad}Scan: {table_name} projection={projection:?} filters={filters:?}"
                )
            }
            LogicalPlan::Filter { input, predicate } => {
                writeln!(f, "{pad}Filter: {predicate}")?;
                input.fmt_indent(f, depth + 1)
            }
            LogicalPlan::Aggregate {
                input,
                group_by_names,
                aggregates,
                ..
            } => {
                let aggs: Vec<String> = aggregates
                    .iter()
                    .map(|a| format!("{}={}", a.alias, a.func.name()))
                    .collect();
                writeln!(
                    f,
                    "{pad}Aggregate: group_by=[{}] aggregates=[{}]",
                    group_by_names.join(", "),
                    aggs.join(", ")
                )?;
                input.fmt_indent(f, depth + 1)
            }
            LogicalPlan::Sort { input, keys } => {
                let rendered: Vec<String> = keys
                    .iter()
                    .map(|k| format!("{}{}", k.expr, if k.desc { " DESC" } else { "" }))
                    .collect();
                writeln!(f, "{pad}Sort: {}", rendered.join(", "))?;
                input.fmt_indent(f, depth + 1)
            }
            LogicalPlan::Projection { input, exprs } => {
                let cols: Vec<String> = exprs
                    .iter()
                    .map(|(e, name)| format!("{e} AS {name}"))
                    .collect();
                writeln!(f, "{pad}Projection: {}", cols.join(", "))?;
                input.fmt_indent(f, depth + 1)
            }
            LogicalPlan::Distinct { input } => {
                writeln!(f, "{pad}Distinct")?;
                input.fmt_indent(f, depth + 1)
            }
            LogicalPlan::Limit { input, skip, fetch } => {
                writeln!(f, "{pad}Limit: skip={skip} fetch={fetch:?}")?;
                input.fmt_indent(f, depth + 1)
            }
        }
    }
}

impl fmt::Display for LogicalPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_indent(f, 0)
    }
}
