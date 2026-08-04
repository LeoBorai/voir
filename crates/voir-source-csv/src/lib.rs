use std::fs::File;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use csv::{ReaderBuilder, StringRecord};
use voir_catalog::{BatchStream, FilterExpr, TableProvider};
use voir_types::{Batch, DataType, Field, Row, Schema, SchemaRef, Value, VoirError};

/// Rows sampled to infer column types before the real scan starts. The
/// sample is discarded — the query still reads the whole file itself.
const SAMPLE_SIZE: usize = 1000;

pub struct CsvTableProvider {
    path: PathBuf,
    schema: SchemaRef,
    has_header: bool,
    delimiter: u8,
}

impl CsvTableProvider {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, VoirError> {
        let path = path.into();
        let delimiter = sniff_delimiter(&path)?;
        let has_header = true;
        let schema = infer_schema(&path, delimiter, has_header)?;
        Ok(Self { path, schema: Arc::new(schema), has_header, delimiter })
    }
}

impl TableProvider for CsvTableProvider {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    // Nothing meaningful is possible for plain CSV — there's no index to
    // consult — so filters are always returned unapplied and left to the
    // engine's Filter operator.
    fn scan(&self, projection: Option<&[usize]>, _filters: &[FilterExpr]) -> Result<Box<dyn BatchStream>, VoirError> {
        let reader = ReaderBuilder::new()
            .delimiter(self.delimiter)
            .has_headers(self.has_header)
            .from_path(&self.path)
            .map_err(|e| VoirError::Source(format!("{}: {e}", self.path.display())))?;

        let output_schema: SchemaRef = match projection {
            None => self.schema.clone(),
            Some(indices) => Arc::new(Schema::new(indices.iter().map(|&i| self.schema.fields[i].clone()).collect())),
        };

        Ok(Box::new(CsvBatchStream {
            reader,
            source_schema: self.schema.clone(),
            output_schema,
            projection: projection.map(<[usize]>::to_vec),
        }))
    }
}

struct CsvBatchStream {
    reader: csv::Reader<File>,
    source_schema: SchemaRef,
    output_schema: SchemaRef,
    projection: Option<Vec<usize>>,
}

impl BatchStream for CsvBatchStream {
    fn next_batch(&mut self) -> Result<Option<Batch>, VoirError> {
        let mut rows = Vec::with_capacity(Batch::MAX_ROWS);
        let mut record = StringRecord::new();
        while rows.len() < Batch::MAX_ROWS {
            let read = self.reader.read_record(&mut record).map_err(|e| VoirError::Source(e.to_string()))?;
            if !read {
                break;
            }
            let values = match &self.projection {
                None => (0..self.source_schema.fields.len())
                    .map(|i| parse_field(record.get(i), self.source_schema.fields[i].data_type))
                    .collect(),
                Some(indices) => indices
                    .iter()
                    .map(|&i| parse_field(record.get(i), self.source_schema.fields[i].data_type))
                    .collect(),
            };
            rows.push(Row(values));
        }
        if rows.is_empty() {
            Ok(None)
        } else {
            Ok(Some(Batch::new(self.output_schema.clone(), rows)))
        }
    }
}

fn parse_field(raw: Option<&str>, data_type: DataType) -> Value {
    let raw = raw.unwrap_or("");
    if raw.is_empty() {
        return Value::Null;
    }
    match data_type {
        DataType::Int64 => raw.parse::<i64>().map(Value::Int64).unwrap_or(Value::Null),
        DataType::Float64 => raw.parse::<f64>().map(Value::Float64).unwrap_or(Value::Null),
        DataType::Boolean => parse_bool(raw).map(Value::Boolean).unwrap_or(Value::Null),
        _ => Value::utf8(raw),
    }
}

