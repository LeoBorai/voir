use std::collections::{BTreeSet, HashMap};

use crate::expr::PlanExpr;
use crate::logical::LogicalPlan;

/// Computes the minimal column set each scan must produce and rewrites the
/// tree so `Scan.projection` reflects it — on CSV this alone skips UTF-8
/// validation and parsing on unreferenced fields.
pub fn optimize(plan: LogicalPlan) -> LogicalPlan {
    let mut needed = BTreeSet::new();
    collect_needed_columns(&plan, &mut needed);
    let indices: Vec<usize> = needed.into_iter().collect();
    let remap: HashMap<usize, usize> = indices.iter().enumerate().map(|(new, &old)| (old, new)).collect();
    rewrite(plan, &indices, &remap)
}

fn collect_needed_columns(plan: &LogicalPlan, needed: &mut BTreeSet<usize>) {
    match plan {
        LogicalPlan::Scan { .. } => {}
        LogicalPlan::Filter { input, predicate } => {
            collect_expr_columns(predicate, needed);
            collect_needed_columns(input, needed);
        }
        LogicalPlan::Projection { input, exprs } => {
            for (expr, _) in exprs {
                collect_expr_columns(expr, needed);
            }
            collect_needed_columns(input, needed);
        }
        LogicalPlan::Limit { input, .. } => collect_needed_columns(input, needed),
    }
}

fn collect_expr_columns(expr: &PlanExpr, needed: &mut BTreeSet<usize>) {
    match expr {
        PlanExpr::Column { index, .. } => {
            needed.insert(*index);
        }
        PlanExpr::Literal(_) => {}
        PlanExpr::UnaryOp { expr, .. } => collect_expr_columns(expr, needed),
        PlanExpr::BinaryOp { left, right, .. } => {
            collect_expr_columns(left, needed);
            collect_expr_columns(right, needed);
        }
    }
}

fn rewrite(plan: LogicalPlan, indices: &[usize], remap: &HashMap<usize, usize>) -> LogicalPlan {
    match plan {
        LogicalPlan::Scan { table_name, source, filters, .. } => {
            let projection = if indices.is_empty() { None } else { Some(indices.to_vec()) };
            LogicalPlan::Scan { table_name, source, projection, filters }
        }
        LogicalPlan::Filter { input, predicate } => LogicalPlan::Filter {
            input: Box::new(rewrite(*input, indices, remap)),
            predicate: remap_expr(predicate, remap),
        },
        LogicalPlan::Projection { input, exprs } => LogicalPlan::Projection {
            input: Box::new(rewrite(*input, indices, remap)),
            exprs: exprs.into_iter().map(|(e, n)| (remap_expr(e, remap), n)).collect(),
        },
        LogicalPlan::Limit { input, fetch } => LogicalPlan::Limit {
            input: Box::new(rewrite(*input, indices, remap)),
            fetch,
        },
    }
}

fn remap_expr(expr: PlanExpr, remap: &HashMap<usize, usize>) -> PlanExpr {
    match expr {
        PlanExpr::Column { index, name } => PlanExpr::Column {
            index: *remap.get(&index).expect("column collected before rewrite"),
            name,
        },
        PlanExpr::Literal(v) => PlanExpr::Literal(v),
        PlanExpr::UnaryOp { op, expr } => PlanExpr::UnaryOp { op, expr: Box::new(remap_expr(*expr, remap)) },
        PlanExpr::BinaryOp { left, op, right } => PlanExpr::BinaryOp {
            left: Box::new(remap_expr(*left, remap)),
            op,
            right: Box::new(remap_expr(*right, remap)),
        },
    }
}
