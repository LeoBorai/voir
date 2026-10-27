use std::fs::File;
use std::io::{self, BufWriter, IsTerminal, Write};
use std::path::Path;

use comfy_table::presets::ASCII_MARKDOWN;
use comfy_table::{Attribute, Cell, CellAlignment, Color, Table};
use nu_ansi_term::Style;
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
    Markdown,
}

impl OutputFormat {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "table" => Some(Self::Table),
            "csv" => Some(Self::Csv),
            "tsv" => Some(Self::Tsv),
            "json" => Some(Self::Json),
            "ndjson" => Some(Self::NdJson),
            "markdown" | "md" => Some(Self::Markdown),
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
            Self::Markdown => "markdown",
        }
    }
}

pub trait ResultWriter {
    fn write_batch(&mut self, batch: &Batch) -> Result<(), VoirError>;
    fn finish(&mut self) -> Result<(), VoirError>;
}

/// Whether stdout should carry ANSI colors: a terminal, and the user hasn't
/// opted out through `NO_COLOR` (<https://no-color.org>).
pub fn color_enabled() -> bool {
    io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

/// How a writer decorates its output. Both flags are off whenever results go
/// to a file or a pipe, so scripts and `\o` targets get byte-for-byte the
/// plain format; a human at a terminal gets indented JSON and colors.
#[derive(Debug, Clone, Copy)]
struct Decor {
    /// Indent JSON and add whitespace meant for reading, not parsing.
    pretty: bool,
    /// Wrap values in ANSI color codes.
    color: bool,
}

pub fn writer(
    format: OutputFormat,
    schema: &SchemaRef,
    output: Option<&Path>,
) -> Result<Box<dyn ResultWriter>, VoirError> {
    let (sink, decor): (Box<dyn Write>, _) = match output {
        Some(path) => (
            Box::new(BufWriter::new(File::create(path)?)),
            Decor {
                pretty: false,
                color: false,
            },
        ),
        None => (
            Box::new(io::stdout()),
            Decor {
                pretty: io::stdout().is_terminal(),
                color: color_enabled(),
            },
        ),
    };
    Ok(match format {
        OutputFormat::Table => Box::new(TableWriter::new(schema, sink, false, output, decor)),
        OutputFormat::Csv => Box::new(DelimitedWriter::new(schema, sink, b',', decor)?),
        OutputFormat::Tsv => Box::new(DelimitedWriter::new(schema, sink, b'\t', decor)?),
        OutputFormat::Json => Box::new(JsonWriter::new(schema, sink, false, decor)),
        OutputFormat::NdJson => Box::new(JsonWriter::new(schema, sink, true, decor)),
        OutputFormat::Markdown => Box::new(TableWriter::new(schema, sink, true, output, decor)),
    })
}

/// Shared palette so every format paints the same kind of value the same way.
mod palette {
    use nu_ansi_term::{Color, Style};

    pub fn header() -> Style {
        Style::new().fg(Color::LightBlue).bold()
    }
    pub fn key() -> Style {
        Style::new().fg(Color::LightBlue).bold()
    }
    pub fn null() -> Style {
        Style::new().fg(Color::DarkGray).italic()
    }
    pub fn boolean() -> Style {
        Style::new().fg(Color::Yellow)
    }
    pub fn number() -> Style {
        Style::new().fg(Color::LightMagenta)
    }
    pub fn string() -> Style {
        Style::new().fg(Color::Green)
    }
    pub fn temporal() -> Style {
        Style::new().fg(Color::Cyan)
    }
    pub fn punct() -> Style {
        Style::new().fg(Color::DarkGray)
    }
}

/// Style for a value in the delimited formats; strings stay unstyled there so
/// a mostly-text result doesn't turn into a wall of color.
fn value_style(value: &Value) -> Style {
    match value {
        Value::Null => palette::null(),
        Value::Boolean(_) => palette::boolean(),
        Value::Int64(_) | Value::Float64(_) => palette::number(),
        Value::Utf8(_) => Style::new(),
        Value::Date(_) | Value::Timestamp(_) => palette::temporal(),
    }
}

struct TableWriter {
    table: Table,
    sink: Box<dyn Write>,
    color: bool,
}

impl TableWriter {
    fn new(
        schema: &SchemaRef,
        sink: Box<dyn Write>,
        markdown: bool,
        output: Option<&Path>,
        decor: Decor,
    ) -> Self {
        let mut table = Table::new();
        if markdown {
            table.load_preset(ASCII_MARKDOWN);
        }
        // comfy-table sniffs stdout on its own; when writing to a file that
        // would leak escape codes (and terminal-width wrapping) into it.
        if output.is_some() {
            table.force_no_tty();
        }
        table.set_header(schema.fields.iter().map(|f| {
            let cell = Cell::new(&f.name);
            if decor.color {
                cell.fg(Color::Blue).add_attribute(Attribute::Bold)
            } else {
                cell
            }
        }));
        Self {
            table,
            sink,
            color: decor.color,
        }
    }
}

impl ResultWriter for TableWriter {
    fn write_batch(&mut self, batch: &Batch) -> Result<(), VoirError> {
        for row in &batch.rows {
            self.table
                .add_row(row.0.iter().map(|v| render_cell(v, self.color)));
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), VoirError> {
        writeln!(self.sink, "{}", self.table)?;
        Ok(())
    }
}

fn render_cell(value: &Value, color: bool) -> Cell {
    let (cell, fg) = match value {
        Value::Null => (
            Cell::new("NULL").add_attribute(Attribute::Italic),
            Some(Color::DarkGrey),
        ),
        Value::Boolean(b) => (Cell::new(b.to_string()), Some(Color::Yellow)),
        Value::Int64(v) => (
            Cell::new(v.to_string()).set_alignment(CellAlignment::Right),
            Some(Color::Magenta),
        ),
        Value::Float64(v) => (
            Cell::new(v.to_string()).set_alignment(CellAlignment::Right),
            Some(Color::Magenta),
        ),
        Value::Utf8(s) => (Cell::new(truncate(s)), None),
        Value::Date(d) => (Cell::new(d.to_string()), Some(Color::Cyan)),
        Value::Timestamp(t) => (Cell::new(t.to_string()), Some(Color::Cyan)),
    };
    match fg {
        Some(fg) if color => cell.fg(fg),
        _ => cell,
    }
}

fn truncate(s: &str) -> String {
    if s.chars().count() <= MAX_CELL_WIDTH {
        return s.to_string();
    }
    let head: String = s.chars().take(MAX_CELL_WIDTH.saturating_sub(1)).collect();
    format!("{head}…")
}

/// Plain mode streams straight through the `csv` writer. Colored mode quotes
/// each field with a throwaway `csv` writer first and paints the result, so
/// escape codes never end up inside quotes (where they'd change the data)
/// and delimiters stay uncolored.
struct DelimitedWriter {
    out: DelimitedSink,
    delimiter: u8,
}

enum DelimitedSink {
    Plain(Box<csv::Writer<Box<dyn Write>>>),
    Colored(Box<dyn Write>),
}

impl DelimitedWriter {
    fn new(
        schema: &SchemaRef,
        sink: Box<dyn Write>,
        delimiter: u8,
        decor: Decor,
    ) -> Result<Self, VoirError> {
        let out = if decor.color {
            DelimitedSink::Colored(sink)
        } else {
            DelimitedSink::Plain(Box::new(
                csv::WriterBuilder::new()
                    .delimiter(delimiter)
                    .from_writer(sink),
            ))
        };
        let mut this = Self { out, delimiter };
        let header = schema
            .fields
            .iter()
            .map(|f| (f.name.as_str(), palette::header()));
        this.write_fields(header)?;
        Ok(this)
    }

    fn write_fields<'a>(
        &mut self,
        fields: impl Iterator<Item = (&'a str, Style)>,
    ) -> Result<(), VoirError> {
        let sink = match &mut self.out {
            DelimitedSink::Plain(writer) => {
                return writer
                    .write_record(fields.map(|(field, _)| field))
                    .map_err(|e| VoirError::Exec(e.to_string()));
            }
            DelimitedSink::Colored(sink) => sink,
        };
        let separator = char::from(self.delimiter).to_string();
        let mut line = String::new();
        for (i, (field, style)) in fields.enumerate() {
            if i > 0 {
                line.push_str(&separator);
            }
            let quoted = quote_field(field, self.delimiter)?;
            if quoted.is_empty() {
                continue;
            }
            line.push_str(&style.paint(quoted).to_string());
        }
        writeln!(sink, "{line}")?;
        Ok(())
    }
}

/// Runs a single field through the `csv` writer to get exactly the quoting
/// and escaping plain mode would produce for it.
fn quote_field(field: &str, delimiter: u8) -> Result<String, VoirError> {
    // A lone empty field gets quoted (`""`) so the record isn't a blank line;
    // inside a multi-field record it's written bare, which is what we want.
    if field.is_empty() {
        return Ok(String::new());
    }
    let mut writer = csv::WriterBuilder::new()
        .delimiter(delimiter)
        .terminator(csv::Terminator::Any(b'\n'))
        .from_writer(Vec::new());
    writer
        .write_record([field])
        .map_err(|e| VoirError::Exec(e.to_string()))?;
    let bytes = writer
        .into_inner()
        .map_err(|e| VoirError::Exec(e.to_string()))?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    text.pop(); // record terminator
    Ok(text)
}

impl ResultWriter for DelimitedWriter {
    fn write_batch(&mut self, batch: &Batch) -> Result<(), VoirError> {
        for row in &batch.rows {
            let fields: Vec<(String, Style)> = row
                .0
                .iter()
                .map(|v| (render_plain(v), value_style(v)))
                .collect();
            self.write_fields(fields.iter().map(|(s, style)| (s.as_str(), *style)))?;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), VoirError> {
        match &mut self.out {
            DelimitedSink::Plain(writer) => writer.flush()?,
            DelimitedSink::Colored(sink) => sink.flush()?,
        }
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
    decor: Decor,
    wrote_any: bool,
}

impl JsonWriter {
    fn new(schema: &SchemaRef, sink: Box<dyn Write>, ndjson: bool, decor: Decor) -> Self {
        // NDJSON is one record per line by definition, so it never indents.
        let decor = Decor {
            pretty: decor.pretty && !ndjson,
            ..decor
        };
        Self {
            fields: schema.fields.iter().map(|f| f.name.clone()).collect(),
            sink,
            ndjson,
            decor,
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
            let mut line = String::new();
            render_json(&mut line, &serde_json::Value::Object(object), 1, self.decor);
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

/// Serializes `value` like `serde_json`'s compact form, optionally indented
/// (`depth` levels of two spaces for nested lines) and colored jq-style.
fn render_json(out: &mut String, value: &serde_json::Value, depth: usize, decor: Decor) {
    let paint = |out: &mut String, style: Style, text: &str| {
        if decor.color {
            out.push_str(&style.paint(text).to_string());
        } else {
            out.push_str(text);
        }
    };
    let newline = |out: &mut String, depth: usize| {
        if decor.pretty {
            out.push('\n');
            out.push_str(&"  ".repeat(depth));
        }
    };
    match value {
        serde_json::Value::Null => paint(out, palette::null(), "null"),
        serde_json::Value::Bool(b) => paint(out, palette::boolean(), &b.to_string()),
        serde_json::Value::Number(n) => paint(out, palette::number(), &n.to_string()),
        serde_json::Value::String(_) => paint(out, palette::string(), &value.to_string()),
        serde_json::Value::Array(items) => {
            paint(out, palette::punct(), "[");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    paint(out, palette::punct(), ",");
                }
                newline(out, depth + 1);
                render_json(out, item, depth + 1, decor);
            }
            if !items.is_empty() {
                newline(out, depth);
            }
            paint(out, palette::punct(), "]");
        }
        serde_json::Value::Object(map) => {
            paint(out, palette::punct(), "{");
            for (i, (key, item)) in map.iter().enumerate() {
                if i > 0 {
                    paint(out, palette::punct(), ",");
                }
                newline(out, depth + 1);
                let key = serde_json::Value::String(key.clone()).to_string();
                paint(out, palette::key(), &key);
                paint(out, palette::punct(), ":");
                if decor.pretty {
                    out.push(' ');
                }
                render_json(out, item, depth + 1, decor);
            }
            if !map.is_empty() {
                newline(out, depth);
            }
            paint(out, palette::punct(), "}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Decor, quote_field, render_json};

    fn render(value: &serde_json::Value, pretty: bool) -> String {
        let mut out = String::new();
        render_json(
            &mut out,
            value,
            0,
            Decor {
                pretty,
                color: false,
            },
        );
        out
    }

    #[test]
    fn compact_json_matches_serde() {
        let value = serde_json::json!({"a": 1, "b": "x\"y", "c": null, "d": [true, 1.5], "e": {}});
        assert_eq!(render(&value, false), value.to_string());
    }

    #[test]
    fn pretty_json_matches_serde() {
        let value = serde_json::json!({"a": 1, "b": "x", "c": [true, null], "d": []});
        assert_eq!(
            render(&value, true),
            serde_json::to_string_pretty(&value).unwrap()
        );
    }

    #[test]
    fn quote_field_matches_csv_quoting() {
        assert_eq!(quote_field("plain", b',').unwrap(), "plain");
        assert_eq!(quote_field("", b',').unwrap(), "");
        assert_eq!(quote_field("a,b", b',').unwrap(), "\"a,b\"");
        assert_eq!(quote_field("a,b", b'\t').unwrap(), "a,b");
        assert_eq!(
            quote_field("say \"hi\"", b',').unwrap(),
            "\"say \"\"hi\"\"\""
        );
    }
}