fn parse_bool(raw: &str) -> Option<bool> {
    match raw.to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn sniff_delimiter(path: &Path) -> Result<u8, VoirError> {
    let file = File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let mut first_line = String::new();
    reader.read_line(&mut first_line)?;
    let candidates = [b',', b';', b'\t', b'|'];
    Ok(candidates
        .iter()
        .map(|&d| (d, first_line.bytes().filter(|&b| b == d).count()))
        .max_by_key(|&(_, count)| count)
        .filter(|&(_, count)| count > 0)
        .map_or(b',', |(d, _)| d))
}

fn infer_schema(path: &Path, delimiter: u8, has_header: bool) -> Result<Schema, VoirError> {
    let mut reader = ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(has_header)
        .from_path(path)
        .map_err(|e| VoirError::Source(format!("{}: {e}", path.display())))?;

    let names: Vec<String> = if has_header {
        reader.headers().map_err(|e| VoirError::Source(e.to_string()))?.iter().map(str::to_string).collect()
    } else {
        Vec::new()
    };

    let mut samples: Vec<StringRecord> = Vec::new();
    let mut record = StringRecord::new();
    while samples.len() < SAMPLE_SIZE {
        if !reader.read_record(&mut record).map_err(|e| VoirError::Source(e.to_string()))? {
            break;
        }
        samples.push(record.clone());
    }

    let num_columns = names.len().max(samples.first().map_or(0, StringRecord::len));
    if num_columns == 0 {
        return Err(VoirError::Source(format!("{}: empty file, cannot infer schema", path.display())));
    }

    let mut fields = Vec::with_capacity(num_columns);
    for col in 0..num_columns {
        let name = names.get(col).cloned().unwrap_or_else(|| format!("column_{}", col + 1));
        let mut data_type: Option<DataType> = None;
        let mut nullable = false;
        for record in &samples {
            let raw = record.get(col).unwrap_or("");
            if raw.is_empty() {
                nullable = true;
                continue;
            }
            let inferred = infer_value_type(raw);
            data_type = Some(match data_type {
                None => inferred,
                Some(current) => widen(current, inferred),
            });
        }
        fields.push(Field::new(name, data_type.unwrap_or(DataType::Utf8), nullable));
    }

    Ok(Schema::new(fields))
}

fn infer_value_type(raw: &str) -> DataType {
    if looks_like_leading_zero(raw) {
        return DataType::Utf8;
    }
    if raw.parse::<i64>().is_ok() {
        return DataType::Int64;
    }
    if raw.parse::<f64>().is_ok() {
        return DataType::Float64;
    }
    if parse_bool(raw).is_some() {
        return DataType::Boolean;
    }
    DataType::Utf8
}

// Zip codes, phone numbers: `00123` parsed as Int64 silently loses data.
// `--preserve-leading-zeros` would gate this in a fuller build; for now it's
// always on.
fn looks_like_leading_zero(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    bytes.len() > 1 && bytes[0] == b'0' && bytes.iter().all(u8::is_ascii_digit)
}

fn widen(current: DataType, incoming: DataType) -> DataType {
    use DataType::{Float64, Int64, Utf8};
    match (current, incoming) {
        (a, b) if a == b => a,
        (Utf8, _) | (_, Utf8) => Utf8,
        (Int64, Float64) | (Float64, Int64) => Float64,
        _ => Utf8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_csv(contents: &str) -> tempfile::NamedTempFile {
        let mut file = tempfile::Builder::new().suffix(".csv").tempfile().unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        file.flush().unwrap();
        file
    }

    #[test]
    fn infers_types_and_nullability() {
        let file = write_csv("id,name,score\n1,alice,9.5\n2,bob,\n007,carol,7\n");
        let provider = CsvTableProvider::open(file.path()).unwrap();
        let schema = provider.schema();
        assert_eq!(schema.fields[0].name, "id");
        assert_eq!(schema.fields[0].data_type, DataType::Utf8); // leading zero -> Utf8
        assert_eq!(schema.fields[1].data_type, DataType::Utf8);
        assert_eq!(schema.fields[2].data_type, DataType::Float64);
        assert!(schema.fields[2].nullable);
    }

    #[test]
    fn scan_reads_all_rows() {
        let file = write_csv("a,b\n1,2\n3,4\n5,6\n");
        let provider = CsvTableProvider::open(file.path()).unwrap();
        let mut stream = provider.scan(None, &[]).unwrap();
        let mut total = 0;
        while let Some(batch) = stream.next_batch().unwrap() {
            total += batch.rows.len();
        }
        assert_eq!(total, 3);
    }

    #[test]
    fn projection_pushdown_selects_subset() {
        let file = write_csv("a,b,c\n1,2,3\n");
        let provider = CsvTableProvider::open(file.path()).unwrap();
        let mut stream = provider.scan(Some(&[2, 0]), &[]).unwrap();
        let batch = stream.next_batch().unwrap().unwrap();
        assert_eq!(batch.rows[0].0, vec![Value::Int64(3), Value::Int64(1)]);
    }
}
