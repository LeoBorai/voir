use voir_types::VoirError;

use crate::ast::{
    BinaryOperator, Expr, Literal, Query, SelectItem, Statement, TableRef, UnaryOperator,
};
use crate::lexer::Lexer;
use crate::token::{Token, TokenKind, describe};

/// # Errors
///
/// Returns an error if `sql` fails to lex or does not parse as a valid statement.
pub fn parse(sql: &str) -> Result<Statement, VoirError> {
    let tokens = Lexer::new(sql).tokenize()?;
    Parser::new(tokens).parse_statement()
}

// Binding power table (see plan.md section 4): OR < AND < NOT < comparisons
// < +- < */% < unary. Levels are spaced two apart so infix ops can report
// (left_bp, right_bp) as (level, level + 1) for left-associativity.
const NOT_BP: u8 = 7;
const UNARY_BP: u8 = 14;

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn expect(&mut self, kind: &TokenKind, expected: &str) -> Result<Token, VoirError> {
        if std::mem::discriminant(&self.peek().kind) == std::mem::discriminant(kind) {
            Ok(self.advance())
        } else {
            self.error(expected)
        }
    }

    fn error<T>(&self, expected: &str) -> Result<T, VoirError> {
        let tok = self.peek();
        Err(VoirError::Parse {
            message: format!("expected {expected}, found {}", describe(&tok.kind)),
            span: tok.span,
        })
    }

    fn parse_statement(&mut self) -> Result<Statement, VoirError> {
        let statement = if matches!(self.peek().kind, TokenKind::Explain) {
            self.advance();
            Statement::Explain(Box::new(self.parse_query()?))
        } else {
            Statement::Query(self.parse_query()?)
        };
        self.finish()?;
        Ok(statement)
    }

    fn finish(&mut self) -> Result<(), VoirError> {
        if matches!(self.peek().kind, TokenKind::Semicolon) {
            self.advance();
        }
        if !matches!(self.peek().kind, TokenKind::Eof) {
            return self.error("end of statement");
        }
        Ok(())
    }

    fn parse_query(&mut self) -> Result<Query, VoirError> {
        self.expect(&TokenKind::Select, "SELECT")?;
        let projection = self.parse_select_list()?;
        self.expect(&TokenKind::From, "FROM")?;
        let from = self.parse_table_ref()?;
        let selection = if matches!(self.peek().kind, TokenKind::Where) {
            self.advance();
            Some(self.parse_expr(0)?)
        } else {
            None
        };
        let limit = if matches!(self.peek().kind, TokenKind::Limit) {
            self.advance();
            Some(self.parse_expr(0)?)
        } else {
            None
        };
        Ok(Query {
            projection,
            from,
            selection,
            limit,
        })
    }

    fn parse_select_list(&mut self) -> Result<Vec<SelectItem>, VoirError> {
        let mut items = vec![self.parse_select_item()?];
        while matches!(self.peek().kind, TokenKind::Comma) {
            self.advance();
            items.push(self.parse_select_item()?);
        }
        Ok(items)
    }

    fn parse_select_item(&mut self) -> Result<SelectItem, VoirError> {
        if matches!(self.peek().kind, TokenKind::Star) {
            self.advance();
            return Ok(SelectItem::Wildcard);
        }
        let expr = self.parse_expr(0)?;
        let alias = if matches!(self.peek().kind, TokenKind::As) {
            self.advance();
            Some(self.parse_ident()?)
        } else {
            None
        };
        Ok(SelectItem::Expr { expr, alias })
    }

    fn parse_table_ref(&mut self) -> Result<TableRef, VoirError> {
        let tok = self.advance();
        let path = match tok.kind {
            TokenKind::StringLiteral(s) | TokenKind::Ident(s) | TokenKind::QuotedIdent(s) => s,
            other => {
                return Err(VoirError::Parse {
                    message: format!("expected table reference, found {}", describe(&other)),
                    span: tok.span,
                });
            }
        };
        let alias = if matches!(self.peek().kind, TokenKind::As) {
            self.advance();
            Some(self.parse_ident()?)
        } else if matches!(self.peek().kind, TokenKind::Ident(_)) {
            Some(self.parse_ident()?)
        } else {
            None
        };
        Ok(TableRef { path, alias })
    }

    fn parse_ident(&mut self) -> Result<String, VoirError> {
        let tok = self.advance();
        match tok.kind {
            TokenKind::Ident(s) | TokenKind::QuotedIdent(s) => Ok(s),
            other => Err(VoirError::Parse {
                message: format!("expected identifier, found {}", describe(&other)),
                span: tok.span,
            }),
        }
    }

    fn parse_expr(&mut self, min_bp: u8) -> Result<Expr, VoirError> {
        let mut lhs = self.parse_prefix()?;
        while let Some((op, l_bp, r_bp)) = infix_binding_power(&self.peek().kind) {
            if l_bp < min_bp {
                break;
            }
            self.advance();
            let rhs = self.parse_expr(r_bp)?;
            lhs = Expr::BinaryOp {
                left: Box::new(lhs),
                op,
                right: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_prefix(&mut self) -> Result<Expr, VoirError> {
        let tok = self.advance();
        match tok.kind {
            TokenKind::Not => {
                let expr = self.parse_expr(NOT_BP)?;
                Ok(Expr::UnaryOp {
                    op: UnaryOperator::Not,
                    expr: Box::new(expr),
                })
            }
            TokenKind::Minus => {
                let expr = self.parse_expr(UNARY_BP)?;
                Ok(Expr::UnaryOp {
                    op: UnaryOperator::Negate,
                    expr: Box::new(expr),
                })
            }
            TokenKind::Plus => {
                let expr = self.parse_expr(UNARY_BP)?;
                Ok(Expr::UnaryOp {
                    op: UnaryOperator::Plus,
                    expr: Box::new(expr),
                })
            }
            TokenKind::LParen => {
                let expr = self.parse_expr(0)?;
                self.expect(&TokenKind::RParen, "')'")?;
                Ok(expr)
            }
            TokenKind::Null => Ok(Expr::Literal(Literal::Null)),
            TokenKind::True => Ok(Expr::Literal(Literal::Boolean(true))),
            TokenKind::False => Ok(Expr::Literal(Literal::Boolean(false))),
            TokenKind::Number(text) => Ok(parse_number_literal(&text)),
            TokenKind::StringLiteral(s) => Ok(Expr::Literal(Literal::String(s))),
            TokenKind::Ident(s) | TokenKind::QuotedIdent(s) => Ok(Expr::Ident(s)),
            other => Err(VoirError::Parse {
                message: format!("expected expression, found {}", describe(&other)),
                span: tok.span,
            }),
        }
    }
}

fn infix_binding_power(kind: &TokenKind) -> Option<(BinaryOperator, u8, u8)> {
    let (op, l_bp) = match kind {
        TokenKind::Or => (BinaryOperator::Or, 2),
        TokenKind::And => (BinaryOperator::And, 4),
        TokenKind::Eq => (BinaryOperator::Eq, 8),
        TokenKind::NotEq => (BinaryOperator::NotEq, 8),
        TokenKind::Lt => (BinaryOperator::Lt, 8),
        TokenKind::LtEq => (BinaryOperator::LtEq, 8),
        TokenKind::Gt => (BinaryOperator::Gt, 8),
        TokenKind::GtEq => (BinaryOperator::GtEq, 8),
        TokenKind::Plus => (BinaryOperator::Plus, 10),
        TokenKind::Minus => (BinaryOperator::Minus, 10),
        TokenKind::Star => (BinaryOperator::Multiply, 12),
        TokenKind::Slash => (BinaryOperator::Divide, 12),
        TokenKind::Percent => (BinaryOperator::Modulo, 12),
        _ => return None,
    };
    Some((op, l_bp, l_bp + 1))
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

    #[test]
    fn parses_simple_select() {
        let stmt = parse("SELECT a, b FROM 'sales.csv' WHERE a = 1 LIMIT 10").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        assert_eq!(query.projection.len(), 2);
        assert_eq!(query.from.path, "sales.csv");
        assert!(query.selection.is_some());
        assert!(query.limit.is_some());
    }

    #[test]
    fn parses_wildcard() {
        let stmt = parse("SELECT * FROM 'sales.csv'").unwrap();
        let Statement::Query(query) = stmt else {
            panic!("expected query")
        };
        assert_eq!(query.projection, vec![SelectItem::Wildcard]);
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
        }) = query.selection
        else {
            panic!("expected top-level OR")
        };
        assert!(matches!(
            *right,
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
        }) = query.selection
        else {
            panic!("expected top-level AND")
        };
        assert!(matches!(
            *left,
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
        let SelectItem::Expr { expr, .. } = &query.projection[0] else {
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
    fn explain_wraps_query() {
        let stmt = parse("EXPLAIN SELECT * FROM 't.csv'").unwrap();
        assert!(matches!(stmt, Statement::Explain(_)));
    }

    #[test]
    fn reports_expected_from() {
        let err = parse("SELECT a WEHRE b").unwrap_err();
        assert!(err.to_string().contains("expected FROM"));
    }
}
