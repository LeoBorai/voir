use std::path::Path;

use voir_catalog::Catalog;
use voir_sql::{Expr, Literal, OrderByExpr, Query, SelectItem, Statement};
use voir_types::{DataType, Schema, SchemaRef, Value, VoirError};

use crate::agg::{AggExpr, AggFunc, agg_func_from_name};
use crate::expr::{PlanExpr, ScalarFunc};
use crate::logical::{LogicalPlan, SortKey};

pub struct Binder<'a> {
    catalog: &'a Catalog,
}

impl<'a> Binder<'a> {
    #[must_use]
    pub fn new(catalog: &'a Catalog) -> Self {
        Self { catalog }
    }

    /// Returns the bound plan and whether the statement was `EXPLAIN ...`
    /// (in which case the caller should print the plan and not execute it).
    ///
    /// # Errors
    ///
    /// Returns an error if the query references an unknown table or column,
    /// or otherwise fails to bind.
    pub fn bind(&self, statement: &Statement) -> Result<(LogicalPlan, bool), VoirError> {
        match statement {
            Statement::Query(query) => Ok((self.bind_query(query)?, false)),
            Statement::Explain(query) => Ok((self.bind_query(query)?, true)),
        }
    }

    fn bind_query(&self, query: &Query) -> Result<LogicalPlan, VoirError> {
        let (mut plan, source_schema) = self.bind_scan(query)?;

        if let Some(predicate) = &query.selection {
            plan = bind_where(plan, predicate, &source_schema)?;
        }

        let (select_exprs, having_bound, order_keys) = if query_is_aggregate(query) {
            let binding = bind_aggregate_query(query, &source_schema, plan)?;
            plan = binding.plan;
            (binding.select_exprs, binding.having, binding.order_keys)
        } else {
            let select_exprs = bind_select_list(&query.projection, &source_schema)?;
            let order_keys = bind_order_by_plain(&query.order_by, &select_exprs, &source_schema)?;
            (select_exprs, None, order_keys)
        };

        if let Some(having) = having_bound {
            plan = LogicalPlan::Filter {
                input: Box::new(plan),
                predicate: having,
            };
        }

        if !order_keys.is_empty() {
            plan = LogicalPlan::Sort {
                input: Box::new(plan),
                keys: order_keys,
            };
        }

        plan = LogicalPlan::Projection {
            input: Box::new(plan),
            exprs: select_exprs,
        };

        if query.distinct {
            plan = LogicalPlan::Distinct {
                input: Box::new(plan),
            };
        }

        apply_limit_offset(plan, query)
    }

    fn bind_scan(&self, query: &Query) -> Result<(LogicalPlan, SchemaRef), VoirError> {
        let source = self.catalog.resolve(&query.from.path)?;
        let table_name = query.from.alias.clone().unwrap_or_else(|| {
            if self.catalog.table(&query.from.path).is_some() {
                query.from.path.clone()
            } else {
                table_name_from_path(&query.from.path)
            }
        });
        let source_schema = source.schema();
        let plan = LogicalPlan::Scan {
            table_name,
            source,
            projection: None,
            filters: Vec::new(),
        };
        Ok((plan, source_schema))
    }
}

fn bind_where(
    plan: LogicalPlan,
    predicate: &Expr,
    schema: &Schema,
) -> Result<LogicalPlan, VoirError> {
    if contains_aggregate(predicate) {
        return Err(VoirError::Plan(
            "aggregate functions are not allowed in WHERE (use HAVING)".into(),
        ));
    }
    let predicate = bind_expr(predicate, schema)?;
    Ok(LogicalPlan::Filter {
        input: Box::new(plan),
        predicate,
    })
}

fn apply_limit_offset(plan: LogicalPlan, query: &Query) -> Result<LogicalPlan, VoirError> {
    let skip = query
        .offset
        .as_ref()
        .map(bind_limit_or_offset)
        .transpose()?
        .unwrap_or(0);
    let fetch = query.limit.as_ref().map(bind_limit_or_offset).transpose()?;
    if skip > 0 || fetch.is_some() {
        Ok(LogicalPlan::Limit {
            input: Box::new(plan),
            skip,
            fetch,
        })
    } else {
        Ok(plan)
    }
}

