use voir_sql::BinaryOperator;
use voir_types::{DataType, Schema, VoirError};

use crate::expr::{PlanExpr, ScalarFunc};

#[derive(Clone)]
enum ScopeColumn {
    Physical(usize),
    /// A computed reference — currently only `COALESCE(left, right)` for a
    /// `USING` column, which chains correctly across repeated `USING` joins
    /// since `COALESCE(COALESCE(a, b), c)` behaves like `COALESCE(a, b, c)`.
    Computed(Box<PlanExpr>),
}

#[derive(Clone)]
struct ScopeEntry {
    table: Option<String>,
    name: String,
    column: ScopeColumn,
    data_type: DataType,
    /// Included in an unqualified `SELECT *`.
    in_wildcard: bool,
    /// Reachable by an unqualified bare-name reference (`WHERE id = 1`).
    /// `USING` hides the two physical duplicates from this so the merged
    /// entry is the sole unqualified match; qualified access (`t.id`) always
    /// reaches the physical entries regardless of this flag.
    bare_visible: bool,
}

impl ScopeEntry {
    fn expr(&self) -> PlanExpr {
        match &self.column {
            ScopeColumn::Physical(index) => PlanExpr::Column {
                index: *index,
                name: self.name.clone(),
            },
            ScopeColumn::Computed(expr) => (**expr).clone(),
        }
    }
}

/// Bind-time name resolution across one or more joined relations. Unrelated
/// to `LogicalPlan::schema()` — a `Scope` only maps names to `PlanExpr`s and
/// tracks visibility (qualified/unqualified/wildcard); the physical row
/// layout is whatever `LogicalPlan::Join` concatenates (left row ++ right
/// row), which is exactly what `Physical(index)` indexes into.
#[derive(Clone)]
pub struct Scope {
    entries: Vec<ScopeEntry>,
}

impl Scope {
    #[must_use]
    pub fn from_schema(alias: Option<&str>, schema: &Schema) -> Self {
        let entries = schema
            .fields
            .iter()
            .enumerate()
            .map(|(index, field)| ScopeEntry {
                table: alias.map(str::to_string),
                name: field.name.clone(),
                column: ScopeColumn::Physical(index),
                data_type: field.data_type,
                in_wildcard: true,
                bare_visible: true,
            })
            .collect();
        Self { entries }
    }

    fn offset(&mut self, by: usize) {
        for entry in &mut self.entries {
            offset_expr_or_index(&mut entry.column, by);
        }
    }

    /// Concatenates `left` and `right` for a `CROSS`/`ON`-conditioned join —
    /// no columns are hidden or merged.
    #[must_use]
    pub fn concat(left: Scope, mut right: Scope, left_width: usize) -> Scope {
        right.offset(left_width);
        let mut entries = left.entries;
        entries.extend(right.entries);
        Scope { entries }
    }

    /// Concatenates `left` and `right` for a `USING (cols)` join: each
    /// column in `cols` gets a single merged, unqualified `COALESCE(left,
    /// right)` entry; the two physical duplicates remain reachable only via
    /// an explicit table qualifier. Returns the combined scope and the
    /// bound equi-join predicate (`AND`-ed across `cols`).
    ///
    /// # Errors
    ///
    /// Returns an error if a `USING` column doesn't exist, unqualified, on
    /// both sides.
    pub fn join_using(
        left: Scope,
        right: Scope,
        left_width: usize,
        cols: &[String],
    ) -> Result<(Scope, PlanExpr), VoirError> {
        let left_count = left.entries.len();
        let mut right = right;
        right.offset(left_width);
        let mut entries = left.entries;
        entries.extend(right.entries);
        let mut scope = Scope { entries };

        let mut predicate: Option<PlanExpr> = None;
        let mut merged = Vec::new();
        for col in cols {
            let left_pos = scope.position_bare_before(col, left_count)?;
            let right_pos = scope.position_bare_from(col, left_count)?;
            let left_expr = scope.entries[left_pos].expr();
            let right_expr = scope.entries[right_pos].expr();
            scope.entries[left_pos].bare_visible = false;
            scope.entries[left_pos].in_wildcard = false;
            scope.entries[right_pos].bare_visible = false;
            scope.entries[right_pos].in_wildcard = false;

            let data_type = scope.entries[left_pos].data_type;
            let eq = PlanExpr::BinaryOp {
                left: Box::new(left_expr.clone()),
                op: BinaryOperator::Eq,
                right: Box::new(right_expr.clone()),
            };
            predicate = Some(match predicate {
                None => eq,
                Some(p) => PlanExpr::BinaryOp {
                    left: Box::new(p),
                    op: BinaryOperator::And,
                    right: Box::new(eq),
                },
            });
            merged.push(ScopeEntry {
                table: None,
                name: col.clone(),
                column: ScopeColumn::Computed(Box::new(PlanExpr::Function {
                    func: ScalarFunc::Coalesce,
                    args: vec![left_expr, right_expr],
                })),
                data_type,
                in_wildcard: true,
                bare_visible: true,
            });
        }
        for entry in merged.into_iter().rev() {
            scope.entries.insert(0, entry);
        }
        let predicate = predicate.expect("USING requires at least one column");
        Ok((scope, predicate))
    }

