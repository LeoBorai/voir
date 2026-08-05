//! Token-to-AST parsing. Grammar is expressed as recursive-descent functions
//! over `&[Token]`, using `nom`'s `alt`/`opt`/`value` combinators (via the
//! `token()` primitive below) wherever a production is a genuine choice
//! between alternatives; purely sequential/mandatory productions just thread
//! `Input` through by hand, same as the combinators would under the hood.
//!
//! Precedence (low to high): OR, AND, NOT (prefix), comparisons/IN, `+ -`,
//! `* / %`, unary `+ -`. Each level below is `next (OP next)*`, which is the
//! recursive-descent shape of that same table.
//!
//! `SetExpr` precedence (low to high): UNION/EXCEPT, INTERSECT — matching
//! Postgres, where INTERSECT binds tighter.

use nom::Parser;
use nom::branch::alt;
use nom::combinator::{opt, value};

use voir_types::VoirError;

use crate::ast::{
    BinaryOperator, Cte, Expr, FromClause, JoinCondition, JoinType, Literal, OrderByExpr, Query,
    Select, SelectItem, SetExpr, SetOperator, Statement, UnaryOperator,
};
use crate::token::{Token, TokenKind, describe};

type Input<'a> = &'a [Token];
type PResult<'a, T> = nom::IResult<Input<'a>, T, nom::error::Error<Input<'a>>>;

/// # Errors
///
/// Returns an error if `sql` fails to lex or does not parse as a valid statement.
pub fn parse(sql: &str) -> Result<Statement, VoirError> {
    let tokens = crate::lexer::tokenize(sql)?;
    let input: Input<'_> = &tokens;
    let (input, statement) = parse_statement(input)?;
    finish(input)?;
    Ok(statement)
}

fn advance(input: Input<'_>) -> (Input<'_>, Token) {
    let tok = input[0].clone();
    if matches!(tok.kind, TokenKind::Eof) {
        (input, tok)
    } else {
        (&input[1..], tok)
    }
}

fn is_kind(kind: &TokenKind, want: &TokenKind) -> bool {
    std::mem::discriminant(kind) == std::mem::discriminant(want)
}

fn peek_kind(input: Input<'_>, offset: usize) -> Option<&TokenKind> {
    input.get(offset).map(|t| &t.kind)
}

/// A `nom`-`Parser`-compatible primitive: consumes one token when its kind
/// matches `want` (the payload of `want`, if any, is never inspected — only
/// the discriminant is compared).
fn token<'a>(want: TokenKind) -> impl FnMut(Input<'a>) -> PResult<'a, Token> {
    move |input: Input<'a>| {
        if is_kind(&input[0].kind, &want) {
            Ok(advance(input))
        } else {
            Err(nom::Err::Error(nom::error::Error::new(
                input,
                nom::error::ErrorKind::Tag,
            )))
        }
    }
}

fn opt_token(input: Input<'_>, want: TokenKind) -> (Input<'_>, bool) {
    let (input, matched) = opt(token(want)).parse(input).expect("opt() never fails");
    (input, matched.is_some())
}

fn expect<'a>(
    input: Input<'a>,
    want: &TokenKind,
    expected: &str,
) -> Result<(Input<'a>, Token), VoirError> {
    if is_kind(&input[0].kind, want) {
        Ok(advance(input))
    } else {
        Err(err_at(input, expected))
    }
}

fn err_at(input: Input<'_>, expected: &str) -> VoirError {
    let tok = &input[0];
    VoirError::Parse {
        message: format!("expected {expected}, found {}", describe(&tok.kind)),
        span: tok.span,
    }
}

fn starts_subquery(input: Input<'_>) -> bool {
    matches!(
        peek_kind(input, 0),
        Some(TokenKind::Select | TokenKind::With)
    )
}

fn parse_statement(input: Input<'_>) -> Result<(Input<'_>, Statement), VoirError> {
    let (input, is_explain) = opt_token(input, TokenKind::Explain);
    let (input, query) = parse_query(input)?;
    let statement = if is_explain {
        Statement::Explain(Box::new(query))
    } else {
        Statement::Query(Box::new(query))
    };
    Ok((input, statement))
}

fn finish(input: Input<'_>) -> Result<(), VoirError> {
    let (input, _) = opt_token(input, TokenKind::Semicolon);
    if matches!(input[0].kind, TokenKind::Eof) {
        Ok(())
    } else {
        Err(err_at(input, "end of statement"))
    }
}

fn parse_query(input: Input<'_>) -> Result<(Input<'_>, Query), VoirError> {
    let (input, with) = parse_with_clause(input)?;
    let (input, body) = parse_set_expr(input)?;

    let (input, has_order) = opt_token(input, TokenKind::Order);
    let (input, order_by) = if has_order {
        let (input, _) = expect(input, &TokenKind::By, "BY")?;
        parse_order_by_list(input)?
    } else {
        (input, Vec::new())
    };

    let (input, has_limit) = opt_token(input, TokenKind::Limit);
    let (input, limit) = if has_limit {
        let (input, expr) = parse_expr(input)?;
        (input, Some(expr))
    } else {
        (input, None)
    };

    let (input, has_offset) = opt_token(input, TokenKind::Offset);
    let (input, offset) = if has_offset {
        let (input, expr) = parse_expr(input)?;
        (input, Some(expr))
    } else {
        (input, None)
    };

    Ok((
        input,
        Query {
            with,
            body,
            order_by,
            limit,
            offset,
        },
    ))
}

