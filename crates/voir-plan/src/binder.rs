use std::collections::HashMap;
use std::path::Path;

use voir_catalog::Catalog;
use voir_sql::{
    Cte, Expr, FromClause, JoinCondition, Literal, OrderByExpr, Query, Select, SelectItem, SetExpr,
    SetOperator, Statement,
};
use voir_types::{DataType, SchemaRef, Value, VoirError};

use crate::agg::{AggExpr, AggFunc, agg_func_from_name};
use crate::expr::{PlanExpr, ScalarFunc};
use crate::logical::{LogicalPlan, SortKey};
use crate::scope::Scope;

/// Case-insensitive CTE name -> its (still-unbound) query. Queries are
/// cloned into the map rather than borrowed so binding never has to thread
/// AST lifetimes through the recursion — cheap for the query sizes `voir`
/// deals with, and it keeps every binder function signature simple.
type CteEnv = HashMap<String, Query>;

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
        let ctes = CteEnv::new();
        let mut resolving = Vec::new();
        match statement {
            Statement::Query(query) => {
                Ok((self.bind_query(query, &ctes, &mut resolving)?.0, false))
            }
            Statement::Explain(query) => {
                Ok((self.bind_query(query, &ctes, &mut resolving)?.0, true))
            }
        }
    }

    fn bind_query(
        &self,
        query: &Query,
        outer_ctes: &CteEnv,
        resolving: &mut Vec<String>,
    ) -> Result<(LogicalPlan, SchemaRef), VoirError> {
        let ctes = merge_ctes(outer_ctes, &query.with);

        let plan = match &query.body {
            SetExpr::Select(select) => {
                self.bind_select_full(select, &query.order_by, &ctes, resolving)?
            }
            SetExpr::SetOp { .. } => {
                let mut plan = self.bind_set_expr(&query.body, &ctes, resolving)?;
                let schema = plan.schema();
                let out_scope = Scope::from_schema(None, &schema);
                let select_exprs: Vec<(PlanExpr, String)> = schema
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(index, field)| {
                        (
                            PlanExpr::Column {
                                index,
                                name: field.name.clone(),
                            },
                            field.name.clone(),
                        )
                    })
                    .collect();
                let order_keys = self.bind_order_by_plain(
                    &query.order_by,
                    &select_exprs,
                    &out_scope,
                    &ctes,
                    resolving,
                )?;
                if !order_keys.is_empty() {
                    plan = LogicalPlan::Sort {
                        input: Box::new(plan),
                        keys: order_keys,
                    };
                }
                plan
            }
        };

        let plan = apply_limit_offset(plan, query)?;
        let schema = plan.schema();
        Ok((plan, schema))
    }

    /// Binds a `SetExpr` tree into `LogicalPlan::SetOp`s, with no `ORDER
    /// BY`/`LIMIT` handling — those only ever apply once, at the enclosing
    /// `Query`. `UNION` (distinct) is desugared into `UNION ALL` wrapped in
    /// `Distinct`, reusing the existing streaming-dedup operator instead of
    /// a bespoke one.
    fn bind_set_expr(
        &self,
        body: &SetExpr,
        ctes: &CteEnv,
        resolving: &mut Vec<String>,
    ) -> Result<LogicalPlan, VoirError> {
        match body {
            SetExpr::Select(select) => self.bind_select_full(select, &[], ctes, resolving),
            SetExpr::SetOp {
                op,
                all,
                left,
                right,
            } => {
                let left_plan = self.bind_set_expr(left, ctes, resolving)?;
                let right_plan = self.bind_set_expr(right, ctes, resolving)?;
                let left_cols = left_plan.schema().fields.len();
                let right_cols = right_plan.schema().fields.len();
                if left_cols != right_cols {
                    return Err(VoirError::Plan(format!(
                        "each {op:?} query must have the same number of columns ({left_cols} vs {right_cols})"
                    )));
                }
                let plan = LogicalPlan::SetOp {
                    left: Box::new(left_plan),
                    right: Box::new(right_plan),
                    op: *op,
                    all: *all,
                };
                Ok(if matches!(op, SetOperator::Union) && !all {
                    LogicalPlan::Distinct {
                        input: Box::new(plan),
                    }
                } else {
                    plan
                })
            }
        }
    }

    fn bind_select_full(
        &self,
        select: &Select,
        order_by: &[OrderByExpr],
        ctes: &CteEnv,
        resolving: &mut Vec<String>,
    ) -> Result<LogicalPlan, VoirError> {
        let (mut plan, scope) = self.bind_from_clause(&select.from, ctes, resolving)?;

        if let Some(predicate) = &select.selection {
            plan = self.bind_where(plan, predicate, &scope, ctes, resolving)?;
        }

        let (select_exprs, having_bound, order_keys) = if select_is_aggregate(select, order_by) {
            let binding =
                self.bind_aggregate_select(select, order_by, &scope, plan, ctes, resolving)?;
            plan = binding.plan;
            (binding.select_exprs, binding.having, binding.order_keys)
        } else {
            let select_exprs =
                self.bind_select_list(&select.projection, &scope, ctes, resolving)?;
            let order_keys =
                self.bind_order_by_plain(order_by, &select_exprs, &scope, ctes, resolving)?;
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

        if select.distinct {
            plan = LogicalPlan::Distinct {
                input: Box::new(plan),
            };
        }

        Ok(plan)
    }

    fn bind_from_clause(
        &self,
        from: &FromClause,
        ctes: &CteEnv,
        resolving: &mut Vec<String>,
    ) -> Result<(LogicalPlan, Scope), VoirError> {
        match from {
            FromClause::Table { name, alias } => {
                self.bind_table_from(name, alias.as_deref(), ctes, resolving)
            }
            FromClause::Derived { subquery, alias } => {
                let (plan, schema) = self.bind_query(subquery, ctes, resolving)?;
                let scope = Scope::from_schema(Some(alias.as_str()), &schema);
                Ok((plan, scope))
            }
            FromClause::Join {
                left,
                right,
                join_type,
                condition,
            } => {
                let (left_plan, left_scope) = self.bind_from_clause(left, ctes, resolving)?;
                let (right_plan, right_scope) = self.bind_from_clause(right, ctes, resolving)?;
                let left_width = left_plan.schema().fields.len();
                let (scope, on) = match condition {
                    None => (Scope::concat(left_scope, right_scope, left_width), None),
                    Some(JoinCondition::On(expr)) => {
                        let combined = Scope::concat(left_scope, right_scope, left_width);
                        let bound = self.bind_expr(expr, &combined, ctes, resolving)?;
                        (combined, Some(bound))
                    }
                    Some(JoinCondition::Using(cols)) => {
                        let (combined, pred) =
                            Scope::join_using(left_scope, right_scope, left_width, cols)?;
                        (combined, Some(pred))
                    }
                };
                let plan = LogicalPlan::Join {
                    left: Box::new(left_plan),
                    right: Box::new(right_plan),
                    join_type: *join_type,
                    on,
                };
                Ok((plan, scope))
            }
        }
    }

    fn bind_table_from(
        &self,
        name: &str,
        alias: Option<&str>,
        ctes: &CteEnv,
        resolving: &mut Vec<String>,
    ) -> Result<(LogicalPlan, Scope), VoirError> {
        let lname = name.to_ascii_lowercase();
        if let Some(cte_query) = ctes.get(&lname) {
            if resolving.contains(&lname) {
                return Err(VoirError::Plan(format!(
                    "circular reference to CTE \"{name}\""
                )));
            }
            resolving.push(lname);
            let bound = self.bind_query(cte_query, ctes, resolving);
            resolving.pop();
            let (plan, schema) = bound?;
            let display_name = alias.map_or_else(|| name.to_string(), str::to_string);
            let scope = Scope::from_schema(Some(display_name.as_str()), &schema);
            return Ok((plan, scope));
        }

        let source = self.catalog.resolve(name)?;
        let table_name = alias.map_or_else(
            || {
                if self.catalog.table(name).is_some() {
                    name.to_string()
                } else {
                    table_name_from_path(name)
                }
            },
            str::to_string,
        );
        let schema = source.schema();
        let scope = Scope::from_schema(Some(table_name.as_str()), &schema);
        let plan = LogicalPlan::Scan {
            table_name,
            source,
            projection: None,
            filters: Vec::new(),
        };
        Ok((plan, scope))
    }

    fn bind_where(
        &self,
        plan: LogicalPlan,
        predicate: &Expr,
        scope: &Scope,
        ctes: &CteEnv,
        resolving: &mut Vec<String>,
    ) -> Result<LogicalPlan, VoirError> {
        if contains_aggregate(predicate) {
            return Err(VoirError::Plan(
                "aggregate functions are not allowed in WHERE (use HAVING)".into(),
            ));
        }
        let predicate = self.bind_expr(predicate, scope, ctes, resolving)?;
        Ok(LogicalPlan::Filter {
            input: Box::new(plan),
            predicate,
        })
    }

    fn bind_select_list(
        &self,
        items: &[SelectItem],
        scope: &Scope,
        ctes: &CteEnv,
        resolving: &mut Vec<String>,
    ) -> Result<Vec<(PlanExpr, String)>, VoirError> {
        let mut exprs = Vec::new();
        for item in items {
            match item {
                SelectItem::Wildcard => exprs.extend(scope.wildcard(None)?),
                SelectItem::QualifiedWildcard(table) => {
                    exprs.extend(scope.wildcard(Some(table))?);
                }
                SelectItem::Expr { expr, alias } => {
                    let name = alias.clone().unwrap_or_else(|| display_expr_name(expr));
                    let bound = self.bind_expr(expr, scope, ctes, resolving)?;
                    exprs.push((bound, name));
                }
            }
        }
        Ok(exprs)
    }

    fn bind_order_by_plain(
        &self,
        order_by: &[OrderByExpr],
        select_exprs: &[(PlanExpr, String)],
        scope: &Scope,
        ctes: &CteEnv,
        resolving: &mut Vec<String>,
    ) -> Result<Vec<SortKey>, VoirError> {
        let mut order_keys = Vec::new();
        for item in order_by {
            let bound = match order_by_alias_match(&item.expr, select_exprs) {
                Some(e) => e,
                None => self.bind_expr(&item.expr, scope, ctes, resolving)?,
            };
            order_keys.push(SortKey {
                expr: bound,
                desc: item.desc,
            });
        }
        Ok(order_keys)
    }

    #[allow(clippy::too_many_arguments)]
    fn bind_order_by_aggregate(
        &self,
        order_by: &[OrderByExpr],
        select_exprs: &[(PlanExpr, String)],
        scope: &Scope,
        group_by: &[Expr],
        group_by_names: &[String],
        aggs: &mut Vec<(Expr, AggExpr)>,
        ctes: &CteEnv,
        resolving: &mut Vec<String>,
    ) -> Result<Vec<SortKey>, VoirError> {
        let mut order_keys = Vec::new();
        for item in order_by {
            let bound = match order_by_alias_match(&item.expr, select_exprs) {
                Some(e) => e,
                None => self.resolve_post_agg_expr(
                    &item.expr,
                    scope,
                    group_by,
                    group_by_names,
                    aggs,
                    ctes,
                    resolving,
                )?,
            };
            order_keys.push(SortKey {
                expr: bound,
                desc: item.desc,
            });
        }
        Ok(order_keys)
    }

    fn bind_aggregate_select(
        &self,
        select: &Select,
        order_by: &[OrderByExpr],
        scope: &Scope,
        input: LogicalPlan,
        ctes: &CteEnv,
        resolving: &mut Vec<String>,
    ) -> Result<AggregateBinding, VoirError> {
        let group_by_names: Vec<String> = select.group_by.iter().map(display_expr_name).collect();
        let group_by_plan: Vec<PlanExpr> = select
            .group_by
            .iter()
            .map(|e| self.bind_expr(e, scope, ctes, resolving))
            .collect::<Result<_, _>>()?;

        let mut aggs: Vec<(Expr, AggExpr)> = Vec::new();

        let select_exprs = self.bind_aggregate_select_list(
            &select.projection,
            scope,
            &select.group_by,
            &group_by_names,
            &mut aggs,
            ctes,
            resolving,
        )?;

        let having = select
            .having
            .as_ref()
            .map(|h| {
                self.resolve_post_agg_expr(
                    h,
                    scope,
                    &select.group_by,
                    &group_by_names,
                    &mut aggs,
                    ctes,
                    resolving,
                )
            })
            .transpose()?;

        let order_keys = self.bind_order_by_aggregate(
            order_by,
            &select_exprs,
            scope,
            &select.group_by,
            &group_by_names,
            &mut aggs,
            ctes,
            resolving,
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

    #[allow(clippy::too_many_arguments)]
    fn bind_aggregate_select_list(
        &self,
        items: &[SelectItem],
        scope: &Scope,
        group_by: &[Expr],
        group_by_names: &[String],
        aggs: &mut Vec<(Expr, AggExpr)>,
        ctes: &CteEnv,
        resolving: &mut Vec<String>,
    ) -> Result<Vec<(PlanExpr, String)>, VoirError> {
        let mut select_exprs = Vec::new();
        for item in items {
            let (expr, alias) = match item {
                SelectItem::Wildcard | SelectItem::QualifiedWildcard(_) => {
                    return Err(VoirError::Plan(
                        "SELECT * is not allowed with GROUP BY or aggregate functions".into(),
                    ));
                }
                SelectItem::Expr { expr, alias } => (expr, alias),
            };
            let name = alias.clone().unwrap_or_else(|| display_expr_name(expr));
            let bound = self.resolve_post_agg_expr(
                expr,
                scope,
                group_by,
                group_by_names,
                aggs,
                ctes,
                resolving,
            )?;
            select_exprs.push((bound, name));
        }
        Ok(select_exprs)
    }

    /// Binds an expression that may sit at or above a `GROUP BY`/aggregate
    /// boundary: an aggregate call is extracted into `aggs` (deduplicated by
    /// the raw AST so `SELECT SUM(x) ... HAVING SUM(x) > 0` shares one
    /// accumulator) and replaced with a reference to its output column;
    /// anything else must resolve as a `GROUP BY` expression or a
    /// literal/scalar-function combination of them.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn resolve_post_agg_expr(
        &self,
        expr: &Expr,
        scope: &Scope,
        group_by: &[Expr],
        group_by_names: &[String],
        aggs: &mut Vec<(Expr, AggExpr)>,
        ctes: &CteEnv,
        resolving: &mut Vec<String>,
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
            } => self.resolve_function_in_agg_context(
                name,
                args,
                *distinct,
                expr,
                scope,
                group_by,
                group_by_names,
                aggs,
                ctes,
                resolving,
            ),
            Expr::Ident(name) => Err(VoirError::Plan(format!(
                "column \"{name}\" must appear in the GROUP BY clause or be used in an aggregate function"
            ))),
            Expr::CompoundIdent(table, name) => Err(VoirError::Plan(format!(
                "column \"{table}.{name}\" must appear in the GROUP BY clause or be used in an aggregate function"
            ))),
            Expr::Wildcard => Err(VoirError::Plan(
                "'*' is only valid inside COUNT(...)".into(),
            )),
            Expr::Literal(lit) => Ok(PlanExpr::Literal(literal_to_value(lit))),
            Expr::UnaryOp { op, expr } => Ok(PlanExpr::UnaryOp {
                op: *op,
                expr: Box::new(self.resolve_post_agg_expr(
                    expr,
                    scope,
                    group_by,
                    group_by_names,
                    aggs,
                    ctes,
                    resolving,
                )?),
            }),
            Expr::BinaryOp { left, op, right } => Ok(PlanExpr::BinaryOp {
                left: Box::new(self.resolve_post_agg_expr(
                    left,
                    scope,
                    group_by,
                    group_by_names,
                    aggs,
                    ctes,
                    resolving,
                )?),
                op: *op,
                right: Box::new(self.resolve_post_agg_expr(
                    right,
                    scope,
                    group_by,
                    group_by_names,
                    aggs,
                    ctes,
                    resolving,
                )?),
            }),
            Expr::Case {
                operand,
                branches,
                else_expr,
            } => Ok(PlanExpr::Case {
                operand: operand
                    .as_deref()
                    .map(|e| {
                        self.resolve_post_agg_expr(
                            e,
                            scope,
                            group_by,
                            group_by_names,
                            aggs,
                            ctes,
                            resolving,
                        )
                    })
                    .transpose()?
                    .map(Box::new),
                branches: branches
                    .iter()
                    .map(|(w, t)| {
                        Ok((
                            self.resolve_post_agg_expr(
                                w,
                                scope,
                                group_by,
                                group_by_names,
                                aggs,
                                ctes,
                                resolving,
                            )?,
                            self.resolve_post_agg_expr(
                                t,
                                scope,
                                group_by,
                                group_by_names,
                                aggs,
                                ctes,
                                resolving,
                            )?,
                        ))
                    })
                    .collect::<Result<_, VoirError>>()?,
                else_expr: else_expr
                    .as_deref()
                    .map(|e| {
                        self.resolve_post_agg_expr(
                            e,
                            scope,
                            group_by,
                            group_by_names,
                            aggs,
                            ctes,
                            resolving,
                        )
                    })
                    .transpose()?
                    .map(Box::new),
            }),
            Expr::Cast { expr, type_name } => Ok(PlanExpr::Cast {
                expr: Box::new(self.resolve_post_agg_expr(
                    expr,
                    scope,
                    group_by,
                    group_by_names,
                    aggs,
                    ctes,
                    resolving,
                )?),
                target: parse_data_type(type_name)?,
            }),
            Expr::InList { expr, list } => Ok(PlanExpr::InList {
                expr: Box::new(self.resolve_post_agg_expr(
                    expr,
                    scope,
                    group_by,
                    group_by_names,
                    aggs,
                    ctes,
                    resolving,
                )?),
                list: list
                    .iter()
                    .map(|e| {
                        self.resolve_post_agg_expr(
                            e,
                            scope,
                            group_by,
                            group_by_names,
                            aggs,
                            ctes,
                            resolving,
                        )
                    })
                    .collect::<Result<_, VoirError>>()?,
            }),
            Expr::InSubquery { expr, subquery } => {
                let bound = self.resolve_post_agg_expr(
                    expr,
                    scope,
                    group_by,
                    group_by_names,
                    aggs,
                    ctes,
                    resolving,
                )?;
                let (plan, _) = self.bind_query(subquery, ctes, resolving)?;
                Ok(PlanExpr::InSubquery {
                    expr: Box::new(bound),
                    plan: Box::new(plan),
                })
            }
            Expr::Exists { subquery } => {
                let (plan, _) = self.bind_query(subquery, ctes, resolving)?;
                Ok(PlanExpr::Exists(Box::new(plan)))
            }
            Expr::ScalarSubquery(subquery) => {
                let (plan, _) = self.bind_query(subquery, ctes, resolving)?;
                Ok(PlanExpr::ScalarSubquery(Box::new(plan)))
            }
        }
    }

    #[allow(clippy::too_many_arguments, clippy::similar_names)]
    fn resolve_function_in_agg_context(
        &self,
        name: &str,
        args: &[Expr],
        distinct: bool,
        raw: &Expr,
        scope: &Scope,
        group_by: &[Expr],
        group_by_names: &[String],
        aggs: &mut Vec<(Expr, AggExpr)>,
        ctes: &CteEnv,
        resolving: &mut Vec<String>,
    ) -> Result<PlanExpr, VoirError> {
        let lower = name.to_ascii_lowercase();
        if let Some(func) = agg_func_from_name(&lower) {
            return self.resolve_aggregate_call(
                func,
                name,
                args,
                distinct,
                raw,
                scope,
                aggs,
                group_by.len(),
                ctes,
                resolving,
            );
        }
        let scalar = ScalarFunc::from_name(&lower)
            .ok_or_else(|| VoirError::Plan(format!("unknown function \"{name}\"")))?;
        let bound_args = args
            .iter()
            .map(|a| {
                self.resolve_post_agg_expr(
                    a,
                    scope,
                    group_by,
                    group_by_names,
                    aggs,
                    ctes,
                    resolving,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        scalar
            .check_arity(bound_args.len())
            .map_err(VoirError::Plan)?;
        Ok(PlanExpr::Function {
            func: scalar,
            args: bound_args,
        })
    }

    #[allow(clippy::too_many_arguments, clippy::similar_names)]
    fn resolve_aggregate_call(
        &self,
        func: AggFunc,
        name: &str,
        args: &[Expr],
        distinct: bool,
        raw: &Expr,
        scope: &Scope,
        aggs: &mut Vec<(Expr, AggExpr)>,
        group_by_len: usize,
        ctes: &CteEnv,
        resolving: &mut Vec<String>,
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
            [single] => Some(self.bind_expr(single, scope, ctes, resolving)?),
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

    fn bind_expr(
        &self,
        expr: &Expr,
        scope: &Scope,
        ctes: &CteEnv,
        resolving: &mut Vec<String>,
    ) -> Result<PlanExpr, VoirError> {
        match expr {
            Expr::Ident(name) => scope.resolve(None, name),
            Expr::CompoundIdent(table, name) => scope.resolve(Some(table.as_str()), name),
            Expr::Literal(lit) => Ok(PlanExpr::Literal(literal_to_value(lit))),
            Expr::Wildcard => Err(VoirError::Plan(
                "'*' is only valid inside COUNT(...)".into(),
            )),
            Expr::UnaryOp { op, expr } => Ok(PlanExpr::UnaryOp {
                op: *op,
                expr: Box::new(self.bind_expr(expr, scope, ctes, resolving)?),
            }),
            Expr::BinaryOp { left, op, right } => Ok(PlanExpr::BinaryOp {
                left: Box::new(self.bind_expr(left, scope, ctes, resolving)?),
                op: *op,
                right: Box::new(self.bind_expr(right, scope, ctes, resolving)?),
            }),
            Expr::Case {
                operand,
                branches,
                else_expr,
            } => Ok(PlanExpr::Case {
                operand: operand
                    .as_deref()
                    .map(|e| self.bind_expr(e, scope, ctes, resolving))
                    .transpose()?
                    .map(Box::new),
                branches: branches
                    .iter()
                    .map(|(w, t)| {
                        Ok((
                            self.bind_expr(w, scope, ctes, resolving)?,
                            self.bind_expr(t, scope, ctes, resolving)?,
                        ))
                    })
                    .collect::<Result<_, VoirError>>()?,
                else_expr: else_expr
                    .as_deref()
                    .map(|e| self.bind_expr(e, scope, ctes, resolving))
                    .transpose()?
                    .map(Box::new),
            }),
            Expr::Cast { expr, type_name } => Ok(PlanExpr::Cast {
                expr: Box::new(self.bind_expr(expr, scope, ctes, resolving)?),
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
                    .map(|a| self.bind_expr(a, scope, ctes, resolving))
                    .collect::<Result<Vec<_>, _>>()?;
                func.check_arity(bound_args.len())
                    .map_err(VoirError::Plan)?;
                Ok(PlanExpr::Function {
                    func,
                    args: bound_args,
                })
            }
            Expr::InList { expr, list } => Ok(PlanExpr::InList {
                expr: Box::new(self.bind_expr(expr, scope, ctes, resolving)?),
                list: list
                    .iter()
                    .map(|e| self.bind_expr(e, scope, ctes, resolving))
                    .collect::<Result<_, _>>()?,
            }),
            Expr::InSubquery { expr, subquery } => {
                let bound = self.bind_expr(expr, scope, ctes, resolving)?;
                let (plan, _) = self.bind_query(subquery, ctes, resolving)?;
                Ok(PlanExpr::InSubquery {
                    expr: Box::new(bound),
                    plan: Box::new(plan),
                })
            }
            Expr::Exists { subquery } => {
                let (plan, _) = self.bind_query(subquery, ctes, resolving)?;
                Ok(PlanExpr::Exists(Box::new(plan)))
            }
            Expr::ScalarSubquery(subquery) => {
                let (plan, _) = self.bind_query(subquery, ctes, resolving)?;
                Ok(PlanExpr::ScalarSubquery(Box::new(plan)))
            }
        }
    }
}

fn merge_ctes(outer: &CteEnv, new: &[Cte]) -> CteEnv {
    if new.is_empty() {
        return outer.clone();
    }
    let mut merged = outer.clone();
    for cte in new {
        merged.insert(cte.name.to_ascii_lowercase(), (*cte.query).clone());
    }
    merged
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

/// Whether `select`'s `SELECT`/`HAVING`/`ORDER BY` needs whole-table or
/// grouped aggregation — either an explicit `GROUP BY`, or an aggregate call
/// appearing anywhere in those clauses. `order_by` is threaded in separately
/// since it lives on the enclosing `Query`, not `Select`.
fn select_is_aggregate(select: &Select, order_by: &[OrderByExpr]) -> bool {
    !select.group_by.is_empty()
        || select
            .projection
            .iter()
            .any(|item| matches!(item, SelectItem::Expr { expr, .. } if contains_aggregate(expr)))
        || select.having.as_ref().is_some_and(contains_aggregate)
        || order_by.iter().any(|o| contains_aggregate(&o.expr))
}

struct AggregateBinding {
    plan: LogicalPlan,
    select_exprs: Vec<(PlanExpr, String)>,
    having: Option<PlanExpr>,
    order_keys: Vec<SortKey>,
}

/// Recursively checks whether `expr` calls an aggregate function anywhere in
/// its tree — used to detect implicit whole-table aggregation (no `GROUP
/// BY`, an aggregate in `SELECT`/`HAVING`/`ORDER BY`) and to reject
/// aggregates in `WHERE`. Subqueries are opaque here: an aggregate inside a
/// nested subquery belongs to that subquery's own (independently bound)
/// aggregation, not this one.
fn contains_aggregate(expr: &Expr) -> bool {
    match expr {
        Expr::Function { name, args, .. } => {
            agg_func_from_name(&name.to_ascii_lowercase()).is_some()
                || args.iter().any(contains_aggregate)
        }
        Expr::UnaryOp { expr, .. } | Expr::Cast { expr, .. } | Expr::InSubquery { expr, .. } => {
            contains_aggregate(expr)
        }
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
        Expr::InList { expr, list } => {
            contains_aggregate(expr) || list.iter().any(contains_aggregate)
        }
        Expr::Ident(_)
        | Expr::CompoundIdent(_, _)
        | Expr::Literal(_)
        | Expr::Wildcard
        | Expr::Exists { .. }
        | Expr::ScalarSubquery(_) => false,
    }
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
        Expr::Ident(name) | Expr::CompoundIdent(_, name) => name.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;
    use voir_catalog::{BatchStream, SourceRegistry, TableProvider};
    use voir_types::{Batch, Field, Schema};

    struct TestSource {
        schema: SchemaRef,
    }

    impl TableProvider for TestSource {
        fn schema(&self) -> SchemaRef {
            self.schema.clone()
        }
        fn scan(
            &self,
            _projection: Option<&[usize]>,
            _filters: &[voir_catalog::FilterExpr],
        ) -> Result<Box<dyn BatchStream>, VoirError> {
            struct Empty;
            impl BatchStream for Empty {
                fn next_batch(&mut self) -> Result<Option<Batch>, VoirError> {
                    Ok(None)
                }
            }
            Ok(Box::new(Empty))
        }
    }

    fn catalog_with(tables: &[(&str, &[(&str, DataType)])]) -> Catalog {
        let mut cat = Catalog::new(SourceRegistry::new());
        for (name, cols) in tables {
            let fields = cols.iter().map(|(n, t)| Field::new(*n, *t, true)).collect();
            let provider: Arc<dyn TableProvider> = Arc::new(TestSource {
                schema: Arc::new(Schema::new(fields)),
            });
            cat.register_table(name, PathBuf::from(name), provider);
        }
        cat
    }

    fn bind(sql: &str, catalog: &Catalog) -> Result<LogicalPlan, VoirError> {
        let stmt = voir_sql::parse(sql)?;
        Binder::new(catalog).bind(&stmt).map(|(plan, _)| plan)
    }

    #[test]
    fn binds_inner_join_with_qualified_columns() {
        let catalog = catalog_with(&[
            ("a", &[("id", DataType::Int64), ("name", DataType::Utf8)]),
            ("b", &[("id", DataType::Int64), ("amount", DataType::Int64)]),
        ]);
        let plan = bind(
            "SELECT a.name, b.amount FROM a JOIN b ON a.id = b.id",
            &catalog,
        )
        .unwrap();
        let schema = plan.schema();
        assert_eq!(schema.fields.len(), 2);
        assert_eq!(schema.fields[0].name, "name");
        assert_eq!(schema.fields[1].name, "amount");
    }

    #[test]
    fn ambiguous_bare_column_after_join_errors() {
        let catalog = catalog_with(&[
            ("a", &[("id", DataType::Int64)]),
            ("b", &[("id", DataType::Int64)]),
        ]);
        let err = bind("SELECT id FROM a JOIN b ON a.id = b.id", &catalog).unwrap_err();
        assert!(err.to_string().contains("ambiguous"));
    }

    #[test]
    fn using_join_merges_column_and_stays_unambiguous() {
        let catalog = catalog_with(&[
            ("a", &[("id", DataType::Int64), ("x", DataType::Int64)]),
            ("b", &[("id", DataType::Int64), ("y", DataType::Int64)]),
        ]);
        let plan = bind("SELECT id, x, y FROM a JOIN b USING (id)", &catalog).unwrap();
        assert_eq!(plan.schema().fields.len(), 3);
    }

    #[test]
    fn binds_cte() {
        let catalog = catalog_with(&[("a", &[("id", DataType::Int64)])]);
        let plan = bind("WITH s AS (SELECT id FROM a) SELECT id FROM s", &catalog).unwrap();
        assert_eq!(plan.schema().fields.len(), 1);
    }

    #[test]
    fn circular_cte_reference_errors() {
        let catalog = catalog_with(&[]);
        let err = bind("WITH s AS (SELECT * FROM s) SELECT * FROM s", &catalog).unwrap_err();
        assert!(err.to_string().contains("circular"));
    }

    #[test]
    fn binds_union_with_matching_column_counts() {
        let catalog = catalog_with(&[
            ("a", &[("id", DataType::Int64)]),
            ("b", &[("id", DataType::Int64)]),
        ]);
        let plan = bind("SELECT id FROM a UNION SELECT id FROM b", &catalog).unwrap();
        assert_eq!(plan.schema().fields.len(), 1);
    }

    #[test]
    fn union_with_mismatched_columns_errors() {
        let catalog = catalog_with(&[
            ("a", &[("id", DataType::Int64)]),
            ("b", &[("id", DataType::Int64), ("x", DataType::Int64)]),
        ]);
        let err = bind("SELECT id FROM a UNION SELECT * FROM b", &catalog).unwrap_err();
        assert!(err.to_string().contains("same number of columns"));
    }

    #[test]
    fn binds_in_subquery() {
        let catalog = catalog_with(&[
            ("a", &[("id", DataType::Int64)]),
            ("b", &[("id", DataType::Int64)]),
        ]);
        let plan = bind("SELECT id FROM a WHERE id IN (SELECT id FROM b)", &catalog).unwrap();
        assert_eq!(plan.schema().fields.len(), 1);
    }
}