    /// Finds the `USING`-eligible entry named `name` among vector positions
    /// `0..boundary` (the left side's entries, before the merge-shifting
    /// `insert(0, ..)` calls below run).
    fn position_bare_before(&self, name: &str, boundary: usize) -> Result<usize, VoirError> {
        self.find_bare_in(name, 0..boundary)
    }

    /// Same, but for vector positions `boundary..` (the right side).
    fn position_bare_from(&self, name: &str, boundary: usize) -> Result<usize, VoirError> {
        self.find_bare_in(name, boundary..self.entries.len())
    }

    fn find_bare_in(&self, name: &str, range: std::ops::Range<usize>) -> Result<usize, VoirError> {
        let matches: Vec<usize> = self.entries[range.clone()]
            .iter()
            .enumerate()
            .filter(|(_, e)| e.bare_visible && e.name.eq_ignore_ascii_case(name))
            .map(|(i, _)| i + range.start)
            .collect();
        match matches.as_slice() {
            [] => Err(VoirError::Plan(format!(
                "column \"{name}\" specified in USING clause does not exist"
            ))),
            [only] => Ok(*only),
            _ => Err(VoirError::Plan(format!(
                "column \"{name}\" specified in USING clause is ambiguous"
            ))),
        }
    }

    /// # Errors
    ///
    /// Returns an error if `name` doesn't resolve to exactly one column
    /// (unqualified) or `table.name` doesn't exist (qualified).
    pub fn resolve(&self, table: Option<&str>, name: &str) -> Result<PlanExpr, VoirError> {
        if let Some(table) = table {
            let matches: Vec<&ScopeEntry> = self
                .entries
                .iter()
                .filter(|e| {
                    e.table
                        .as_deref()
                        .is_some_and(|t| t.eq_ignore_ascii_case(table))
                        && e.name.eq_ignore_ascii_case(name)
                })
                .collect();
            return match matches.as_slice() {
                [only] => Ok(only.expr()),
                [] => Err(VoirError::Plan(format!(
                    "column \"{table}.{name}\" does not exist"
                ))),
                _ => Err(VoirError::Plan(format!(
                    "column \"{table}.{name}\" is ambiguous"
                ))),
            };
        }
        let matches: Vec<&ScopeEntry> = self
            .entries
            .iter()
            .filter(|e| e.bare_visible && e.name.eq_ignore_ascii_case(name))
            .collect();
        match matches.as_slice() {
            [only] => Ok(only.expr()),
            [] => Err(VoirError::Plan(format!("column \"{name}\" does not exist"))),
            multiple => {
                let tables: Vec<String> = multiple
                    .iter()
                    .map(|e| {
                        e.table
                            .clone()
                            .map_or_else(|| name.to_string(), |t| format!("{t}.{name}"))
                    })
                    .collect();
                Err(VoirError::Plan(format!(
                    "column \"{name}\" is ambiguous: could be {}",
                    tables.join(" or ")
                )))
            }
        }
    }

    /// # Errors
    ///
    /// Returns an error if `table` is `Some` and no column carries that
    /// qualifier.
    pub fn wildcard(&self, table: Option<&str>) -> Result<Vec<(PlanExpr, String)>, VoirError> {
        let items: Vec<(PlanExpr, String)> = match table {
            Some(table) => self
                .entries
                .iter()
                .filter(|e| {
                    e.table
                        .as_deref()
                        .is_some_and(|t| t.eq_ignore_ascii_case(table))
                })
                .map(|e| (e.expr(), e.name.clone()))
                .collect(),
            None => self
                .entries
                .iter()
                .filter(|e| e.in_wildcard)
                .map(|e| (e.expr(), e.name.clone()))
                .collect(),
        };
        if items.is_empty() {
            if let Some(table) = table {
                return Err(VoirError::Plan(format!(
                    "no table or alias named \"{table}\" in FROM clause"
                )));
            }
        }
        Ok(items)
    }
}

fn offset_expr_or_index(column: &mut ScopeColumn, by: usize) {
    match column {
        ScopeColumn::Physical(index) => *index += by,
        ScopeColumn::Computed(expr) => offset_plan_expr(expr, by),
    }
}

fn offset_plan_expr(expr: &mut PlanExpr, by: usize) {
    match expr {
        PlanExpr::Column { index, .. } => *index += by,
        PlanExpr::Function { args, .. } => args.iter_mut().for_each(|a| offset_plan_expr(a, by)),
        PlanExpr::BinaryOp { left, right, .. } => {
            offset_plan_expr(left, by);
            offset_plan_expr(right, by);
        }
        PlanExpr::UnaryOp { expr, .. } | PlanExpr::Cast { expr, .. } => offset_plan_expr(expr, by),
        _ => {}
    }
}
