use std::path::Path;

use voir_catalog::SourceRegistry;
use voir_sql::{Expr, Literal, Query, SelectItem, Statement};
use voir_types::{Schema, Value, VoirError};

use crate::expr::PlanExpr;
use crate::logical::LogicalPlan;

pub struct Binder<'a> {
    registry: &'a SourceRegistry,
}

impl<'a> Binder<'a> {
    pub fn new(registry: &'a SourceRegistry) -> Self {
        Self { registry }
    }

    /// Returns the bound plan and whether the statement was `EXPLAIN ...`
    /// (in which case the caller should print the plan and not execute it).
    pub fn bind(&self, statement: &Statement) -> Result<(LogicalPlan, bool), VoirError> {
        match statement {
            Statement::Query(query) => Ok((self.bind_query(query)?, false)),
            Statement::Explain(query) => Ok((self.bind_query(query)?, true)),
        }
    }

    fn bind_query(&self, query: &Query) -> Result<LogicalPlan, VoirError> {
        let source = self.registry.resolve(Path::new(&query.from.path))?;
        let table_name = query.from.alias.clone().unwrap_or_else(|| table_name_from_path(&query.from.path));
        let source_schema = source.schema();

        let mut plan = LogicalPlan::Scan {
            table_name,
            source,
            projection: None,
            filters: Vec::new(),
        };

        if let Some(predicate) = &query.selection {
            let predicate = self.bind_expr(predicate, &source_schema)?;
            plan = LogicalPlan::Filter { input: Box::new(plan), predicate };
        }

        let exprs = self.bind_select_list(&query.projection, &source_schema)?;
        plan = LogicalPlan::Projection { input: Box::new(plan), exprs };

        if let Some(limit_expr) = &query.limit {
            let fetch = bind_limit(limit_expr)?;
            plan = LogicalPlan::Limit { input: Box::new(plan), fetch };
        }

        Ok(plan)
    }

    fn bind_select_list(&self, items: &[SelectItem], schema: &Schema) -> Result<Vec<(PlanExpr, String)>, VoirError> {
        let mut exprs = Vec::new();
        for item in items {
            match item {
                SelectItem::Wildcard => {
                    for (index, field) in schema.fields.iter().enumerate() {
                        exprs.push((PlanExpr::Column { index, name: field.name.clone() }, field.name.clone()));
                    }
                }
                SelectItem::Expr { expr, alias } => {
                    let name = alias.clone().unwrap_or_else(|| display_expr_name(expr));
                    let bound = self.bind_expr(expr, schema)?;
                    exprs.push((bound, name));
                }
            }
        }
        Ok(exprs)
    }

    fn bind_expr(&self, expr: &Expr, schema: &Schema) -> Result<PlanExpr, VoirError> {
        match expr {
            Expr::Ident(name) => {
                let index = schema
                    .index_of(name)
                    .ok_or_else(|| VoirError::Plan(format!("column \"{name}\" does not exist")))?;
                Ok(PlanExpr::Column { index, name: schema.fields[index].name.clone() })
            }
            Expr::Literal(lit) => Ok(PlanExpr::Literal(literal_to_value(lit))),
            Expr::UnaryOp { op, expr } => Ok(PlanExpr::UnaryOp {
                op: *op,
                expr: Box::new(self.bind_expr(expr, schema)?),
            }),
            Expr::BinaryOp { left, op, right } => Ok(PlanExpr::BinaryOp {
                left: Box::new(self.bind_expr(left, schema)?),
                op: *op,
                right: Box::new(self.bind_expr(right, schema)?),
            }),
        }
    }
}

fn bind_limit(expr: &Expr) -> Result<usize, VoirError> {
    match expr {
        Expr::Literal(Literal::Int64(n)) if *n >= 0 => Ok(*n as usize),
        _ => Err(VoirError::Plan("LIMIT requires a non-negative integer literal".into())),
    }
}

fn literal_to_value(lit: &Literal) -> Value {
    match lit {
        Literal::Null => Value::Null,
        Literal::Boolean(b) => Value::Boolean(*b),
        Literal::Int64(v) => Value::Int64(*v),
        Literal::Float64(v) => Value::Float64(*v),
        Literal::String(s) => Value::utf8(s.clone()),
    }
}

fn display_expr_name(expr: &Expr) -> String {
    match expr {
        Expr::Ident(name) => name.clone(),
        _ => "?column?".to_string(),
    }
}

fn table_name_from_path(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_string()
}