fn parse_with_clause(input: Input<'_>) -> Result<(Input<'_>, Vec<Cte>), VoirError> {
    let (input, has_with) = opt_token(input, TokenKind::With);
    if !has_with {
        return Ok((input, Vec::new()));
    }
    let (mut input, first) = parse_cte(input)?;
    let mut ctes = vec![first];
    loop {
        let (next, has_comma) = opt_token(input, TokenKind::Comma);
        if !has_comma {
            break;
        }
        let (next, cte) = parse_cte(next)?;
        ctes.push(cte);
        input = next;
    }
    Ok((input, ctes))
}

fn parse_cte(input: Input<'_>) -> Result<(Input<'_>, Cte), VoirError> {
    let (input, name) = parse_ident(input)?;
    let (input, _) = expect(input, &TokenKind::As, "AS")?;
    let (input, _) = expect(input, &TokenKind::LParen, "'('")?;
    let (input, query) = parse_query(input)?;
    let (input, _) = expect(input, &TokenKind::RParen, "')'")?;
    Ok((
        input,
        Cte {
            name,
            query: Box::new(query),
        },
    ))
}

fn parse_set_expr(input: Input<'_>) -> Result<(Input<'_>, SetExpr), VoirError> {
    let (mut input, mut lhs) = parse_intersect_expr(input)?;
    loop {
        let op = match peek_kind(input, 0) {
            Some(TokenKind::Union) => SetOperator::Union,
            Some(TokenKind::Except) => SetOperator::Except,
            _ => break,
        };
        let (next, _) = advance(input);
        let (next, all) = opt_token(next, TokenKind::All);
        let (next, rhs) = parse_intersect_expr(next)?;
        lhs = SetExpr::SetOp {
            op,
            all,
            left: Box::new(lhs),
            right: Box::new(rhs),
        };
        input = next;
    }
    Ok((input, lhs))
}

fn parse_intersect_expr(input: Input<'_>) -> Result<(Input<'_>, SetExpr), VoirError> {
    let (mut input, mut lhs) = parse_set_primary(input)?;
    loop {
        if !matches!(peek_kind(input, 0), Some(TokenKind::Intersect)) {
            break;
        }
        let (next, _) = advance(input);
        let (next, all) = opt_token(next, TokenKind::All);
        let (next, rhs) = parse_set_primary(next)?;
        lhs = SetExpr::SetOp {
            op: SetOperator::Intersect,
            all,
            left: Box::new(lhs),
            right: Box::new(rhs),
        };
        input = next;
    }
    Ok((input, lhs))
}

fn parse_set_primary(input: Input<'_>) -> Result<(Input<'_>, SetExpr), VoirError> {
    if matches!(peek_kind(input, 0), Some(TokenKind::LParen)) {
        let (input, _) = advance(input);
        let (input, inner) = parse_set_expr(input)?;
        let (input, _) = expect(input, &TokenKind::RParen, "')'")?;
        Ok((input, inner))
    } else {
        let (input, select) = parse_select_core(input)?;
        Ok((input, SetExpr::Select(Box::new(select))))
    }
}

fn parse_select_core(input: Input<'_>) -> Result<(Input<'_>, Select), VoirError> {
    let (input, _) = expect(input, &TokenKind::Select, "SELECT")?;
    let (input, distinct) = opt_token(input, TokenKind::Distinct);
    let (input, projection) = parse_select_list(input)?;
    let (input, _) = expect(input, &TokenKind::From, "FROM")?;
    let (input, from) = parse_from_clause(input)?;

    let (input, has_where) = opt_token(input, TokenKind::Where);
    let (input, selection) = if has_where {
        let (input, expr) = parse_expr(input)?;
        (input, Some(expr))
    } else {
        (input, None)
    };

    let (input, has_group) = opt_token(input, TokenKind::Group);
    let (input, group_by) = if has_group {
        let (input, _) = expect(input, &TokenKind::By, "BY")?;
        parse_expr_list(input)?
    } else {
        (input, Vec::new())
    };

    let (input, has_having) = opt_token(input, TokenKind::Having);
    let (input, having) = if has_having {
        let (input, expr) = parse_expr(input)?;
        (input, Some(expr))
    } else {
        (input, None)
    };

    Ok((
        input,
        Select {
            distinct,
            projection,
            from,
            selection,
            group_by,
            having,
        },
    ))
}

fn parse_select_list(input: Input<'_>) -> Result<(Input<'_>, Vec<SelectItem>), VoirError> {
    let (mut input, first) = parse_select_item(input)?;
    let mut items = vec![first];
    loop {
        let (next, has_comma) = opt_token(input, TokenKind::Comma);
        if !has_comma {
            break;
        }
        let (next, item) = parse_select_item(next)?;
        items.push(item);
        input = next;
    }
    Ok((input, items))
}

fn parse_expr_list(input: Input<'_>) -> Result<(Input<'_>, Vec<Expr>), VoirError> {
    let (mut input, first) = parse_expr(input)?;
    let mut exprs = vec![first];
    loop {
        let (next, has_comma) = opt_token(input, TokenKind::Comma);
        if !has_comma {
            break;
        }
        let (next, expr) = parse_expr(next)?;
        exprs.push(expr);
        input = next;
    }
    Ok((input, exprs))
}

fn parse_order_by_list(input: Input<'_>) -> Result<(Input<'_>, Vec<OrderByExpr>), VoirError> {
    let (mut input, first) = parse_order_by_item(input)?;
    let mut items = vec![first];
    loop {
        let (next, has_comma) = opt_token(input, TokenKind::Comma);
        if !has_comma {
            break;
        }
        let (next, item) = parse_order_by_item(next)?;
        items.push(item);
        input = next;
    }
    Ok((input, items))
}

fn parse_order_by_item(input: Input<'_>) -> Result<(Input<'_>, OrderByExpr), VoirError> {
    let (input, expr) = parse_expr(input)?;
    let (input, desc) = opt_token(input, TokenKind::Desc);
    let input = if desc {
        input
    } else {
        opt_token(input, TokenKind::Asc).0
    };
    Ok((input, OrderByExpr { expr, desc }))
}

