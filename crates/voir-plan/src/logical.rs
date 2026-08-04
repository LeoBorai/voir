use std::fmt;
use std::sync::Arc;

use voir_catalog::{FilterExpr, TableProvider};
use voir_types::{Field, Schema, SchemaRef};

use crate::expr::PlanExpr;

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
    Projection {
        input: Box<LogicalPlan>,
        exprs: Vec<(PlanExpr, String)>,
    },
    Limit {
        input: Box<LogicalPlan>,
        fetch: usize,
    },
}

impl LogicalPlan {
    pub fn schema(&self) -> SchemaRef {
        match self {
            LogicalPlan::Scan { source, projection, .. } => {
                let full = source.schema();
                match projection {
                    None => full,
                    Some(indices) => Arc::new(Schema::new(indices.iter().map(|&i| full.fields[i].clone()).collect())),
                }
            }
            LogicalPlan::Filter { input, .. } => input.schema(),
            LogicalPlan::Projection { input, exprs } => {
                let input_schema = input.schema();
                let fields = exprs
                    .iter()
                    .map(|(expr, name)| Field::new(name.clone(), expr.infer_type(&input_schema), true))
                    .collect();
                Arc::new(Schema::new(fields))
            }
            LogicalPlan::Limit { input, .. } => input.schema(),
        }
    }

    fn fmt_indent(&self, f: &mut fmt::Formatter<'_>, depth: usize) -> fmt::Result {
        let pad = "  ".repeat(depth);
        match self {
            LogicalPlan::Scan { table_name, projection, filters, .. } => {
                writeln!(f, "{pad}Scan: {table_name} projection={projection:?} filters={filters:?}")
            }
            LogicalPlan::Filter { input, predicate } => {
                writeln!(f, "{pad}Filter: {predicate}")?;
                input.fmt_indent(f, depth + 1)
            }
            LogicalPlan::Projection { input, exprs } => {
                let cols: Vec<String> = exprs.iter().map(|(e, name)| format!("{e} AS {name}")).collect();
                writeln!(f, "{pad}Projection: {}", cols.join(", "))?;
                input.fmt_indent(f, depth + 1)
            }
            LogicalPlan::Limit { input, fetch } => {
                writeln!(f, "{pad}Limit: {fetch}")?;
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
