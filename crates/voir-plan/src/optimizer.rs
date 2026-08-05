use std::collections::{BTreeSet, HashMap};

use voir_catalog::{CompareOp, FilterExpr};
use voir_sql::BinaryOperator;

use crate::agg::AggExpr;
use crate::expr::PlanExpr;
use crate::logical::{LogicalPlan, SortKey};

/// Runs the rule-based passes in a fixed order. Each pass is a full
/// `LogicalPlan -> LogicalPlan` rewrite; there's no cost model or iteration
/// to a fixed point yet, since none of the current rules can re-enable each
/// other.
///
/// `push_down_projection` assumes one flat, single-scan index space rooted
/// at the plan it's given — true for every plan shape before `Join`/`SetOp`
/// existed. A `Join` concatenates two independently-indexed subtrees and a
/// `SetOp` requires both branches to keep identical column positions, so
/// when either appears anywhere in the plan, projection pushdown is skipped
/// for the whole tree rather than risk misrewriting one side's indices.
#[must_use]
pub fn optimize(plan: LogicalPlan) -> LogicalPlan {
    let plan = push_down_predicates(plan);
    if contains_join_or_setop(&plan) {
        plan
    } else {
        push_down_projection(plan)
    }
}

fn contains_join_or_setop(plan: &LogicalPlan) -> bool {
    match plan {
        LogicalPlan::Join { .. } | LogicalPlan::SetOp { .. } => true,
        LogicalPlan::Scan { .. } => false,
        LogicalPlan::Filter { input, .. }
        | LogicalPlan::Sort { input, .. }
        | LogicalPlan::Projection { input, .. }
        | LogicalPlan::Distinct { input }
        | LogicalPlan::Limit { input, .. }
        | LogicalPlan::Aggregate { input, .. } => contains_join_or_setop(input),
    }
}

/// Moves simple `WHERE` conjuncts (`column <op> literal`, `AND`-joined) onto
/// the `Scan` as a best-effort hint. CSV ignores them today — "returning
/// fewer filters as applied is always correct" — but the `Filter` operator
/// still re-checks everything, so this is free groundwork for sources (e.g.
/// Parquet) that can actually use it.
fn push_down_predicates(plan: LogicalPlan) -> LogicalPlan {
    match plan {
        LogicalPlan::Filter { input, predicate } => {
            let input = match *input {
                LogicalPlan::Scan {
                    table_name,
                    source,
                    projection,
                    ..
                } => {
                    let mut filters = Vec::new();
                    collect_conjuncts(&predicate, &mut filters);
                    LogicalPlan::Scan {
                        table_name,
                        source,
                        projection,
                        filters,
                    }
                }
                other => push_down_predicates(other),
            };
            LogicalPlan::Filter {
                input: Box::new(input),
                predicate,
            }
        }
        LogicalPlan::Aggregate {
            input,
            group_by,
            group_by_names,
            aggregates,
        } => LogicalPlan::Aggregate {
            input: Box::new(push_down_predicates(*input)),
            group_by,
            group_by_names,
            aggregates,
        },
        LogicalPlan::Sort { input, keys } => LogicalPlan::Sort {
            input: Box::new(push_down_predicates(*input)),
            keys,
        },
        LogicalPlan::Projection { input, exprs } => LogicalPlan::Projection {
            input: Box::new(push_down_predicates(*input)),
            exprs,
        },
        LogicalPlan::Distinct { input } => LogicalPlan::Distinct {
            input: Box::new(push_down_predicates(*input)),
        },
        LogicalPlan::Limit { input, skip, fetch } => LogicalPlan::Limit {
            input: Box::new(push_down_predicates(*input)),
            skip,
            fetch,
        },
        LogicalPlan::Join {
            left,
            right,
            join_type,
            on,
        } => LogicalPlan::Join {
            left: Box::new(push_down_predicates(*left)),
            right: Box::new(push_down_predicates(*right)),
            join_type,
            on,
        },
        LogicalPlan::SetOp {
            left,
            right,
            op,
            all,
        } => LogicalPlan::SetOp {
            left: Box::new(push_down_predicates(*left)),
            right: Box::new(push_down_predicates(*right)),
            op,
            all,
        },
        LogicalPlan::Scan { .. } => plan,
    }
}

fn collect_conjuncts(expr: &PlanExpr, out: &mut Vec<FilterExpr>) {
    if let PlanExpr::BinaryOp {
        left,
        op: BinaryOperator::And,
        right,
    } = expr
    {
        collect_conjuncts(left, out);
        collect_conjuncts(right, out);
        return;
    }
    if let Some(filter) = to_filter_expr(expr) {
        out.push(filter);
    }
}