fn parse_select_item(input: Input<'_>) -> Result<(Input<'_>, SelectItem), VoirError> {
    let (input, is_wildcard) = opt_token(input, TokenKind::Star);
    if is_wildcard {
        return Ok((input, SelectItem::Wildcard));
    }
    if let Some(TokenKind::Ident(name) | TokenKind::QuotedIdent(name)) = peek_kind(input, 0) {
        if matches!(peek_kind(input, 1), Some(TokenKind::Dot))
            && matches!(peek_kind(input, 2), Some(TokenKind::Star))
        {
            let name = name.clone();
            return Ok((&input[3..], SelectItem::QualifiedWildcard(name)));
        }
    }
    let (input, expr) = parse_expr(input)?;
    let (input, has_as) = opt_token(input, TokenKind::As);
    let (input, alias) = if has_as {
        let (input, name) = parse_ident(input)?;
        (input, Some(name))
    } else {
        (input, None)
    };
    Ok((input, SelectItem::Expr { expr, alias }))
}

fn parse_from_clause(input: Input<'_>) -> Result<(Input<'_>, FromClause), VoirError> {
    let (mut input, mut left) = parse_from_operand(input)?;
    loop {
        let (next, has_comma) = opt_token(input, TokenKind::Comma);
        if has_comma {
            let (next, right) = parse_from_operand(next)?;
            left = FromClause::Join {
                left: Box::new(left),
                right: Box::new(right),
                join_type: JoinType::Cross,
                condition: None,
            };
            input = next;
            continue;
        }
        if peek_join_type(input) {
            let (next, join_type, needs_condition) = parse_join_head(input)?;
            let (next, right) = parse_from_operand(next)?;
            let (next, condition) = if needs_condition {
                parse_join_condition(next)?
            } else {
                (next, None)
            };
            left = FromClause::Join {
                left: Box::new(left),
                right: Box::new(right),
                join_type,
                condition,
            };
            input = next;
            continue;
        }
        break;
    }
    Ok((input, left))
}

fn peek_join_type(input: Input<'_>) -> bool {
    matches!(
        peek_kind(input, 0),
        Some(
            TokenKind::Join
                | TokenKind::Inner
                | TokenKind::Left
                | TokenKind::Right
                | TokenKind::Full
                | TokenKind::Cross
        )
    )
}

fn parse_join_head(input: Input<'_>) -> Result<(Input<'_>, JoinType, bool), VoirError> {
    let (input, join_type) = match peek_kind(input, 0) {
        Some(TokenKind::Inner) => (advance(input).0, JoinType::Inner),
        Some(TokenKind::Left) => (advance(input).0, JoinType::Left),
        Some(TokenKind::Right) => (advance(input).0, JoinType::Right),
        Some(TokenKind::Full) => (advance(input).0, JoinType::Full),
        Some(TokenKind::Cross) => (advance(input).0, JoinType::Cross),
        _ => (input, JoinType::Inner), // bare JOIN
    };
    let (input, _) = opt_token(input, TokenKind::Outer);
    let (input, _) = expect(input, &TokenKind::Join, "JOIN")?;
    let needs_condition = !matches!(join_type, JoinType::Cross);
    Ok((input, join_type, needs_condition))
}

fn parse_join_condition(input: Input<'_>) -> Result<(Input<'_>, Option<JoinCondition>), VoirError> {
    match peek_kind(input, 0) {
        Some(TokenKind::On) => {
            let (input, _) = advance(input);
            let (input, expr) = parse_expr(input)?;
            Ok((input, Some(JoinCondition::On(expr))))
        }
        Some(TokenKind::Using) => {
            let (input, _) = advance(input);
            let (input, _) = expect(input, &TokenKind::LParen, "'('")?;
            let (mut input, first) = parse_ident(input)?;
            let mut cols = vec![first];
            loop {
                let (next, has_comma) = opt_token(input, TokenKind::Comma);
                if !has_comma {
                    break;
                }
                let (next, col) = parse_ident(next)?;
                cols.push(col);
                input = next;
            }
            let (input, _) = expect(input, &TokenKind::RParen, "')'")?;
            Ok((input, Some(JoinCondition::Using(cols))))
        }
        _ => Err(err_at(input, "ON or USING")),
    }
}

fn parse_from_operand(input: Input<'_>) -> Result<(Input<'_>, FromClause), VoirError> {
    if matches!(peek_kind(input, 0), Some(TokenKind::LParen)) {
        if starts_subquery(&input[1..]) {
            let (input, _) = advance(input);
            let (input, subquery) = parse_query(input)?;
            let (input, _) = expect(input, &TokenKind::RParen, "')'")?;
            let (input, _) = opt_token(input, TokenKind::As);
            let (input, alias) = parse_ident(input)?;
            Ok((
                input,
                FromClause::Derived {
                    subquery: Box::new(subquery),
                    alias,
                },
            ))
        } else {
            let (input, _) = advance(input);
            let (input, inner) = parse_from_clause(input)?;
            let (input, _) = expect(input, &TokenKind::RParen, "')'")?;
            Ok((input, inner))
        }
    } else {
        parse_table_from(input)
    }
}

fn parse_table_from(input: Input<'_>) -> Result<(Input<'_>, FromClause), VoirError> {
    let (input, tok) = advance(input);
    let name = match tok.kind {
        TokenKind::StringLiteral(s) | TokenKind::Ident(s) | TokenKind::QuotedIdent(s) => s,
        other => {
            return Err(VoirError::Parse {
                message: format!("expected table reference, found {}", describe(&other)),
                span: tok.span,
            });
        }
    };
    let (input, has_as) = opt_token(input, TokenKind::As);
    let (input, alias) = if has_as || matches!(peek_kind(input, 0), Some(TokenKind::Ident(_))) {
        let (input, name) = parse_ident(input)?;
        (input, Some(name))
    } else {
        (input, None)
    };
    Ok((input, FromClause::Table { name, alias }))
}