/// Whether `query`'s `SELECT`/`HAVING`/`ORDER BY` needs whole-table or
/// grouped aggregation — either an explicit `GROUP BY`, or an aggregate call
/// appearing anywhere in those clauses.
fn query_is_aggregate(query: &Query) -> bool {
    !query.group_by.is_empty()
        || query
            .projection
            .iter()
            .any(|item| matches!(item, SelectItem::Expr { expr, .. } if contains_aggregate(expr)))
        || query.having.as_ref().is_some_and(contains_aggregate)
        || query.order_by.iter().any(|o| contains_aggregate(&o.expr))
}

struct AggregateBinding {
    plan: LogicalPlan,
    select_exprs: Vec<(PlanExpr, String)>,
    having: Option<PlanExpr>,
    order_keys: Vec<SortKey>,
}

fn bind_aggregate_query(
    query: &Query,
    schema: &Schema,
    input: LogicalPlan,
) -> Result<AggregateBinding, VoirError> {
    let group_by_names: Vec<String> = query.group_by.iter().map(display_expr_name).collect();
    let group_by_plan: Vec<PlanExpr> = query
        .group_by
        .iter()
        .map(|e| bind_expr(e, schema))
        .collect::<Result<_, _>>()?;

    let mut aggs: Vec<(Expr, AggExpr)> = Vec::new();

    let select_exprs = bind_aggregate_select_list(
        &query.projection,
        schema,
        &query.group_by,
        &group_by_names,
        &mut aggs,
    )?;

    let having = query
        .having
        .as_ref()
        .map(|h| resolve_post_agg_expr(h, schema, &query.group_by, &group_by_names, &mut aggs))
        .transpose()?;

    let order_keys = bind_order_by_aggregate(
        &query.order_by,
        &select_exprs,
        schema,
        &query.group_by,
        &group_by_names,
        &mut aggs,
    )?;

    let plan = LogicalPlan::Aggregate {
        input: Box::new(input),
        group_by: group_by_plan,
        group_by_names,
        aggregates: aggs.into_iter().map(|(_, a)| a).collect(),
    };

    Ok(AggregateBinding {
        plan,
        select_exprs,
        having,
        order_keys,
    })
}

fn bind_aggregate_select_list(
    items: &[SelectItem],
    schema: &Schema,
    group_by: &[Expr],
    group_by_names: &[String],
    aggs: &mut Vec<(Expr, AggExpr)>,
) -> Result<Vec<(PlanExpr, String)>, VoirError> {
    let mut select_exprs = Vec::new();
    for item in items {
        let (expr, alias) = match item {
            SelectItem::Wildcard => {
                return Err(VoirError::Plan(
                    "SELECT * is not allowed with GROUP BY or aggregate functions".into(),
                ));
            }
            SelectItem::Expr { expr, alias } => (expr, alias),
        };
        let name = alias.clone().unwrap_or_else(|| display_expr_name(expr));
        let bound = resolve_post_agg_expr(expr, schema, group_by, group_by_names, aggs)?;
        select_exprs.push((bound, name));
    }
    Ok(select_exprs)
}

/// If `expr` is a bare identifier matching a `SELECT`-list alias, reuses
/// that item's already-bound expression — the mechanism behind `ORDER BY
/// <alias>`, since `Sort` sits below `Projection` and never sees aliases.
fn order_by_alias_match(expr: &Expr, select_exprs: &[(PlanExpr, String)]) -> Option<PlanExpr> {
    match expr {
        Expr::Ident(name) => select_exprs
            .iter()
            .find(|(_, alias)| alias.eq_ignore_ascii_case(name))
            .map(|(e, _)| e.clone()),
        _ => None,
    }
}

fn bind_order_by_aggregate(
    order_by: &[OrderByExpr],
    select_exprs: &[(PlanExpr, String)],
    schema: &Schema,
    group_by: &[Expr],
    group_by_names: &[String],
    aggs: &mut Vec<(Expr, AggExpr)>,
) -> Result<Vec<SortKey>, VoirError> {
    let mut order_keys = Vec::new();
    for item in order_by {
        let bound = match order_by_alias_match(&item.expr, select_exprs) {
            Some(e) => e,
            None => resolve_post_agg_expr(&item.expr, schema, group_by, group_by_names, aggs)?,
        };
        order_keys.push(SortKey {
            expr: bound,
            desc: item.desc,
        });
    }
    Ok(order_keys)
}

