use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};
use std::io::IsTerminal;

use rustyline::completion::{Completer, FilenameCompleter, Pair};
use rustyline::highlight::{CmdKind, Highlighter};
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper, Result as RlResult};
use voir_sql::KEYWORDS;

use crate::highlight;
use crate::session::Session;

/// Tab completion for the REPL: SQL keywords, registered table names, and
/// (when a `FROM <table>` is detectable in the current line) that table's
/// column names — plus file-path completion inside a quoted string literal.
pub struct VoirHelper {
    filename: FilenameCompleter,
    tables: Vec<String>,
    columns: HashMap<String, Vec<String>>,
    color: bool,
}

impl VoirHelper {
    pub fn new() -> Self {
        Self {
            filename: FilenameCompleter::new(),
            tables: Vec::new(),
            columns: HashMap::new(),
            color: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
        }
    }

    /// Snapshots registered table/column names ahead of each `readline`
    /// call — the editor owns the helper, so its state can't borrow the
    /// session live.
    pub fn refresh(&mut self, session: &Session) {
        self.tables.clear();
        self.columns.clear();
        for (name, table) in session.catalog.tables() {
            self.tables.push(name.to_string());
            let cols = table
                .provider
                .schema()
                .fields
                .iter()
                .map(|f| f.name.clone())
                .collect();
            self.columns.insert(name.to_string(), cols);
        }
    }
}

impl Default for VoirHelper {
    fn default() -> Self {
        Self::new()
    }
}

impl Completer for VoirHelper {
    type Candidate = Pair;

    fn complete(&self, line: &str, pos: usize, ctx: &Context<'_>) -> RlResult<(usize, Vec<Pair>)> {
        if in_quoted_string(line, pos) {
            return self.filename.complete(line, pos, ctx);
        }

        let start = word_start(line, pos);
        let fragment = line[start..pos].to_ascii_lowercase();

        let mut candidates: Vec<String> = KEYWORDS.iter().map(|k| (*k).to_string()).collect();
        candidates.extend(self.tables.iter().cloned());
        if let Some(table) = from_target(&line[..pos]) {
            if let Some(cols) = self.columns.get(&table.to_ascii_lowercase()) {
                candidates.extend(cols.iter().cloned());
            }
        } else {
            for cols in self.columns.values() {
                candidates.extend(cols.iter().cloned());
            }
        }

        let matches: BTreeSet<String> = candidates
            .into_iter()
            .filter(|c| c.to_ascii_lowercase().starts_with(&fragment))
            .collect();
        let pairs = matches
            .into_iter()
            .map(|c| Pair {
                display: c.clone(),
                replacement: c,
            })
            .collect();
        Ok((start, pairs))
    }
}

impl Hinter for VoirHelper {
    type Hint = String;
}

impl Highlighter for VoirHelper {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        if !self.color {
            return Cow::Borrowed(line);
        }
        highlight::colorize(line).map_or(Cow::Borrowed(line), Cow::Owned)
    }

    fn highlight_char(&self, _line: &str, _pos: usize, _kind: CmdKind) -> bool {
        self.color
    }
}

impl Validator for VoirHelper {}
impl Helper for VoirHelper {}

fn word_start(line: &str, pos: usize) -> usize {
    line[..pos]
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
        .map_or(0, |i| i + 1)
}

fn in_quoted_string(line: &str, pos: usize) -> bool {
    line[..pos].chars().filter(|&c| c == '\'').count() % 2 == 1
}

/// Best-effort scan for the identifier following the most recent `FROM`
/// before the cursor — enough to scope column completion without a full
/// reparse of in-progress input.
fn from_target(prefix: &str) -> Option<String> {
    let lower = prefix.to_ascii_lowercase();
    let idx = lower.rfind("from")?;
    let rest = prefix[idx + 4..].trim_start();
    let ident: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if ident.is_empty() { None } else { Some(ident) }
}