fn to_filter_expr(expr: &PlanExpr) -> Option<FilterExpr> {
    match expr {
        PlanExpr::Column { index, .. } => Some(FilterExpr::Column(*index)),
        PlanExpr::Literal(v) => Some(FilterExpr::Literal(v.clone())),
        PlanExpr::BinaryOp { left, op, right } => {
            let op = match op {
                BinaryOperator::Eq => CompareOp::Eq,
                BinaryOperator::NotEq => CompareOp::NotEq,
                BinaryOperator::Lt => CompareOp::Lt,
                BinaryOperator::LtEq => CompareOp::LtEq,
                BinaryOperator::Gt => CompareOp::Gt,
                BinaryOperator::GtEq => CompareOp::GtEq,
                _ => return None,
            };
            Some(FilterExpr::Compare {
                left: Box::new(to_filter_expr(left)?),
                op,
                right: Box::new(to_filter_expr(right)?),
            })
        }
        _ => None,
    }
}

/// Computes the minimal column set each scan must produce and rewrites the
/// tree so `Scan.projection` reflects it — on CSV this alone skips UTF-8
/// validation and parsing on unreferenced fields.
///
/// An `Aggregate` node is a hard boundary: everything at or below it shares
/// the source schema's index space, but everything above it (`HAVING`,
/// `ORDER BY`, the final `Projection`) indexes into the aggregate's *own*
/// output schema instead. Only the pre-boundary portion gets rewritten here.
fn push_down_projection(plan: LogicalPlan) -> LogicalPlan {
    let mut needed = BTreeSet::new();
    collect_needed_columns(&plan, &mut needed);
    let indices: Vec<usize> = needed.into_iter().collect();
    let remap: HashMap<usize, usize> = indices
        .iter()
        .enumerate()
        .map(|(new, &old)| (old, new))
        .collect();
    rewrite(plan, &indices, &remap)
}

fn subtree_has_aggregate(plan: &LogicalPlan) -> bool {
    match plan {
        LogicalPlan::Scan { .. } | LogicalPlan::Join { .. } | LogicalPlan::SetOp { .. } => false,
        LogicalPlan::Aggregate { .. } => true,
        LogicalPlan::Filter { input, .. }
        | LogicalPlan::Sort { input, .. }
        | LogicalPlan::Projection { input, .. }
        | LogicalPlan::Distinct { input }
        | LogicalPlan::Limit { input, .. } => subtree_has_aggregate(input),
    }
}

fn collect_needed_columns(plan: &LogicalPlan, needed: &mut BTreeSet<usize>) {
    match plan {
        // `optimize` never calls this pass when a `Join`/`SetOp` is present
        // anywhere in the plan (see its doc comment) — projection pushdown's
        // single flat index space doesn't hold once two subtrees have been
        // concatenated.
        LogicalPlan::Scan { .. } | LogicalPlan::Join { .. } | LogicalPlan::SetOp { .. } => {}
        LogicalPlan::Aggregate {
            input,
            group_by,
            aggregates,
            ..
        } => {
            for g in group_by {
                collect_expr_columns(g, needed);
            }
            for agg in aggregates {
                if let Some(arg) = &agg.arg {
                    collect_expr_columns(arg, needed);
                }
            }
            collect_needed_columns(input, needed);
        }
        LogicalPlan::Filter { input, predicate } => {
            if !subtree_has_aggregate(input) {
                collect_expr_columns(predicate, needed);
            }
            collect_needed_columns(input, needed);
        }
        LogicalPlan::Sort { input, keys } => {
            if !subtree_has_aggregate(input) {
                for key in keys {
                    collect_expr_columns(&key.expr, needed);
                }
            }
            collect_needed_columns(input, needed);
        }
        LogicalPlan::Projection { input, exprs } => {
            if !subtree_has_aggregate(input) {
                for (expr, _) in exprs {
                    collect_expr_columns(expr, needed);
                }
            }
            collect_needed_columns(input, needed);
        }
        LogicalPlan::Distinct { input } | LogicalPlan::Limit { input, .. } => {
            collect_needed_columns(input, needed);
        }
    }
}

fn collect_expr_columns(expr: &PlanExpr, needed: &mut BTreeSet<usize>) {
    match expr {
        PlanExpr::Column { index, .. } => {
            needed.insert(*index);
        }
        PlanExpr::Literal(_) | PlanExpr::Exists(_) | PlanExpr::ScalarSubquery(_) => {}
        PlanExpr::UnaryOp { expr, .. } | PlanExpr::Cast { expr, .. } => {
            collect_expr_columns(expr, needed);
        }
        PlanExpr::BinaryOp { left, right, .. } => {
            collect_expr_columns(left, needed);
            collect_expr_columns(right, needed);
        }
        PlanExpr::Function { args, .. } => {
            for arg in args {
                collect_expr_columns(arg, needed);
            }
        }
        PlanExpr::Case {
            operand,
            branches,
            else_expr,
        } => {
            if let Some(operand) = operand {
                collect_expr_columns(operand, needed);
            }
            for (when, then) in branches {
                collect_expr_columns(when, needed);
                collect_expr_columns(then, needed);
            }
            if let Some(else_expr) = else_expr {
                collect_expr_columns(else_expr, needed);
            }
        }
        PlanExpr::InList { expr, list } => {
            collect_expr_columns(expr, needed);
            for item in list {
                collect_expr_columns(item, needed);
            }
        }
        // Uncorrelated: the nested plan indexes its own independent schema,
        // contributing nothing to the outer scan's needed-column set.
        PlanExpr::InSubquery { expr, .. } => collect_expr_columns(expr, needed),
    }
}