fn bind_order_by_plain(
    order_by: &[OrderByExpr],
    select_exprs: &[(PlanExpr, String)],
    schema: &Schema,
) -> Result<Vec<SortKey>, VoirError> {
    let mut order_keys = Vec::new();
    for item in order_by {
        let bound = match order_by_alias_match(&item.expr, select_exprs) {
            Some(e) => e,
            None => bind_expr(&item.expr, schema)?,
        };
        order_keys.push(SortKey {
            expr: bound,
            desc: item.desc,
        });
    }
    Ok(order_keys)
}

fn bind_select_list(
    items: &[SelectItem],
    schema: &Schema,
) -> Result<Vec<(PlanExpr, String)>, VoirError> {
    let mut exprs = Vec::new();
    for item in items {
        match item {
            SelectItem::Wildcard => {
                for (index, field) in schema.fields.iter().enumerate() {
                    exprs.push((
                        PlanExpr::Column {
                            index,
                            name: field.name.clone(),
                        },
                        field.name.clone(),
                    ));
                }
            }
            SelectItem::Expr { expr, alias } => {
                let name = alias.clone().unwrap_or_else(|| display_expr_name(expr));
                let bound = bind_expr(expr, schema)?;
                exprs.push((bound, name));
            }
        }
    }
    Ok(exprs)
}

/// Recursively checks whether `expr` calls an aggregate function anywhere in
/// its tree — used to detect implicit whole-table aggregation (no `GROUP
/// BY`, an aggregate in `SELECT`/`HAVING`/`ORDER BY`) and to reject
/// aggregates in `WHERE`.
fn contains_aggregate(expr: &Expr) -> bool {
    match expr {
        Expr::Function { name, args, .. } => {
            agg_func_from_name(&name.to_ascii_lowercase()).is_some()
                || args.iter().any(contains_aggregate)
        }
        Expr::UnaryOp { expr, .. } | Expr::Cast { expr, .. } => contains_aggregate(expr),
        Expr::BinaryOp { left, right, .. } => contains_aggregate(left) || contains_aggregate(right),
        Expr::Case {
            operand,
            branches,
            else_expr,
        } => {
            operand.as_deref().is_some_and(contains_aggregate)
                || branches
                    .iter()
                    .any(|(w, t)| contains_aggregate(w) || contains_aggregate(t))
                || else_expr.as_deref().is_some_and(contains_aggregate)
        }
        Expr::Ident(_) | Expr::Literal(_) | Expr::Wildcard => false,
    }
}

