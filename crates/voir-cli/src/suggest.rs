//! Pure suggestion engine: `(buffer, cursor)` in, ranked [`Candidate`]s out.
//! No terminal, no I/O — [`classify`] and [`candidates`] are plain
//! functions so the ranking rules are unit-testable without a pty.

use std::collections::HashSet;

use voir_sql::{KEYWORDS, Token, TokenKind};
use voir_types::VoirError;

/// What kind of thing the cursor is positioned to type next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Context {
    /// Start of a statement: SELECT, EXPLAIN, WITH.
    StatementStart,
    /// After FROM / JOIN: registered tables.
    TableRef,
    /// Projection, WHERE, GROUP BY, HAVING, ORDER BY, ON:
    /// columns of the FROM targets, functions, keywords.
    Expression { tables: Vec<String> },
    /// Inside a string literal in table position — file paths only.
    PathLiteral { prefix: String },
    /// After `\` at line start: meta-command names.
    MetaCommand,
    /// After `\open`, `\i`, `\o`, `\save`, `\load`: file paths.
    MetaArgPath { prefix: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Table,
    Column,
    Keyword,
    Function,
    File,
}

impl Kind {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Kind::Table => "table",
            Kind::Column => "column",
            Kind::Keyword => "keyword",
            Kind::Function => "function",
            Kind::File => "file",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub value: String,
    pub kind: Kind,
    /// Table a column came from, for the description pane.
    pub source: Option<String>,
}

/// A registered table's name and column names, snapshotted ahead of a
/// completion pass — mirrors the old `VoirHelper::refresh`.
#[derive(Debug, Clone, Default)]
pub struct TableSchema {
    pub name: String,
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub tables: Vec<TableSchema>,
}

impl Snapshot {
    #[must_use]
    pub fn columns_of(&self, name: &str) -> Option<&[String]> {
        self.tables
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
            .map(|t| t.columns.as_slice())
    }
}

const STATEMENT_START_KEYWORDS: &[&str] = &["SELECT", "EXPLAIN", "WITH"];
const SCALAR_FUNCTIONS: &[&str] = &[
    "upper", "lower", "trim", "length", "concat", "abs", "round", "ceil", "floor", "power", "sqrt",
    "mod", "coalesce", "nullif",
];
const AGG_FUNCTIONS: &[&str] = &["count", "sum", "avg", "min", "max"];
const META_COMMANDS: &[&str] = &[
    "open",
    "dt",
    "d",
    "format",
    "timing",
    "explain",
    "i",
    "o",
    "save",
    "load",
    "autocomplete",
    "q",
    "?",
];
const META_PATH_COMMANDS: &[&str] = &["open", "i", "o", "save", "load"];

/// Classifies the cursor position in `line` into a [`Context`]. Lexes
/// `line[..pos]` (or, for identifier contexts, the buffer up to the start of
/// the in-progress word) with `voir_sql::tokenize`; on a lex error — most
/// commonly an unterminated string literal while the user is mid-path —
/// retries against the prefix up to the error's span start, the same
/// fallback shape `highlight.rs` uses for colorizing incomplete input.
#[must_use]
pub fn classify(line: &str, pos: usize) -> Context {
    if line.trim_start().starts_with('\\') {
        return classify_meta(line, pos);
    }

    if in_open_string(line, pos) {
        if let Some(quote_idx) = line[..pos].rfind('\'') {
            let tokens = tokenize_lenient(&line[..quote_idx]);
            if clause_state(&tokens) == ClauseState::TableRef {
                return Context::PathLiteral {
                    prefix: line[quote_idx + 1..pos].to_string(),
                };
            }
        }
        return Context::Expression { tables: Vec::new() };
    }

    let start = word_start(line, pos);
    let tokens = tokenize_lenient(&line[..start]);
    match clause_state(&tokens) {
        ClauseState::StatementStart => Context::StatementStart,
        ClauseState::TableRef => Context::TableRef,
        ClauseState::Expression => Context::Expression {
            tables: from_tables(&statement_tokens_at(line, pos)),
        },
    }
}