fn rewrite(plan: LogicalPlan, indices: &[usize], remap: &HashMap<usize, usize>) -> LogicalPlan {
    match plan {
        LogicalPlan::Scan {
            table_name,
            source,
            filters,
            ..
        } => {
            let projection = if indices.is_empty() {
                None
            } else {
                Some(indices.to_vec())
            };
            LogicalPlan::Scan {
                table_name,
                source,
                projection,
                filters,
            }
        }
        LogicalPlan::Aggregate {
            input,
            group_by,
            group_by_names,
            aggregates,
        } => LogicalPlan::Aggregate {
            input: Box::new(rewrite(*input, indices, remap)),
            group_by: group_by.into_iter().map(|e| remap_expr(e, remap)).collect(),
            group_by_names,
            aggregates: aggregates
                .into_iter()
                .map(|agg| AggExpr {
                    arg: agg.arg.map(|e| remap_expr(e, remap)),
                    ..agg
                })
                .collect(),
        },
        LogicalPlan::Filter { input, predicate } => {
            let predicate = if subtree_has_aggregate(&input) {
                predicate
            } else {
                remap_expr(predicate, remap)
            };
            LogicalPlan::Filter {
                input: Box::new(rewrite(*input, indices, remap)),
                predicate,
            }
        }
        LogicalPlan::Sort { input, keys } => {
            let keys = if subtree_has_aggregate(&input) {
                keys
            } else {
                keys.into_iter()
                    .map(|k| SortKey {
                        expr: remap_expr(k.expr, remap),
                        desc: k.desc,
                    })
                    .collect()
            };
            LogicalPlan::Sort {
                input: Box::new(rewrite(*input, indices, remap)),
                keys,
            }
        }
        LogicalPlan::Projection { input, exprs } => {
            let exprs = if subtree_has_aggregate(&input) {
                exprs
            } else {
                exprs
                    .into_iter()
                    .map(|(e, n)| (remap_expr(e, remap), n))
                    .collect()
            };
            LogicalPlan::Projection {
                input: Box::new(rewrite(*input, indices, remap)),
                exprs,
            }
        }
        LogicalPlan::Distinct { input } => LogicalPlan::Distinct {
            input: Box::new(rewrite(*input, indices, remap)),
        },
        LogicalPlan::Limit { input, skip, fetch } => LogicalPlan::Limit {
            input: Box::new(rewrite(*input, indices, remap)),
            skip,
            fetch,
        },
        // Unreachable in practice — see the matching arm in
        // `collect_needed_columns`.
        LogicalPlan::Join { .. } | LogicalPlan::SetOp { .. } => plan,
    }
}

fn remap_expr(expr: PlanExpr, remap: &HashMap<usize, usize>) -> PlanExpr {
    match expr {
        PlanExpr::Column { index, name } => PlanExpr::Column {
            index: *remap.get(&index).expect("column collected before rewrite"),
            name,
        },
        PlanExpr::Literal(v) => PlanExpr::Literal(v),
        PlanExpr::UnaryOp { op, expr } => PlanExpr::UnaryOp {
            op,
            expr: Box::new(remap_expr(*expr, remap)),
        },
        PlanExpr::BinaryOp { left, op, right } => PlanExpr::BinaryOp {
            left: Box::new(remap_expr(*left, remap)),
            op,
            right: Box::new(remap_expr(*right, remap)),
        },
        PlanExpr::Function { func, args } => PlanExpr::Function {
            func,
            args: args.into_iter().map(|a| remap_expr(a, remap)).collect(),
        },
        PlanExpr::Case {
            operand,
            branches,
            else_expr,
        } => PlanExpr::Case {
            operand: operand.map(|e| Box::new(remap_expr(*e, remap))),
            branches: branches
                .into_iter()
                .map(|(w, t)| (remap_expr(w, remap), remap_expr(t, remap)))
                .collect(),
            else_expr: else_expr.map(|e| Box::new(remap_expr(*e, remap))),
        },
        PlanExpr::Cast { expr, target } => PlanExpr::Cast {
            expr: Box::new(remap_expr(*expr, remap)),
            target,
        },
        PlanExpr::InList { expr, list } => PlanExpr::InList {
            expr: Box::new(remap_expr(*expr, remap)),
            list: list.into_iter().map(|e| remap_expr(e, remap)).collect(),
        },
        // The nested plan's own indices are a separate space — untouched by
        // the outer rewrite.
        PlanExpr::InSubquery { expr, plan } => PlanExpr::InSubquery {
            expr: Box::new(remap_expr(*expr, remap)),
            plan,
        },
        PlanExpr::Exists(plan) => PlanExpr::Exists(plan),
        PlanExpr::ScalarSubquery(plan) => PlanExpr::ScalarSubquery(plan),
    }
}
