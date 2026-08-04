use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use comfy_table::Table;
use voir_catalog::{Catalog, SourceRegistry, TableProvider};
use voir_plan::{Binder, optimize};
use voir_source_csv::CsvTableProvider;
use voir_types::VoirError;

use crate::diagnostics;
use crate::format::{self, OutputFormat};

pub struct Session {
    pub catalog: Catalog,
    pub format: OutputFormat,
    pub timing: bool,
    pub explain: bool,
    pub output: Option<PathBuf>,
}

impl Session {
    pub fn new() -> Self {
        let mut registry = SourceRegistry::new();
        registry.register("csv", |path| {
            Ok(Arc::new(CsvTableProvider::open(path)?) as Arc<dyn TableProvider>)
        });
        Self {
            catalog: Catalog::new(registry),
            format: default_format(),
            timing: false,
            explain: false,
            output: None,
        }
    }

    /// Runs `sql`, printing results (or an error) and returning whether it
    /// succeeded — the exit-code signal for non-interactive mode.
    pub fn execute(&mut self, sql: &str) -> bool {
        let start = Instant::now();
        let ok = match self.run_query(sql) {
            Ok(()) => true,
            Err(err) => {
                diagnostics::report(sql, &err);
                false
            }
        };
        if self.timing {
            println!("Time: {:.3} ms", start.elapsed().as_secs_f64() * 1000.0);
        }
        ok
    }

    fn run_query(&mut self, sql: &str) -> Result<(), VoirError> {
        let statement = voir_sql::parse(sql)?;
        let (logical_plan, is_explain_stmt) = Binder::new(&self.catalog).bind(&statement)?;
        let logical_plan = optimize(logical_plan);

        if self.explain || is_explain_stmt {
            print!("{logical_plan}");
        }
        if is_explain_stmt {
            return Ok(());
        }

        let schema = logical_plan.schema();
        let mut operator = voir_exec::build(&logical_plan)?;
        let mut writer = format::writer(self.format, &schema, self.output.as_deref())?;
        while let Some(batch) = operator.next_batch()? {
            writer.write_batch(&batch)?;
        }
        writer.finish()
    }

    pub fn cmd_open(&mut self, args: &str) {
        let mut parts = args.split_whitespace();
        let Some(path_str) = parts.next() else {
            eprintln!("usage: \\open <path> [AS <name>]");
            return;
        };
        let mut name = None;
        if let Some(kw) = parts.next() {
            if kw.eq_ignore_ascii_case("as") {
                name = parts.next().map(str::to_string);
            }
        }
        let path = PathBuf::from(path_str);
        let name = name.unwrap_or_else(|| stem_name(&path));
        match self.catalog.open(&path) {
            Ok(provider) => {
                self.catalog.register_table(&name, path, provider);
                println!("registered table \"{name}\"");
            }
            Err(err) => eprintln!("error: {err}"),
        }
    }

    pub fn cmd_list_tables(&self) {
        let mut rows: Vec<_> = self.catalog.tables().collect();
        if rows.is_empty() {
            println!("No tables registered. Use \\open <path> [AS <name>].");
            return;
        }
        rows.sort_by(|a, b| a.0.cmp(b.0));
        let mut table = Table::new();
        table.set_header(vec!["Name", "Path"]);
        for (name, reg) in rows {
            table.add_row(vec![name.to_string(), reg.path.display().to_string()]);
        }
        println!("{table}");
    }

    pub fn cmd_describe(&self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            eprintln!("usage: \\d <table>");
            return;
        }
        let Some(reg) = self.catalog.table(name) else {
            eprintln!("no such table \"{name}\" (see \\dt)");
            return;
        };
        let schema = reg.provider.schema();
        let mut table = Table::new();
        table.set_header(vec!["Column", "Type", "Nullable"]);
        for field in &schema.fields {
            table.add_row(vec![
                field.name.clone(),
                format!("{:?}", field.data_type),
                field.nullable.to_string(),
            ]);
        }
        println!("{table}");
    }

    pub fn cmd_format(&mut self, arg: &str) {
        let arg = arg.trim();
        if arg.is_empty() {
            println!("format: {}", self.format.name());
            return;
        }
        match OutputFormat::parse(arg) {
            Some(f) => {
                self.format = f;
                println!("format set to {}", f.name());
            }
            None => eprintln!("unknown format '{arg}' (expected table|csv|tsv|json|ndjson)"),
        }
    }

    pub fn cmd_timing(&mut self) {
        self.timing = !self.timing;
        println!("timing is {}", if self.timing { "on" } else { "off" });
    }

    pub fn cmd_explain_toggle(&mut self) {
        self.explain = !self.explain;
        println!("explain is {}", if self.explain { "on" } else { "off" });
    }

    pub fn cmd_output(&mut self, arg: &str) {
        let arg = arg.trim();
        if arg.is_empty() {
            if self.output.take().is_some() {
                println!("output reset to stdout");
            } else {
                println!("output is stdout");
            }
            return;
        }
        self.output = Some(PathBuf::from(arg));
        println!("output redirected to {arg}");
    }

    pub fn cmd_include(&mut self, arg: &str) {
        let path = arg.trim();
        if path.is_empty() {
            eprintln!("usage: \\i <file>");
            return;
        }
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(err) => {
                eprintln!("error: {path}: {err}");
                return;
            }
        };
        match crate::statement::split(&content) {
            Ok(statements) => {
                for stmt in statements {
                    if !self.execute(&stmt) {
                        break;
                    }
                }
            }
            Err(err) => diagnostics::report(&content, &err),
        }
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

fn stem_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("table")
        .to_string()
}

fn default_format() -> OutputFormat {
    if io::stdout().is_terminal() {
        OutputFormat::Table
    } else {
        OutputFormat::Csv
    }
}
