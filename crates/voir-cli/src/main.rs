use std::process::ExitCode;

use clap::Parser;

mod completion;
mod diagnostics;
mod format;
mod meta;
mod repl;
mod session;
mod statement;

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

    /// Print the logical plan before running each query
    #[arg(long)]
    explain: bool,

    /// Output format: table, csv, tsv, json, ndjson
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
            eprintln!("error: unknown format '{format}' (expected table|csv|tsv|json|ndjson)");
            return ExitCode::from(2);
        }
    }

    match &cli.query {
        Some(query) => {
            if session.execute(query) {
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