/// Classifies and ranks candidates for `line[..pos]` against `schema` in one
/// call — the entry point `VoirCompleter` drives on every keystroke.
/// Returns no candidates for [`Context::PathLiteral`] / [`Context::MetaArgPath`];
/// those are file-path completions handled separately (see `paths.rs`).
#[must_use]
pub fn candidates(line: &str, pos: usize, schema: &Snapshot) -> Vec<Candidate> {
    let context = classify(line, pos);
    let fragment = fragment(line, pos, &context);
    rank(&context, &fragment, schema)
}

/// Ranks candidates for an already-classified `context` — split out from
/// [`candidates`] so a caller that needs to augment `schema` first (e.g. the
/// completer merging in an inferred file schema, see `schema_cache.rs`) can
/// classify once and rank against the augmented snapshot.
#[must_use]
pub fn rank(context: &Context, fragment: &str, schema: &Snapshot) -> Vec<Candidate> {
    match context {
        Context::StatementStart => statement_start_candidates(fragment),
        Context::TableRef => table_ref_candidates(fragment, schema),
        Context::Expression { tables } => expression_candidates(tables, fragment, schema),
        Context::MetaCommand => meta_command_candidates(fragment),
        Context::PathLiteral { .. } | Context::MetaArgPath { .. } => Vec::new(),
    }
}

/// Byte offset of the start of the identifier ending at `pos` — the
/// in-progress word for identifier-shaped contexts.
#[must_use]
pub fn word_start(line: &str, pos: usize) -> usize {
    line[..pos]
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
        .map_or(0, |i| i + 1)
}

/// The in-progress word's text for `context` — what candidates are filtered
/// against, and (paired with its start offset from [`word_start`] or the
/// `Path*` variants' own `prefix`) what a completer's replacement span covers.
#[must_use]
pub fn fragment(line: &str, pos: usize, context: &Context) -> String {
    match context {
        Context::MetaCommand => {
            let backslash = line.find('\\').unwrap_or(pos.saturating_sub(1));
            line.get(backslash + 1..pos).unwrap_or("").to_string()
        }
        Context::PathLiteral { prefix } | Context::MetaArgPath { prefix } => prefix.clone(),
        Context::StatementStart | Context::TableRef | Context::Expression { .. } => {
            let start = word_start(line, pos);
            line[start..pos].to_string()
        }
    }
}