fn parse_ident(input: Input<'_>) -> Result<(Input<'_>, String), VoirError> {
    let (input, tok) = advance(input);
    match tok.kind {
        TokenKind::Ident(s) | TokenKind::QuotedIdent(s) => Ok((input, s)),
        other => Err(VoirError::Parse {
            message: format!("expected identifier, found {}", describe(&other)),
            span: tok.span,
        }),
    }
}

fn parse_expr(input: Input<'_>) -> Result<(Input<'_>, Expr), VoirError> {
    parse_or_expr(input)
}

fn parse_or_expr(input: Input<'_>) -> Result<(Input<'_>, Expr), VoirError> {
    let (mut input, mut lhs) = parse_and_expr(input)?;
    loop {
        let (next, has_or) = opt_token(input, TokenKind::Or);
        if !has_or {
            break;
        }
        let (next, rhs) = parse_and_expr(next)?;
        lhs = Expr::BinaryOp {
            left: Box::new(lhs),
            op: BinaryOperator::Or,
            right: Box::new(rhs),
        };
        input = next;
    }
    Ok((input, lhs))
}

fn parse_and_expr(input: Input<'_>) -> Result<(Input<'_>, Expr), VoirError> {
    let (mut input, mut lhs) = parse_not_expr(input)?;
    loop {
        let (next, has_and) = opt_token(input, TokenKind::And);
        if !has_and {
            break;
        }
        let (next, rhs) = parse_not_expr(next)?;
        lhs = Expr::BinaryOp {
            left: Box::new(lhs),
            op: BinaryOperator::And,
            right: Box::new(rhs),
        };
        input = next;
    }
    Ok((input, lhs))
}

fn parse_not_expr(input: Input<'_>) -> Result<(Input<'_>, Expr), VoirError> {
    let (next, has_not) = opt_token(input, TokenKind::Not);
    if has_not {
        let (input, expr) = parse_not_expr(next)?;
        Ok((
            input,
            Expr::UnaryOp {
                op: UnaryOperator::Not,
                expr: Box::new(expr),
            },
        ))
    } else {
        parse_comparison_expr(input)
    }
}

