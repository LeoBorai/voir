#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Query(Box<Query>),
    Explain(Box<Query>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub with: Vec<Cte>,
    pub body: SetExpr,
    pub order_by: Vec<OrderByExpr>,
    pub limit: Option<Expr>,
    pub offset: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cte {
    pub name: String,
    pub query: Box<Query>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SetExpr {
    Select(Box<Select>),
    SetOp {
        op: SetOperator,
        all: bool,
        left: Box<SetExpr>,
        right: Box<SetExpr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetOperator {
    Union,
    Except,
    Intersect,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Select {
    pub distinct: bool,
    pub projection: Vec<SelectItem>,
    pub from: FromClause,
    pub selection: Option<Expr>,
    pub group_by: Vec<Expr>,
    pub having: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrderByExpr {
    pub expr: Expr,
    pub desc: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SelectItem {
    Wildcard,
    QualifiedWildcard(String),
    Expr { expr: Expr, alias: Option<String> },
}

/// A `FROM` item: a leaf table (path-as-table-name, a registered table, or a
/// CTE reference — the binder decides which), a derived subquery, or a join
/// of two `FromClause`s.
#[derive(Debug, Clone, PartialEq)]
pub enum FromClause {
    Table {
        name: String,
        alias: Option<String>,
    },
    Derived {
        subquery: Box<Query>,
        alias: String,
    },
    Join {
        left: Box<FromClause>,
        right: Box<FromClause>,
        join_type: JoinType,
        condition: Option<JoinCondition>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinType {
    Inner,
    Left,
    Right,
    Full,
    Cross,
}

#[derive(Debug, Clone, PartialEq)]
pub enum JoinCondition {
    On(Expr),
    Using(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Ident(String),
    CompoundIdent(String, String),
    Literal(Literal),
    /// Only ever legal as the sole argument of `COUNT(*)` — the parser never
    /// produces it anywhere else.
    Wildcard,
    BinaryOp {
        left: Box<Expr>,
        op: BinaryOperator,
        right: Box<Expr>,
    },
    UnaryOp {
        op: UnaryOperator,
        expr: Box<Expr>,
    },
    Function {
        name: String,
        args: Vec<Expr>,
        distinct: bool,
    },
    Case {
        operand: Option<Box<Expr>>,
        branches: Vec<(Expr, Expr)>,
        else_expr: Option<Box<Expr>>,
    },
    Cast {
        expr: Box<Expr>,
        type_name: String,
    },
    InList {
        expr: Box<Expr>,
        list: Vec<Expr>,
    },
    InSubquery {
        expr: Box<Expr>,
        subquery: Box<Query>,
    },
    Exists {
        subquery: Box<Query>,
    },
    ScalarSubquery(Box<Query>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Null,
    Boolean(bool),
    Int64(i64),
    Float64(f64),
    String(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    Or,
    And,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Plus,
    Minus,
    Multiply,
    Divide,
    Modulo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOperator {
    Not,
    Negate,
    Plus,
}