fn in_open_string(line: &str, pos: usize) -> bool {
    line[..pos].chars().filter(|&c| c == '\'').count() % 2 == 1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClauseState {
    StatementStart,
    TableRef,
    Expression,
}

fn clause_state(tokens: &[Token]) -> ClauseState {
    let mut state = ClauseState::StatementStart;
    for tok in tokens {
        advance_clause_state(&mut state, &tok.kind);
    }
    state
}

fn advance_clause_state(state: &mut ClauseState, kind: &TokenKind) {
    match kind {
        TokenKind::Semicolon | TokenKind::Union | TokenKind::Except | TokenKind::Intersect => {
            *state = ClauseState::StatementStart;
        }
        TokenKind::From
        | TokenKind::Join
        | TokenKind::Inner
        | TokenKind::Left
        | TokenKind::Right
        | TokenKind::Full
        | TokenKind::Outer
        | TokenKind::Cross => {
            *state = ClauseState::TableRef;
        }
        TokenKind::Select
        | TokenKind::Where
        | TokenKind::On
        | TokenKind::Having
        | TokenKind::By
        | TokenKind::Group
        | TokenKind::Order
        | TokenKind::And
        | TokenKind::Or
        | TokenKind::Not
        | TokenKind::As
        | TokenKind::Distinct
        | TokenKind::Case
        | TokenKind::When
        | TokenKind::Then
        | TokenKind::Else
        | TokenKind::End
        | TokenKind::Cast
        | TokenKind::Using
        | TokenKind::In
        | TokenKind::Exists
        | TokenKind::All
        | TokenKind::Limit
        | TokenKind::Offset
        | TokenKind::Eq
        | TokenKind::NotEq
        | TokenKind::Lt
        | TokenKind::LtEq
        | TokenKind::Gt
        | TokenKind::GtEq
        | TokenKind::Plus
        | TokenKind::Minus
        | TokenKind::Slash
        | TokenKind::Percent
        | TokenKind::Star
        | TokenKind::Dot
        | TokenKind::LParen => {
            *state = ClauseState::Expression;
        }
        _ => {}
    }
}

/// Lexes `src`, retrying against the prefix up to a lex error's span start
/// when the whole string doesn't lex (e.g. a half-typed `'file.csv`
/// unterminated literal).
fn tokenize_lenient(src: &str) -> Vec<Token> {
    match voir_sql::tokenize(src) {
        Ok(tokens) => tokens,
        Err(VoirError::Parse { span, .. }) => {
            let cut = (span.start as usize).min(src.len());
            voir_sql::tokenize(&src[..cut]).unwrap_or_default()
        }
        Err(_) => Vec::new(),
    }
}

/// Tokens of the statement containing `pos`, spanning the whole statement
/// (not just the text before the cursor) — so a `FROM` clause is found
/// regardless of where within the statement the cursor sits, and a table
/// named in an earlier `;`-separated statement doesn't leak into this one.
fn statement_tokens_at(line: &str, pos: usize) -> Vec<Token> {
    let tokens = tokenize_lenient(line);
    let target = tokens
        .iter()
        .filter(|t| matches!(t.kind, TokenKind::Semicolon) && (t.span.end as usize) <= pos)
        .count();
    let mut idx = 0usize;
    let mut out = Vec::new();
    for tok in tokens {
        if matches!(tok.kind, TokenKind::Eof) {
            break;
        }
        if matches!(tok.kind, TokenKind::Semicolon) {
            idx += 1;
            continue;
        }
        if idx == target {
            out.push(tok);
        }
    }
    out
}

/// Table references named in a `FROM`/`JOIN` list — registered names *and*
/// `'path.csv'` literals, since `voir` lets a query name a file directly.
/// Aliases and join conditions are skipped: only the token immediately
/// after `FROM`/`JOIN`/a list comma is taken.
fn from_tables(tokens: &[Token]) -> Vec<String> {
    let mut tables = Vec::new();
    let mut in_from = false;
    let mut expect_ref = false;
    for tok in tokens {
        match &tok.kind {
            TokenKind::From | TokenKind::Join => {
                in_from = true;
                expect_ref = true;
            }
            TokenKind::Comma if in_from => expect_ref = true,
            TokenKind::Where
            | TokenKind::Group
            | TokenKind::Having
            | TokenKind::Order
            | TokenKind::Limit
            | TokenKind::Offset
            | TokenKind::Semicolon => {
                in_from = false;
                expect_ref = false;
            }
            TokenKind::Ident(name) | TokenKind::QuotedIdent(name) if expect_ref => {
                tables.push(name.clone());
                expect_ref = false;
            }
            TokenKind::StringLiteral(path) if expect_ref => {
                tables.push(path.clone());
                expect_ref = false;
            }
            _ => {}
        }
    }
    tables
}

fn classify_meta(line: &str, pos: usize) -> Context {
    let Some(backslash) = line.find('\\') else {
        return Context::MetaCommand;
    };
    if pos <= backslash {
        return Context::MetaCommand;
    }
    let after = &line[backslash + 1..];
    let after_pos = pos - (backslash + 1);
    let cmd_end = after.find(char::is_whitespace).unwrap_or(after.len());
    if after_pos <= cmd_end {
        return Context::MetaCommand;
    }
    let rest = &after[cmd_end..];
    let rest_pos = after_pos - cmd_end;
    let word_start_in_rest = rest[..rest_pos]
        .rfind(char::is_whitespace)
        .map_or(0, |i| i + 1);
    let prefix = rest[word_start_in_rest..rest_pos].to_string();
    let cmd = &after[..cmd_end];
    if META_PATH_COMMANDS.contains(&cmd.to_ascii_lowercase().as_str()) {
        Context::MetaArgPath { prefix }
    } else {
        Context::MetaCommand
    }
}

/// Filters `entries` by prefix/substring match against `fragment`, ranks by
/// (match kind, context-preferred kind, alphabetical), and dedups.
fn finalize(entries: Vec<(String, Kind, Option<String>, u8)>, fragment: &str) -> Vec<Candidate> {
    let frag_lower = fragment.to_ascii_lowercase();
    let mut matched: Vec<(u8, u8, String, Candidate)> = entries
        .into_iter()
        .filter_map(|(value, kind, source, weight)| {
            let lower = value.to_ascii_lowercase();
            let match_rank = if lower.starts_with(&frag_lower) {
                0u8
            } else if lower.contains(&frag_lower) {
                1u8
            } else {
                return None;
            };
            Some((
                match_rank,
                weight,
                lower,
                Candidate {
                    value,
                    kind,
                    source,
                },
            ))
        })
        .collect();
    matched.sort_by(|a, b| (a.0, a.1, a.2.as_str()).cmp(&(b.0, b.1, b.2.as_str())));

    let mut seen = HashSet::new();
    matched
        .into_iter()
        .filter(|(_, _, _, c)| seen.insert((c.value.clone(), c.kind)))
        .map(|(_, _, _, c)| c)
        .collect()
}

fn statement_start_candidates(fragment: &str) -> Vec<Candidate> {
    let entries = STATEMENT_START_KEYWORDS
        .iter()
        .map(|k| ((*k).to_string(), Kind::Keyword, None, 0u8))
        .collect();
    finalize(entries, fragment)
}

fn table_ref_candidates(fragment: &str, schema: &Snapshot) -> Vec<Candidate> {
    let entries = schema
        .tables
        .iter()
        .map(|t| (t.name.clone(), Kind::Table, None, 0u8))
        .collect();
    finalize(entries, fragment)
}

fn expression_candidates(tables: &[String], fragment: &str, schema: &Snapshot) -> Vec<Candidate> {
    let mut resolved: Vec<&TableSchema> = tables
        .iter()
        .filter_map(|t| {
            schema
                .tables
                .iter()
                .find(|s| s.name.eq_ignore_ascii_case(t))
        })
        .collect();
    if resolved.is_empty() {
        resolved = schema.tables.iter().collect();
    }

    let mut entries = Vec::new();
    for table in &resolved {
        for col in &table.columns {
            entries.push((col.clone(), Kind::Column, Some(table.name.clone()), 0u8));
        }
    }
    for name in SCALAR_FUNCTIONS.iter().chain(AGG_FUNCTIONS.iter()) {
        entries.push(((*name).to_string(), Kind::Function, None, 1u8));
    }
    for kw in KEYWORDS {
        entries.push(((*kw).to_string(), Kind::Keyword, None, 2u8));
    }
    finalize(entries, fragment)
}

fn meta_command_candidates(fragment: &str) -> Vec<Candidate> {
    let entries = META_COMMANDS
        .iter()
        .map(|c| ((*c).to_string(), Kind::Keyword, None, 0u8))
        .collect();
    finalize(entries, fragment)
}

#[cfg(test)]
mod tests {
    use super::{Context, Kind, Snapshot, TableSchema, candidates, classify};

    /// `at("SELECT re|gion", '|')` returns `("SELECT region", 9)`.
    fn at(marked: &str) -> (String, usize) {
        let idx = marked
            .find('|')
            .expect("test input needs a | cursor marker");
        let mut line = String::with_capacity(marked.len() - 1);
        line.push_str(&marked[..idx]);
        line.push_str(&marked[idx + 1..]);
        (line, idx)
    }

    fn schema() -> Snapshot {
        Snapshot {
            tables: vec![
                TableSchema {
                    name: "users".into(),
                    columns: vec!["id".into(), "name".into(), "region".into()],
                },
                TableSchema {
                    name: "orders".into(),
                    columns: vec!["id".into(), "user_id".into(), "amount".into()],
                },
            ],
        }
    }

    #[test]
    fn empty_buffer_is_statement_start() {
        let (line, pos) = at("|");
        assert_eq!(classify(&line, pos), Context::StatementStart);
    }

    #[test]
    fn after_semicolon_is_statement_start() {
        let (line, pos) = at("SELECT 1; |");
        assert_eq!(classify(&line, pos), Context::StatementStart);
    }

    #[test]
    fn after_from_is_table_ref() {
        let (line, pos) = at("SELECT a FROM |");
        assert_eq!(classify(&line, pos), Context::TableRef);
    }

    #[test]
    fn comma_in_from_list_stays_table_ref() {
        let (line, pos) = at("SELECT a FROM users, |");
        assert_eq!(classify(&line, pos), Context::TableRef);
    }

    #[test]
    fn projection_is_expression() {
        let (line, pos) = at("SELECT |");
        assert_eq!(
            classify(&line, pos),
            Context::Expression { tables: Vec::new() }
        );
    }

    #[test]
    fn where_clause_resolves_from_table() {
        let (line, pos) = at("SELECT a FROM users WHERE |");
        assert_eq!(
            classify(&line, pos),
            Context::Expression {
                tables: vec!["users".into()]
            }
        );
    }

    #[test]
    fn mid_word_cursor_uses_context_before_the_word() {
        let (line, pos) = at("SELECT re|gion FROM t");
        assert_eq!(
            classify(&line, pos),
            Context::Expression {
                tables: vec!["t".into()]
            }
        );
    }

    #[test]
    fn unterminated_literal_after_from_is_path_literal() {
        let (line, pos) = at("SELECT * FROM 'testdata/earthq|");
        assert_eq!(
            classify(&line, pos),
            Context::PathLiteral {
                prefix: "testdata/earthq".into()
            }
        );
    }

    #[test]
    fn unterminated_literal_outside_table_position_is_not_path_literal() {
        let (line, pos) = at("SELECT a FROM t WHERE region = 'EU|");
        assert!(!matches!(classify(&line, pos), Context::PathLiteral { .. }));
    }

    #[test]
    fn from_clause_naming_a_file_path() {
        let (line, pos) = at("SELECT a FROM 'sales.csv' WHERE |");
        assert_eq!(
            classify(&line, pos),
            Context::Expression {
                tables: vec!["sales.csv".into()]
            }
        );
    }

    #[test]
    fn only_the_statement_under_the_cursor_matters() {
        let (line, pos) = at("SELECT a FROM t; SELECT b FROM u WHERE |");
        assert_eq!(
            classify(&line, pos),
            Context::Expression {
                tables: vec!["u".into()]
            }
        );
    }

    #[test]
    fn backslash_command_word_is_meta_command() {
        let (line, pos) = at("\\op|");
        assert_eq!(classify(&line, pos), Context::MetaCommand);
    }

    #[test]
    fn backslash_open_argument_is_meta_arg_path() {
        let (line, pos) = at("\\open ./testdata/sal|");
        assert_eq!(
            classify(&line, pos),
            Context::MetaArgPath {
                prefix: "./testdata/sal".into()
            }
        );
    }

    #[test]
    fn backslash_dt_argument_is_not_meta_arg_path() {
        let (line, pos) = at("\\dt fo|");
        assert!(!matches!(classify(&line, pos), Context::MetaArgPath { .. }));
    }

    #[test]
    fn table_ref_candidates_rank_prefix_matches_first() {
        let (line, pos) = at("SELECT a FROM use|");
        let cands = candidates(&line, pos, &schema());
        assert_eq!(cands[0].value, "users");
        assert_eq!(cands[0].kind, Kind::Table);
    }

    #[test]
    fn expression_candidates_prefer_columns_over_keywords() {
        let (line, pos) = at("SELECT a FROM users WHERE |");
        let cands = candidates(&line, pos, &schema());
        let column_pos = cands.iter().position(|c| c.value == "region").unwrap();
        let keyword_pos = cands.iter().position(|c| c.value == "SELECT").unwrap();
        assert!(column_pos < keyword_pos);
    }

    #[test]
    fn expression_candidates_fall_back_to_every_table_with_no_resolvable_from() {
        let (line, pos) = at("SELECT |");
        let cands = candidates(&line, pos, &schema());
        assert!(cands.iter().any(|c| c.value == "amount"));
        assert!(cands.iter().any(|c| c.value == "region"));
    }

    #[test]
    fn path_literal_and_meta_arg_path_produce_no_identifier_candidates() {
        let (line, pos) = at("SELECT * FROM 'testdata/earthq|");
        assert_eq!(candidates(&line, pos, &schema()).len(), 0);

        let (line, pos) = at("\\open ./testdata/sal|");
        assert_eq!(candidates(&line, pos, &schema()).len(), 0);
    }

    #[test]
    fn statement_start_offers_only_statement_keywords() {
        let (line, pos) = at("|");
        let cands = candidates(&line, pos, &schema());
        for c in &cands {
            assert!(["SELECT", "EXPLAIN", "WITH"].contains(&c.value.as_str()));
        }
    }
}
