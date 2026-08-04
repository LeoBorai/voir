use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use comfy_table::{Cell, CellAlignment, Color, Table};
use voir_types::{Batch, SchemaRef, Value, VoirError};

/// Cells beyond this width are truncated with an ellipsis so one wide column
/// doesn't blow out the whole table.
const MAX_CELL_WIDTH: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Table,
    Csv,
    Tsv,
    Json,
    NdJson,
}

impl OutputFormat {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "table" => Some(Self::Table),
            "csv" => Some(Self::Csv),
            "tsv" => Some(Self::Tsv),
            "json" => Some(Self::Json),
            "ndjson" => Some(Self::NdJson),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Csv => "csv",
            Self::Tsv => "tsv",
            Self::Json => "json",
            Self::NdJson => "ndjson",
        }
    }
}

pub trait ResultWriter {
    fn write_batch(&mut self, batch: &Batch) -> Result<(), VoirError>;
    fn finish(&mut self) -> Result<(), VoirError>;
}

pub fn writer(
    format: OutputFormat,
    schema: &SchemaRef,
    output: Option<&Path>,
) -> Result<Box<dyn ResultWriter>, VoirError> {
    let sink: Box<dyn Write> = match output {
        Some(path) => Box::new(BufWriter::new(File::create(path)?)),
        None => Box::new(io::stdout()),
    };
    Ok(match format {
        OutputFormat::Table => Box::new(TableWriter::new(schema, sink)),
        OutputFormat::Csv => Box::new(DelimitedWriter::new(schema, sink, b',')?),
        OutputFormat::Tsv => Box::new(DelimitedWriter::new(schema, sink, b'\t')?),
        OutputFormat::Json => Box::new(JsonWriter::new(schema, sink, false)),
        OutputFormat::NdJson => Box::new(JsonWriter::new(schema, sink, true)),
    })
}

struct TableWriter {
    table: Table,
    sink: Box<dyn Write>,
}

impl TableWriter {
    fn new(schema: &SchemaRef, sink: Box<dyn Write>) -> Self {
        let mut table = Table::new();
        table.set_header(schema.fields.iter().map(|f| f.name.clone()));
        Self { table, sink }
    }
}

impl ResultWriter for TableWriter {
    fn write_batch(&mut self, batch: &Batch) -> Result<(), VoirError> {
        for row in &batch.rows {
            self.table.add_row(row.0.iter().map(render_cell));
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), VoirError> {
        writeln!(self.sink, "{}", self.table)?;
        Ok(())
    }
}

fn render_cell(value: &Value) -> Cell {
    match value {
        Value::Null => Cell::new("NULL").fg(Color::DarkGrey),
        Value::Boolean(b) => Cell::new(b.to_string()),
        Value::Int64(v) => Cell::new(v.to_string()).set_alignment(CellAlignment::Right),
        Value::Float64(v) => Cell::new(v.to_string()).set_alignment(CellAlignment::Right),
        Value::Utf8(s) => Cell::new(truncate(s)),
        Value::Date(d) => Cell::new(d.to_string()),
        Value::Timestamp(t) => Cell::new(t.to_string()),
    }
}

fn truncate(s: &str) -> String {
    if s.chars().count() <= MAX_CELL_WIDTH {
        return s.to_string();
    }
    let head: String = s.chars().take(MAX_CELL_WIDTH.saturating_sub(1)).collect();
    format!("{head}…")
}

struct DelimitedWriter {
    writer: csv::Writer<Box<dyn Write>>,
}

impl DelimitedWriter {
    fn new(schema: &SchemaRef, sink: Box<dyn Write>, delimiter: u8) -> Result<Self, VoirError> {
        let mut writer = csv::WriterBuilder::new()
            .delimiter(delimiter)
            .from_writer(sink);
        writer
            .write_record(schema.fields.iter().map(|f| f.name.as_str()))
            .map_err(|e| VoirError::Exec(e.to_string()))?;
        Ok(Self { writer })
    }
}

impl ResultWriter for DelimitedWriter {
    fn write_batch(&mut self, batch: &Batch) -> Result<(), VoirError> {
        for row in &batch.rows {
            let fields: Vec<String> = row.0.iter().map(render_plain).collect();
            self.writer
                .write_record(&fields)
                .map_err(|e| VoirError::Exec(e.to_string()))?;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), VoirError> {
        self.writer.flush().map_err(VoirError::Io)?;
        Ok(())
    }
}

fn render_plain(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Boolean(b) => b.to_string(),
        Value::Int64(v) => v.to_string(),
        Value::Float64(v) => v.to_string(),
        Value::Utf8(s) => s.to_string(),
        Value::Date(d) => d.to_string(),
        Value::Timestamp(t) => t.to_string(),
    }
}

struct JsonWriter {
    fields: Vec<String>,
    sink: Box<dyn Write>,
    ndjson: bool,
    wrote_any: bool,
}

impl JsonWriter {
    fn new(schema: &SchemaRef, sink: Box<dyn Write>, ndjson: bool) -> Self {
        Self {
            fields: schema.fields.iter().map(|f| f.name.clone()).collect(),
            sink,
            ndjson,
            wrote_any: false,
        }
    }
}

impl ResultWriter for JsonWriter {
    fn write_batch(&mut self, batch: &Batch) -> Result<(), VoirError> {
        for row in &batch.rows {
            let object: serde_json::Map<String, serde_json::Value> = self
                .fields
                .iter()
                .cloned()
                .zip(row.0.iter().map(value_to_json))
                .collect();
            let line = serde_json::Value::Object(object).to_string();
            if self.ndjson {
                writeln!(self.sink, "{line}")?;
            } else {
                if self.wrote_any {
                    writeln!(self.sink, ",")?;
                } else {
                    writeln!(self.sink, "[")?;
                }
                write!(self.sink, "  {line}")?;
            }
            self.wrote_any = true;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), VoirError> {
        if self.ndjson {
            return Ok(());
        }
        if self.wrote_any {
            writeln!(self.sink)?;
        } else {
            write!(self.sink, "[")?;
        }
        writeln!(self.sink, "]")?;
        Ok(())
    }
}

fn value_to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Boolean(b) => serde_json::Value::Bool(*b),
        Value::Int64(v) | Value::Timestamp(v) => serde_json::Value::from(*v),
        Value::Float64(v) => serde_json::Number::from_f64(*v)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        Value::Utf8(s) => serde_json::Value::String(s.to_string()),
        Value::Date(v) => serde_json::Value::from(*v),
    }
}