/// Binds an expression that may sit at or above a `GROUP BY`/aggregate
/// boundary: an aggregate call is extracted into `aggs` (deduplicated by the
/// raw AST so `SELECT SUM(x) ... HAVING SUM(x) > 0` shares one accumulator)
/// and replaced with a reference to its output column; anything else must
/// resolve as a `GROUP BY` expression or a literal/scalar-function
/// combination of them.
fn resolve_post_agg_expr(
    expr: &Expr,
    schema: &Schema,
    group_by: &[Expr],
    group_by_names: &[String],
    aggs: &mut Vec<(Expr, AggExpr)>,
) -> Result<PlanExpr, VoirError> {
    if let Some(index) = group_by.iter().position(|g| g == expr) {
        return Ok(PlanExpr::Column {
            index,
            name: group_by_names[index].clone(),
        });
    }
    match expr {
        Expr::Function {
            name,
            args,
            distinct,
        } => resolve_function_in_agg_context(
            name,
            args,
            *distinct,
            expr,
            schema,
            group_by,
            group_by_names,
            aggs,
        ),
        Expr::Ident(name) => Err(VoirError::Plan(format!(
            "column \"{name}\" must appear in the GROUP BY clause or be used in an aggregate function"
        ))),
        Expr::Wildcard => Err(VoirError::Plan(
            "'*' is only valid inside COUNT(...)".into(),
        )),
        Expr::Literal(lit) => Ok(PlanExpr::Literal(literal_to_value(lit))),
        Expr::UnaryOp { op, expr } => Ok(PlanExpr::UnaryOp {
            op: *op,
            expr: Box::new(resolve_post_agg_expr(
                expr,
                schema,
                group_by,
                group_by_names,
                aggs,
            )?),
        }),
        Expr::BinaryOp { left, op, right } => Ok(PlanExpr::BinaryOp {
            left: Box::new(resolve_post_agg_expr(
                left,
                schema,
                group_by,
                group_by_names,
                aggs,
            )?),
            op: *op,
            right: Box::new(resolve_post_agg_expr(
                right,
                schema,
                group_by,
                group_by_names,
                aggs,
            )?),
        }),
        Expr::Case {
            operand,
            branches,
            else_expr,
        } => Ok(PlanExpr::Case {
            operand: operand
                .as_deref()
                .map(|e| resolve_post_agg_expr(e, schema, group_by, group_by_names, aggs))
                .transpose()?
                .map(Box::new),
            branches: branches
                .iter()
                .map(|(w, t)| {
                    Ok((
                        resolve_post_agg_expr(w, schema, group_by, group_by_names, aggs)?,
                        resolve_post_agg_expr(t, schema, group_by, group_by_names, aggs)?,
                    ))
                })
                .collect::<Result<_, VoirError>>()?,
            else_expr: else_expr
                .as_deref()
                .map(|e| resolve_post_agg_expr(e, schema, group_by, group_by_names, aggs))
                .transpose()?
                .map(Box::new),
        }),
        Expr::Cast { expr, type_name } => Ok(PlanExpr::Cast {
            expr: Box::new(resolve_post_agg_expr(
                expr,
                schema,
                group_by,
                group_by_names,
                aggs,
            )?),
            target: parse_data_type(type_name)?,
        }),
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_function_in_agg_context(
    name: &str,
    args: &[Expr],
    distinct: bool,
    raw: &Expr,
    schema: &Schema,
    group_by: &[Expr],
    group_by_names: &[String],
    aggs: &mut Vec<(Expr, AggExpr)>,
) -> Result<PlanExpr, VoirError> {
    let lower = name.to_ascii_lowercase();
    if let Some(func) = agg_func_from_name(&lower) {
        return resolve_aggregate_call(
            func,
            name,
            args,
            distinct,
            raw,
            schema,
            aggs,
            group_by.len(),
        );
    }
    let scalar = ScalarFunc::from_name(&lower)
        .ok_or_else(|| VoirError::Plan(format!("unknown function \"{name}\"")))?;
    let bound_args = args
        .iter()
        .map(|a| resolve_post_agg_expr(a, schema, group_by, group_by_names, aggs))
        .collect::<Result<Vec<_>, _>>()?;
    scalar
        .check_arity(bound_args.len())
        .map_err(VoirError::Plan)?;
    Ok(PlanExpr::Function {
        func: scalar,
        args: bound_args,
    })
}

#[allow(clippy::too_many_arguments)]
fn resolve_aggregate_call(
    func: AggFunc,
    name: &str,
    args: &[Expr],
    distinct: bool,
    raw: &Expr,
    schema: &Schema,
    aggs: &mut Vec<(Expr, AggExpr)>,
    group_by_len: usize,
) -> Result<PlanExpr, VoirError> {
    if let Some(existing) = aggs.iter().position(|(r, _)| r == raw) {
        let alias = aggs[existing].1.alias.clone();
        return Ok(PlanExpr::Column {
            index: group_by_len + existing,
            name: alias,
        });
    }
    let arg = match args {
        [Expr::Wildcard] if func == AggFunc::Count => None,
        [Expr::Wildcard] => {
            return Err(VoirError::Plan(
                "'*' is only valid inside COUNT(...)".into(),
            ));
        }
        [single] => Some(bind_expr(single, schema)?),
        _ => {
            return Err(VoirError::Plan(format!(
                "{name}() takes exactly one argument"
            )));
        }
    };
    let alias = display_expr_name(raw);
    let index = aggs.len();
    aggs.push((
        raw.clone(),
        AggExpr {
            func,
            arg,
            distinct,
            alias: alias.clone(),
        },
    ));
    Ok(PlanExpr::Column {
        index: group_by_len + index,
        name: alias,
    })
}

fn bind_expr(expr: &Expr, schema: &Schema) -> Result<PlanExpr, VoirError> {
    match expr {
        Expr::Ident(name) => {
            let index = schema
                .index_of(name)
                .ok_or_else(|| VoirError::Plan(format!("column \"{name}\" does not exist")))?;
            Ok(PlanExpr::Column {
                index,
                name: schema.fields[index].name.clone(),
            })
        }
        Expr::Literal(lit) => Ok(PlanExpr::Literal(literal_to_value(lit))),
        Expr::Wildcard => Err(VoirError::Plan(
            "'*' is only valid inside COUNT(...)".into(),
        )),
        Expr::UnaryOp { op, expr } => Ok(PlanExpr::UnaryOp {
            op: *op,
            expr: Box::new(bind_expr(expr, schema)?),
        }),
        Expr::BinaryOp { left, op, right } => Ok(PlanExpr::BinaryOp {
            left: Box::new(bind_expr(left, schema)?),
            op: *op,
            right: Box::new(bind_expr(right, schema)?),
        }),
        Expr::Case {
            operand,
            branches,
            else_expr,
        } => Ok(PlanExpr::Case {
            operand: operand
                .as_deref()
                .map(|e| bind_expr(e, schema))
                .transpose()?
                .map(Box::new),
            branches: branches
                .iter()
                .map(|(w, t)| Ok((bind_expr(w, schema)?, bind_expr(t, schema)?)))
                .collect::<Result<_, VoirError>>()?,
            else_expr: else_expr
                .as_deref()
                .map(|e| bind_expr(e, schema))
                .transpose()?
                .map(Box::new),
        }),
        Expr::Cast { expr, type_name } => Ok(PlanExpr::Cast {
            expr: Box::new(bind_expr(expr, schema)?),
            target: parse_data_type(type_name)?,
        }),
        Expr::Function { name, args, .. } => {
            let lower = name.to_ascii_lowercase();
            if agg_func_from_name(&lower).is_some() {
                return Err(VoirError::Plan(format!(
                    "aggregate function \"{name}\" requires GROUP BY (or must be used in HAVING)"
                )));
            }
            let func = ScalarFunc::from_name(&lower)
                .ok_or_else(|| VoirError::Plan(format!("unknown function \"{name}\"")))?;
            let bound_args = args
                .iter()
                .map(|a| bind_expr(a, schema))
                .collect::<Result<Vec<_>, _>>()?;
            func.check_arity(bound_args.len())
                .map_err(VoirError::Plan)?;
            Ok(PlanExpr::Function {
                func,
                args: bound_args,
            })
        }
    }
}

fn parse_data_type(name: &str) -> Result<DataType, VoirError> {
    match name.to_ascii_lowercase().as_str() {
        "int" | "integer" | "bigint" => Ok(DataType::Int64),
        "float" | "double" | "real" => Ok(DataType::Float64),
        "text" | "varchar" | "string" => Ok(DataType::Utf8),
        "boolean" | "bool" => Ok(DataType::Boolean),
        other => Err(VoirError::Plan(format!(
            "unsupported CAST target type \"{other}\""
        ))),
    }
}

fn bind_limit_or_offset(expr: &Expr) -> Result<usize, VoirError> {
    match expr {
        Expr::Literal(Literal::Int64(n)) => usize::try_from(*n).map_err(|_| {
            VoirError::Plan("LIMIT/OFFSET requires a non-negative integer literal".into())
        }),
        _ => Err(VoirError::Plan(
            "LIMIT/OFFSET requires a non-negative integer literal".into(),
        )),
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
        Expr::Wildcard => "*".to_string(),
        Expr::Function {
            name,
            args,
            distinct,
        } => {
            let args_str = args
                .iter()
                .map(display_expr_name)
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{}({}{})",
                name.to_ascii_lowercase(),
                if *distinct { "distinct " } else { "" },
                args_str
            )
        }
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
