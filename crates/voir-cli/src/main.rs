use std::io::{self, IsTerminal, Read};
use std::process::ExitCode;

use clap::Parser;

mod completion;
mod diagnostics;
mod format;
mod highlight;
mod meta;
mod paths;
mod prompt;
mod repl;
mod schema_cache;
mod session;
mod statement;
mod suggest;
mod validator;

use format::OutputFormat;
use session::Session;

#[derive(Debug, Parser)]
#[command(
    name = "voir",
    version,
    about = "SQL over tabular files",
    next_line_help = true
)]
struct Cli {
    /// SQL query to run, e.g. SELECT * FROM 'sales.csv' WHERE region = 'EU'.
    /// Omit to start the interactive REPL.
    query: Option<String>,

    /// Execute SQL and exit. Equivalent to the positional argument; wins if
    /// both are given.
    #[arg(short = 'c', long = "command")]
    command: Option<String>,

    /// Print the logical plan before running each query
    #[arg(long)]
    explain: bool,

    /// Output format: table, csv, tsv, json, ndjson, markdown
    #[arg(long)]
    format: Option<String>,

    /// Print query execution time
    #[arg(long)]
    timing: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut session = Session::new();
    session.explain = cli.explain;
    session.timing = cli.timing;
    if let Some(format) = &cli.format {
        if let Some(f) = OutputFormat::parse(format) {
            session.format = f;
        } else {
            eprintln!(
                "error: unknown format '{format}' (expected table|csv|tsv|json|ndjson|markdown)"
            );
            return ExitCode::from(2);
        }
    }

    match cli.command.as_ref().or(cli.query.as_ref()) {
        Some(query) => {
            if session.execute(query) {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        None if !io::stdin().is_terminal() => {
            if run_stdin(&mut session) {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        None => match repl::run(&mut session) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("error: {err}");
                ExitCode::from(1)
            }
        },
    }
}

/// Runs every `;`-separated statement piped in on stdin, stopping at the
/// first failure. Reads the whole input up front (rather than splitting on
/// raw `;` bytes) so `statement::split`'s string/comment awareness applies —
/// a semicolon inside a string literal doesn't end a statement early.
fn run_stdin(session: &mut Session) -> bool {
    let mut content = String::new();
    if let Err(err) = io::stdin().read_to_string(&mut content) {
        eprintln!("error: {err}");
        return false;
    }
    let statements = match statement::split(&content) {
        Ok(statements) => statements,
        Err(err) => {
            diagnostics::report(&content, &err);
            return false;
        }
    };
    for stmt in &statements {
        if !session.execute(stmt) {
            return false;
        }
    }
    true
}
