use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use comfy_table::{Cell, CellAlignment, Table};
use voir_catalog::{SourceRegistry, TableProvider};
use voir_plan::{Binder, optimize};
use voir_source_csv::CsvTableProvider;
use voir_types::{Value, VoirError};

#[derive(Debug, Parser)]
#[command(
    name = "voir",
    version,
    about = "SQL over tabular files",
    next_line_help = true
)]
struct Cli {
    /// SQL query to run, e.g. SELECT * FROM 'sales.csv' WHERE region = 'EU'
    query: String,

    /// Print the logical plan before running the query
    #[arg(long)]
    explain: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::from(1)
        }
    }
}

fn build_registry() -> SourceRegistry {
    let mut registry = SourceRegistry::new();
    registry.register("csv", |path| {
        Ok(Arc::new(CsvTableProvider::open(path)?) as Arc<dyn TableProvider>)
    });
    registry
}

fn run(cli: &Cli) -> Result<(), VoirError> {
    let registry = build_registry();

    let statement = voir_sql::parse(&cli.query)?;
    let (logical_plan, is_explain_stmt) = Binder::new(&registry).bind(&statement)?;
    let logical_plan = optimize(logical_plan);

    if cli.explain || is_explain_stmt {
        print!("{logical_plan}");
    }
    if is_explain_stmt {
        return Ok(());
    }

    let schema = logical_plan.schema();
    let mut operator = voir_exec::build(&logical_plan)?;

    let mut table = Table::new();
    table.set_header(schema.fields.iter().map(|f| f.name.clone()));

    while let Some(batch) = operator.next_batch()? {
        for row in &batch.rows {
            table.add_row(row.0.iter().map(render_cell));
        }
    }

    println!("{table}");
    Ok(())
}

fn render_cell(value: &Value) -> Cell {
    match value {
        Value::Null => Cell::new("NULL"),
        Value::Boolean(b) => Cell::new(b.to_string()),
        Value::Int64(v) => Cell::new(v.to_string()).set_alignment(CellAlignment::Right),
        Value::Float64(v) => Cell::new(v.to_string()).set_alignment(CellAlignment::Right),
        Value::Utf8(s) => Cell::new(s.as_ref()),
        Value::Date(d) => Cell::new(d.to_string()),
        Value::Timestamp(t) => Cell::new(t.to_string()),
    }
}