fn comparison_op(input: Input<'_>) -> PResult<'_, BinaryOperator> {
    alt((
        value(BinaryOperator::Eq, token(TokenKind::Eq)),
        value(BinaryOperator::NotEq, token(TokenKind::NotEq)),
        value(BinaryOperator::LtEq, token(TokenKind::LtEq)),
        value(BinaryOperator::Lt, token(TokenKind::Lt)),
        value(BinaryOperator::GtEq, token(TokenKind::GtEq)),
        value(BinaryOperator::Gt, token(TokenKind::Gt)),
    ))
    .parse(input)
}

fn parse_comparison_expr(input: Input<'_>) -> Result<(Input<'_>, Expr), VoirError> {
    let (mut input, mut lhs) = parse_additive_expr(input)?;
    loop {
        if let Ok((next, op)) = comparison_op(input) {
            let (next, rhs) = parse_additive_expr(next)?;
            lhs = Expr::BinaryOp {
                left: Box::new(lhs),
                op,
                right: Box::new(rhs),
            };
            input = next;
            continue;
        }
        let (lookahead, negated) = opt_token(input, TokenKind::Not);
        if matches!(peek_kind(lookahead, 0), Some(TokenKind::In)) {
            let (next, _) = advance(lookahead);
            let (next, in_expr) = parse_in_rhs(next, lhs)?;
            lhs = if negated {
                Expr::UnaryOp {
                    op: UnaryOperator::Not,
                    expr: Box::new(in_expr),
                }
            } else {
                in_expr
            };
            input = next;
            continue;
        }
        break;
    }
    Ok((input, lhs))
}

fn parse_in_rhs(input: Input<'_>, lhs: Expr) -> Result<(Input<'_>, Expr), VoirError> {
    let (input, _) = expect(input, &TokenKind::LParen, "'('")?;
    if starts_subquery(input) {
        let (input, subquery) = parse_query(input)?;
        let (input, _) = expect(input, &TokenKind::RParen, "')'")?;
        Ok((
            input,
            Expr::InSubquery {
                expr: Box::new(lhs),
                subquery: Box::new(subquery),
            },
        ))
    } else {
        let (mut input, first) = parse_expr(input)?;
        let mut list = vec![first];
        loop {
            let (next, has_comma) = opt_token(input, TokenKind::Comma);
            if !has_comma {
                break;
            }
            let (next, e) = parse_expr(next)?;
            list.push(e);
            input = next;
        }
        let (input, _) = expect(input, &TokenKind::RParen, "')'")?;
        Ok((
            input,
            Expr::InList {
                expr: Box::new(lhs),
                list,
            },
        ))
    }
}

fn additive_op(input: Input<'_>) -> PResult<'_, BinaryOperator> {
    alt((
        value(BinaryOperator::Plus, token(TokenKind::Plus)),
        value(BinaryOperator::Minus, token(TokenKind::Minus)),
    ))
    .parse(input)
}

fn parse_additive_expr(input: Input<'_>) -> Result<(Input<'_>, Expr), VoirError> {
    let (mut input, mut lhs) = parse_multiplicative_expr(input)?;
    while let Ok((next, op)) = additive_op(input) {
        let (next, rhs) = parse_multiplicative_expr(next)?;
        lhs = Expr::BinaryOp {
            left: Box::new(lhs),
            op,
            right: Box::new(rhs),
        };
        input = next;
    }
    Ok((input, lhs))
}

fn multiplicative_op(input: Input<'_>) -> PResult<'_, BinaryOperator> {
    alt((
        value(BinaryOperator::Multiply, token(TokenKind::Star)),
        value(BinaryOperator::Divide, token(TokenKind::Slash)),
        value(BinaryOperator::Modulo, token(TokenKind::Percent)),
    ))
    .parse(input)
}

fn parse_multiplicative_expr(input: Input<'_>) -> Result<(Input<'_>, Expr), VoirError> {
    let (mut input, mut lhs) = parse_unary_expr(input)?;
    while let Ok((next, op)) = multiplicative_op(input) {
        let (next, rhs) = parse_unary_expr(next)?;
        lhs = Expr::BinaryOp {
            left: Box::new(lhs),
            op,
            right: Box::new(rhs),
        };
        input = next;
    }
    Ok((input, lhs))
}

fn parse_unary_expr(input: Input<'_>) -> Result<(Input<'_>, Expr), VoirError> {
    let (next, is_minus) = opt_token(input, TokenKind::Minus);
    if is_minus {
        let (input, expr) = parse_unary_expr(next)?;
        return Ok((
            input,
            Expr::UnaryOp {
                op: UnaryOperator::Negate,
                expr: Box::new(expr),
            },
        ));
    }
    let (next, is_plus) = opt_token(input, TokenKind::Plus);
    if is_plus {
        let (input, expr) = parse_unary_expr(next)?;
        return Ok((
            input,
            Expr::UnaryOp {
                op: UnaryOperator::Plus,
                expr: Box::new(expr),
            },
        ));
    }
    parse_primary_expr(input)
}

fn literal_expr(input: Input<'_>) -> PResult<'_, Expr> {
    alt((
        value(Expr::Literal(Literal::Null), token(TokenKind::Null)),
        value(
            Expr::Literal(Literal::Boolean(true)),
            token(TokenKind::True),
        ),
        value(
            Expr::Literal(Literal::Boolean(false)),
            token(TokenKind::False),
        ),
    ))
    .parse(input)
}

fn parse_primary_expr(input: Input<'_>) -> Result<(Input<'_>, Expr), VoirError> {
    if let Ok((next, expr)) = literal_expr(input) {
        return Ok((next, expr));
    }
    match &input[0].kind {
        TokenKind::Number(_) => {
            let (input, tok) = advance(input);
            let TokenKind::Number(text) = tok.kind else {
                unreachable!("matched above")
            };
            Ok((input, parse_number_literal(&text)))
        }
        TokenKind::StringLiteral(_) => {
            let (input, tok) = advance(input);
            let TokenKind::StringLiteral(s) = tok.kind else {
                unreachable!("matched above")
            };
            Ok((input, Expr::Literal(Literal::String(s))))
        }
        TokenKind::Exists => {
            let (input, _) = advance(input);
            let (input, _) = expect(input, &TokenKind::LParen, "'('")?;
            let (input, subquery) = parse_query(input)?;
            let (input, _) = expect(input, &TokenKind::RParen, "')'")?;
            Ok((
                input,
                Expr::Exists {
                    subquery: Box::new(subquery),
                },
            ))
        }
        TokenKind::LParen => {
            if starts_subquery(&input[1..]) {
                let (input, _) = advance(input);
                let (input, subquery) = parse_query(input)?;
                let (input, _) = expect(input, &TokenKind::RParen, "')'")?;
                Ok((input, Expr::ScalarSubquery(Box::new(subquery))))
            } else {
                let (input, _) = advance(input);
                let (input, expr) = parse_expr(input)?;
                let (input, _) = expect(input, &TokenKind::RParen, "')'")?;
                Ok((input, expr))
            }
        }
        TokenKind::Case => {
            let (input, _) = advance(input);
            parse_case(input)
        }
        TokenKind::Cast => {
            let (input, _) = advance(input);
            parse_cast(input)
        }
        TokenKind::Ident(_) | TokenKind::QuotedIdent(_) => {
            let (input, tok) = advance(input);
            let (TokenKind::Ident(name) | TokenKind::QuotedIdent(name)) = tok.kind else {
                unreachable!("matched above")
            };
            if matches!(input[0].kind, TokenKind::LParen) {
                parse_function_call(input, name)
            } else if matches!(input[0].kind, TokenKind::Dot) {
                let (input, _) = advance(input);
                let (input, col) = parse_ident(input)?;
                Ok((input, Expr::CompoundIdent(name, col)))
            } else {
                Ok((input, Expr::Ident(name)))
            }
        }
        _ => Err(err_at(input, "expression")),
    }
}

fn parse_function_call(input: Input<'_>, name: String) -> Result<(Input<'_>, Expr), VoirError> {
    let (input, _) = expect(input, &TokenKind::LParen, "'('")?;
    let (input, distinct) = opt_token(input, TokenKind::Distinct);

    let (input, is_wildcard) = opt_token(input, TokenKind::Star);
    let (input, args) = if is_wildcard {
        (input, vec![Expr::Wildcard])
    } else if matches!(input[0].kind, TokenKind::RParen) {
        (input, Vec::new())
    } else {
        let (mut input, first) = parse_expr(input)?;
        let mut args = vec![first];
        loop {
            let (next, has_comma) = opt_token(input, TokenKind::Comma);
            if !has_comma {
                break;
            }
            let (next, arg) = parse_expr(next)?;
            args.push(arg);
            input = next;
        }
        (input, args)
    };

    let (input, _) = expect(input, &TokenKind::RParen, "')'")?;
    Ok((
        input,
        Expr::Function {
            name,
            args,
            distinct,
        },
    ))
}

fn parse_case(input: Input<'_>) -> Result<(Input<'_>, Expr), VoirError> {
    let (input, operand) = if matches!(input[0].kind, TokenKind::When) {
        (input, None)
    } else {
        let (input, expr) = parse_expr(input)?;
        (input, Some(Box::new(expr)))
    };

    let (mut input, first_branch) = parse_case_branch(input)?;
    let mut branches = vec![first_branch];
    loop {
        if !matches!(input[0].kind, TokenKind::When) {
            break;
        }
        let (next, branch) = parse_case_branch(input)?;
        branches.push(branch);
        input = next;
    }

    let (input, has_else) = opt_token(input, TokenKind::Else);
    let (input, else_expr) = if has_else {
        let (input, expr) = parse_expr(input)?;
        (input, Some(Box::new(expr)))
    } else {
        (input, None)
    };

    let (input, _) = expect(input, &TokenKind::End, "END")?;
    Ok((
        input,
        Expr::Case {
            operand,
            branches,
            else_expr,
        },
    ))
}

fn parse_case_branch(input: Input<'_>) -> Result<(Input<'_>, (Expr, Expr)), VoirError> {
    let (input, _) = expect(input, &TokenKind::When, "WHEN")?;
    let (input, when) = parse_expr(input)?;
    let (input, _) = expect(input, &TokenKind::Then, "THEN")?;
    let (input, then) = parse_expr(input)?;
    Ok((input, (when, then)))
}

fn parse_cast(input: Input<'_>) -> Result<(Input<'_>, Expr), VoirError> {
    let (input, _) = expect(input, &TokenKind::LParen, "'('")?;
    let (input, expr) = parse_expr(input)?;
    let (input, _) = expect(input, &TokenKind::As, "AS")?;
    let (input, type_name) = parse_type_name(input)?;
    let (input, _) = expect(input, &TokenKind::RParen, "')'")?;
    Ok((
        input,
        Expr::Cast {
            expr: Box::new(expr),
            type_name,
        },
    ))
}

fn parse_type_name(input: Input<'_>) -> Result<(Input<'_>, String), VoirError> {
    let (input, tok) = advance(input);
    match tok.kind {
        TokenKind::Ident(s) => Ok((input, s)),
        other => Err(VoirError::Parse {
            message: format!("expected a type name, found {}", describe(&other)),
            span: tok.span,
        }),
    }
}

fn parse_number_literal(text: &str) -> Expr {
    if text.contains('.') || text.contains('e') || text.contains('E') {
        Expr::Literal(Literal::Float64(text.parse().unwrap_or(f64::NAN)))
    } else {
        match text.parse::<i64>() {
            Ok(v) => Expr::Literal(Literal::Int64(v)),
            Err(_) => Expr::Literal(Literal::Float64(text.parse().unwrap_or(f64::NAN))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn as_select(query: &Query) -> &Select {
        let SetExpr::Select(select) = &query.body else {
            panic!("expected a plain SELECT body")
        };
        select
    }

    fn table_name(from: &FromClause) -> &str {
        let FromClause::Table { name, .. } = from else {
            panic!("expected a leaf table reference")
        };
        name
    }

    #[test]
    fn parses_simple_select() {
        let stmt = parse("SELECT a, b FROM 'sales.csv' WHERE a = 1 LIMIT 10").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let select = as_select(&query);
        assert_eq!(select.projection.len(), 2);
        assert_eq!(table_name(&select.from), "sales.csv");
        assert!(select.selection.is_some());
        assert!(query.limit.is_some());
    }

    #[test]
    fn parses_wildcard() {
        let stmt = parse("SELECT * FROM 'sales.csv'").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        assert_eq!(as_select(&query).projection, vec![SelectItem::Wildcard]);
    }

    #[test]
    fn and_binds_tighter_than_or() {
        let stmt = parse("SELECT * FROM 't.csv' WHERE a = 1 OR b = 2 AND c = 3").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let Some(Expr::BinaryOp {
            op: BinaryOperator::Or,
            right,
            ..
        }) = &as_select(&query).selection
        else {
            panic!("expected top-level OR")
        };
        assert!(matches!(
            **right,
            Expr::BinaryOp {
                op: BinaryOperator::And,
                ..
            }
        ));
    }

    #[test]
    fn not_binds_looser_than_comparison_tighter_than_and() {
        let stmt = parse("SELECT * FROM 't.csv' WHERE NOT a = 1 AND b = 2").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let Some(Expr::BinaryOp {
            op: BinaryOperator::And,
            left,
            ..
        }) = &as_select(&query).selection
        else {
            panic!("expected top-level AND")
        };
        assert!(matches!(
            **left,
            Expr::UnaryOp {
                op: UnaryOperator::Not,
                ..
            }
        ));
    }

    #[test]
    fn double_not_is_preserved_as_nested_unary_ops() {
        let stmt = parse("SELECT * FROM 't.csv' WHERE NOT NOT a").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let Some(Expr::UnaryOp {
            op: UnaryOperator::Not,
            expr: outer,
        }) = &as_select(&query).selection
        else {
            panic!("expected outer NOT")
        };
        assert!(matches!(
            **outer,
            Expr::UnaryOp {
                op: UnaryOperator::Not,
                ..
            }
        ));
    }

    #[test]
    fn unary_minus_binds_tighter_than_multiply() {
        let stmt = parse("SELECT -2 * 3 FROM 't.csv'").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let SelectItem::Expr { expr, .. } = &as_select(&query).projection[0] else {
            panic!("expected expr")
        };
        let Expr::BinaryOp {
            op: BinaryOperator::Multiply,
            left,
            ..
        } = expr
        else {
            panic!("expected top-level multiply")
        };
        assert!(matches!(
            **left,
            Expr::UnaryOp {
                op: UnaryOperator::Negate,
                ..
            }
        ));
    }

    #[test]
    fn unary_plus_is_preserved() {
        let stmt = parse("SELECT +1 FROM 't.csv'").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let SelectItem::Expr { expr, .. } = &as_select(&query).projection[0] else {
            panic!("expected expr")
        };
        assert!(matches!(
            expr,
            Expr::UnaryOp {
                op: UnaryOperator::Plus,
                ..
            }
        ));
    }

    #[test]
    fn parenthesized_expression_overrides_precedence() {
        let stmt = parse("SELECT (1 + 2) * 3 FROM 't.csv'").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let SelectItem::Expr { expr, .. } = &as_select(&query).projection[0] else {
            panic!("expected expr")
        };
        let Expr::BinaryOp {
            op: BinaryOperator::Multiply,
            left,
            ..
        } = expr
        else {
            panic!("expected top-level multiply")
        };
        assert!(matches!(
            **left,
            Expr::BinaryOp {
                op: BinaryOperator::Plus,
                ..
            }
        ));
    }

    // Characterizes existing (left-associative chaining) behavior of the
    // comparison level: `a = 1 = 2` parses as `(a = 1) = 2`, not a parse
    // error. Preserved as-is from the previous Pratt-parser implementation.
    #[test]
    fn chained_comparisons_are_left_associative() {
        let stmt = parse("SELECT * FROM 't.csv' WHERE a = 1 = 2").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let Some(Expr::BinaryOp {
            op: BinaryOperator::Eq,
            left,
            ..
        }) = &as_select(&query).selection
        else {
            panic!("expected top-level Eq")
        };
        assert!(matches!(
            **left,
            Expr::BinaryOp {
                op: BinaryOperator::Eq,
                ..
            }
        ));
    }

    #[test]
    fn explain_wraps_query() {
        let stmt = parse("EXPLAIN SELECT * FROM 't.csv'").unwrap();
        assert!(matches!(stmt, Statement::Explain(_)));
    }

    #[test]
    fn reports_expected_from() {
        let err = parse("SELECT a WEHRE b").unwrap_err();
        assert!(err.to_string().contains("expected FROM"));
    }

    #[test]
    fn reports_unclosed_paren() {
        let err = parse("SELECT (1 + 2 FROM 't.csv'").unwrap_err();
        assert!(err.to_string().contains("expected ')'"));
    }

    #[test]
    fn reports_missing_case_end() {
        let err = parse("SELECT CASE WHEN a THEN 1 FROM 't.csv'").unwrap_err();
        assert!(err.to_string().contains("expected END"));
    }

    #[test]
    fn reports_missing_cast_type() {
        let err = parse("SELECT CAST(a AS) FROM 't.csv'").unwrap_err();
        assert!(err.to_string().contains("expected a type name"));
    }

    #[test]
    fn parses_group_by_having_order_by() {
        let stmt = parse(
            "SELECT region, COUNT(*) AS n FROM 't.csv' GROUP BY region \
             HAVING COUNT(*) > 1 ORDER BY n DESC, region LIMIT 5 OFFSET 1",
        )
        .unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let select = as_select(&query);
        assert_eq!(select.group_by, vec![Expr::Ident("region".to_string())]);
        assert!(select.having.is_some());
        assert_eq!(query.order_by.len(), 2);
        assert!(query.order_by[0].desc);
        assert!(!query.order_by[1].desc);
        assert!(query.offset.is_some());
    }

    #[test]
    fn parses_distinct() {
        let stmt = parse("SELECT DISTINCT region FROM 't.csv'").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        assert!(as_select(&query).distinct);
    }

    #[test]
    fn parses_table_alias_without_as() {
        let stmt = parse("SELECT a FROM 'sales.csv' s").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let FromClause::Table { alias, .. } = &as_select(&query).from else {
            panic!("expected leaf table")
        };
        assert_eq!(*alias, Some("s".to_string()));
    }

    #[test]
    fn parses_quoted_identifier_as_column() {
        let stmt = parse("SELECT \"weird col\" FROM 't.csv'").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let SelectItem::Expr { expr, .. } = &as_select(&query).projection[0] else {
            panic!("expected expr")
        };
        assert_eq!(*expr, Expr::Ident("weird col".to_string()));
    }

    #[test]
    fn parses_function_call_with_star() {
        let stmt = parse("SELECT COUNT(*) FROM 't.csv'").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let SelectItem::Expr { expr, .. } = &as_select(&query).projection[0] else {
            panic!("expected expr")
        };
        assert_eq!(
            *expr,
            Expr::Function {
                name: "COUNT".to_string(),
                args: vec![Expr::Wildcard],
                distinct: false,
            }
        );
    }

    #[test]
    fn parses_nested_function_calls() {
        let stmt = parse("SELECT UPPER(TRIM(a)) FROM 't.csv'").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let SelectItem::Expr { expr, .. } = &as_select(&query).projection[0] else {
            panic!("expected expr")
        };
        let Expr::Function { name, args, .. } = expr else {
            panic!("expected function")
        };
        assert_eq!(name, "UPPER");
        assert!(matches!(args.as_slice(), [Expr::Function { name, .. }] if name == "TRIM"));
    }

    #[test]
    fn parses_count_distinct() {
        let stmt = parse("SELECT COUNT(DISTINCT region) FROM 't.csv'").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let SelectItem::Expr { expr, .. } = &as_select(&query).projection[0] else {
            panic!("expected expr")
        };
        let Expr::Function { distinct, .. } = expr else {
            panic!("expected function")
        };
        assert!(*distinct);
    }

    #[test]
    fn parses_case_expression() {
        let stmt =
            parse("SELECT CASE WHEN a > 1 THEN 'big' ELSE 'small' END FROM 't.csv'").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let SelectItem::Expr { expr, .. } = &as_select(&query).projection[0] else {
            panic!("expected expr")
        };
        assert!(matches!(expr, Expr::Case { operand: None, .. }));
    }

    #[test]
    fn parses_case_expression_with_operand_and_multiple_branches() {
        let stmt = parse(
            "SELECT CASE a WHEN 1 THEN 'one' WHEN 2 THEN 'two' ELSE 'other' END FROM 't.csv'",
        )
        .unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let SelectItem::Expr { expr, .. } = &as_select(&query).projection[0] else {
            panic!("expected expr")
        };
        let Expr::Case {
            operand, branches, ..
        } = expr
        else {
            panic!("expected case")
        };
        assert!(operand.is_some());
        assert_eq!(branches.len(), 2);
    }

    #[test]
    fn parses_cast_expression() {
        let stmt = parse("SELECT CAST(a AS FLOAT) FROM 't.csv'").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let SelectItem::Expr { expr, .. } = &as_select(&query).projection[0] else {
            panic!("expected expr")
        };
        assert!(matches!(expr, Expr::Cast { .. }));
    }

    #[test]
    fn parses_qualified_column() {
        let stmt = parse("SELECT t.a FROM 't.csv' t").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let SelectItem::Expr { expr, .. } = &as_select(&query).projection[0] else {
            panic!("expected expr")
        };
        assert_eq!(*expr, Expr::CompoundIdent("t".into(), "a".into()));
    }

    #[test]
    fn parses_qualified_wildcard() {
        let stmt = parse("SELECT t.* FROM 't.csv' t").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        assert_eq!(
            as_select(&query).projection[0],
            SelectItem::QualifiedWildcard("t".into())
        );
    }

    #[test]
    fn parses_inner_join_on() {
        let stmt = parse("SELECT * FROM 'a.csv' a JOIN 'b.csv' b ON a.id = b.id").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let FromClause::Join {
            join_type,
            condition,
            ..
        } = &as_select(&query).from
        else {
            panic!("expected join")
        };
        assert_eq!(*join_type, JoinType::Inner);
        assert!(matches!(condition, Some(JoinCondition::On(_))));
    }

    #[test]
    fn parses_left_outer_join() {
        let stmt =
            parse("SELECT * FROM 'a.csv' a LEFT OUTER JOIN 'b.csv' b ON a.id = b.id").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let FromClause::Join { join_type, .. } = &as_select(&query).from else {
            panic!("expected join")
        };
        assert_eq!(*join_type, JoinType::Left);
    }

    #[test]
    fn parses_join_using() {
        let stmt = parse("SELECT * FROM 'a.csv' a JOIN 'b.csv' b USING (id)").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let FromClause::Join { condition, .. } = &as_select(&query).from else {
            panic!("expected join")
        };
        assert!(
            matches!(condition, Some(JoinCondition::Using(cols)) if cols == &vec!["id".to_string()])
        );
    }

    #[test]
    fn parses_comma_join_as_cross() {
        let stmt = parse("SELECT * FROM 'a.csv' a, 'b.csv' b WHERE a.id = b.id").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let FromClause::Join { join_type, .. } = &as_select(&query).from else {
            panic!("expected join")
        };
        assert_eq!(*join_type, JoinType::Cross);
    }

    #[test]
    fn parses_derived_table() {
        let stmt = parse("SELECT * FROM (SELECT a FROM 't.csv') AS s").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        assert!(matches!(as_select(&query).from, FromClause::Derived { .. }));
    }

    #[test]
    fn parses_cte() {
        let stmt = parse("WITH s AS (SELECT a FROM 't.csv') SELECT a FROM s").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        assert_eq!(query.with.len(), 1);
        assert_eq!(query.with[0].name, "s");
    }

    #[test]
    fn parses_union() {
        let stmt = parse("SELECT a FROM 't.csv' UNION SELECT a FROM 'u.csv'").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let SetExpr::SetOp { op, all, .. } = query.body else {
            panic!("expected set op")
        };
        assert_eq!(op, SetOperator::Union);
        assert!(!all);
    }

    #[test]
    fn parses_union_all() {
        let stmt = parse("SELECT a FROM 't.csv' UNION ALL SELECT a FROM 'u.csv'").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let SetExpr::SetOp { all, .. } = query.body else {
            panic!("expected set op")
        };
        assert!(all);
    }

    #[test]
    fn parses_in_list() {
        let stmt = parse("SELECT * FROM 't.csv' WHERE a IN (1, 2, 3)").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        assert!(matches!(
            as_select(&query).selection,
            Some(Expr::InList { .. })
        ));
    }

    #[test]
    fn parses_not_in_list() {
        let stmt = parse("SELECT * FROM 't.csv' WHERE a NOT IN (1, 2)").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let Some(Expr::UnaryOp {
            op: UnaryOperator::Not,
            expr,
        }) = &as_select(&query).selection
        else {
            panic!("expected NOT wrapper")
        };
        assert!(matches!(**expr, Expr::InList { .. }));
    }

    #[test]
    fn parses_in_subquery() {
        let stmt = parse("SELECT * FROM 't.csv' WHERE a IN (SELECT b FROM 'u.csv')").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        assert!(matches!(
            as_select(&query).selection,
            Some(Expr::InSubquery { .. })
        ));
    }

    #[test]
    fn parses_exists() {
        let stmt = parse("SELECT * FROM 't.csv' WHERE EXISTS (SELECT 1 FROM 'u.csv')").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        assert!(matches!(
            as_select(&query).selection,
            Some(Expr::Exists { .. })
        ));
    }

    #[test]
    fn parses_scalar_subquery() {
        let stmt = parse("SELECT (SELECT b FROM 'u.csv') FROM 't.csv'").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        let SelectItem::Expr { expr, .. } = &as_select(&query).projection[0] else {
            panic!("expected expr")
        };
        assert!(matches!(expr, Expr::ScalarSubquery(_)));
    }
}
