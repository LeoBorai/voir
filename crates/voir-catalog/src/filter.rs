use voir_types::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
}

/// A pushdown-friendly predicate, independent of `voir-sql`'s AST — sources
/// only ever see this reduced form, never a `SELECT`.
#[derive(Debug, Clone, PartialEq)]
pub enum FilterExpr {
    Column(usize),
    Literal(Value),
    Compare {
        left: Box<FilterExpr>,
        op: CompareOp,
        right: Box<FilterExpr>,
    },
    And(Box<FilterExpr>, Box<FilterExpr>),
}
