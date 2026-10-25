use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use serde_json::{Map, Value as Json};
use voir_catalog::{BatchStream, FilterExpr, TableProvider};
use voir_types::{Batch, DataType, Field, Row, Schema, SchemaRef, Value, VoirError};

/// Reads `.json` and `.jsonc` files. Accepted shapes: an array of objects, a
/// single object (one row), or newline-delimited objects. `.jsonc` features
/// (`//` and `/* */` comments, trailing commas) are accepted in both
/// extensions. Nested arrays/objects surface as JSON text in a `Utf8` column.
///
/// The file is parsed once at `open` and held in memory; JSON has no
/// streaming-friendly layout for the array form.
pub struct JsonTableProvider {
    schema: SchemaRef,
    rows: Arc<Vec<Vec<Json>>>,
}

impl JsonTableProvider {
    /// # Errors
    ///
    /// Returns an error if the file cannot be read, is not valid JSON/JSONC,
    /// or does not contain objects.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, VoirError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)?;
        let objects = parse_objects(&text)
            .map_err(|e| VoirError::Source(format!("{}: {e}", path.display())))?;
        if objects.is_empty() {
            return Err(VoirError::Source(format!(
                "{}: no records, cannot infer schema",
                path.display()
            )));
        }

        let mut names: Vec<String> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        for obj in &objects {
            for key in obj.keys() {
                if !index.contains_key(key) {
                    index.insert(key.clone(), names.len());
                    names.push(key.clone());
                }
            }
        }

        let rows: Vec<Vec<Json>> = objects
            .into_iter()
            .map(|mut obj| {
                names
                    .iter()
                    .map(|n| obj.remove(n).unwrap_or(Json::Null))
                    .collect()
            })
            .collect();

        let fields = names
            .into_iter()
            .enumerate()
            .map(|(col, name)| {
                let mut data_type: Option<DataType> = None;
                let mut nullable = false;
                for row in &rows {
                    match &row[col] {
                        Json::Null => nullable = true,
                        v => {
                            let t = json_type(v);
                            data_type = Some(data_type.map_or(t, |c| widen(c, t)));
                        }
                    }
                }
                Field::new(name, data_type.unwrap_or(DataType::Utf8), nullable)
            })
            .collect();

        Ok(Self {
            schema: Arc::new(Schema::new(fields)),
            rows: Arc::new(rows),
        })
    }
}

impl TableProvider for JsonTableProvider {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    // No index to consult: filters are returned unapplied for the engine's
    // Filter operator.
    fn scan(
        &self,
        projection: Option<&[usize]>,
        _filters: &[FilterExpr],
    ) -> Result<Box<dyn BatchStream>, VoirError> {
        let output_schema: SchemaRef = match projection {
            None => self.schema.clone(),
            Some(indices) => Arc::new(Schema::new(
                indices
                    .iter()
                    .map(|&i| self.schema.fields[i].clone())
                    .collect(),
            )),
        };
        Ok(Box::new(JsonBatchStream {
            rows: self.rows.clone(),
            source_schema: self.schema.clone(),
            output_schema,
            projection: projection.map(<[usize]>::to_vec),
            pos: 0,
        }))
    }
}

struct JsonBatchStream {
    rows: Arc<Vec<Vec<Json>>>,
    source_schema: SchemaRef,
    output_schema: SchemaRef,
    projection: Option<Vec<usize>>,
    pos: usize,
}

impl BatchStream for JsonBatchStream {
    fn next_batch(&mut self) -> Result<Option<Batch>, VoirError> {
        let end = (self.pos + Batch::MAX_ROWS).min(self.rows.len());
        if self.pos >= end {
            return Ok(None);
        }
        let convert =
            |src: &[Json], i: usize| to_value(&src[i], self.source_schema.fields[i].data_type);
        let rows = self.rows[self.pos..end]
            .iter()
            .map(|src| match &self.projection {
                None => Row((0..src.len()).map(|i| convert(src, i)).collect()),
                Some(indices) => Row(indices.iter().map(|&i| convert(src, i)).collect()),
            })
            .collect();
        self.pos = end;
        Ok(Some(Batch::new(self.output_schema.clone(), rows)))
    }
}

fn json_type(v: &Json) -> DataType {
    match v {
        Json::Bool(_) => DataType::Boolean,
        Json::Number(n) if n.is_i64() => DataType::Int64,
        Json::Number(_) => DataType::Float64,
        _ => DataType::Utf8,
    }
}

fn widen(current: DataType, incoming: DataType) -> DataType {
    use DataType::{Float64, Int64, Utf8};
    match (current, incoming) {
        (a, b) if a == b => a,
        (Int64, Float64) | (Float64, Int64) => Float64,
        _ => Utf8,
    }
}

#[allow(clippy::cast_precision_loss)]
fn to_value(v: &Json, data_type: DataType) -> Value {
    match (v, data_type) {
        (Json::Null, _) => Value::Null,
        (Json::Bool(b), DataType::Boolean) => Value::Boolean(*b),
        (Json::Number(n), DataType::Int64) => n.as_i64().map_or(Value::Null, Value::Int64),
        (Json::Number(n), DataType::Float64) => n.as_f64().map_or(Value::Null, Value::Float64),
        (Json::String(s), _) => Value::utf8(s.as_str()),
        // Mixed-type column, or nested value: fall back to its JSON text.
        (other, _) => Value::utf8(other.to_string()),
    }
}

fn parse_objects(text: &str) -> Result<Vec<Map<String, Json>>, String> {
    let clean = strip_jsonc(text);
    match serde_json::from_str::<Json>(&clean) {
        Ok(Json::Array(items)) => items.into_iter().map(into_object).collect(),
        Ok(other) => Ok(vec![into_object(other)?]),
        Err(whole_err) => {
            // Possibly newline-delimited; report the whole-document error if not.
            let lines: Result<Vec<Json>, _> = clean
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(serde_json::from_str::<Json>)
                .collect();
            match lines {
                Ok(items) if !items.is_empty() => items.into_iter().map(into_object).collect(),
                _ => Err(whole_err.to_string()),
            }
        }
    }
}

fn into_object(v: Json) -> Result<Map<String, Json>, String> {
    match v {
        Json::Object(m) => Ok(m),
        _ => Err("expected records to be JSON objects".to_string()),
    }
}

/// Removes `//` and `/* */` comments and trailing commas, leaving string
/// contents untouched. Newlines inside comments are kept so parse errors
/// still report the right line.
fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                '\\' => out.extend(chars.next()),
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                while let Some(&n) = chars.peek() {
                    if n == '\n' {
                        break;
                    }
                    chars.next();
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = '\0';
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                    }
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            _ => out.push(c),
        }
    }
    strip_trailing_commas(&out)
}

fn strip_trailing_commas(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if c == '\\' {
                i += 1;
                if let Some(&n) = chars.get(i) {
                    out.push(n);
                }
            } else if c == '"' {
                in_string = false;
            }
        } else if c == ',' {
            let next = chars[i + 1..].iter().find(|c| !c.is_whitespace());
            if !matches!(next, Some('}' | ']')) {
                out.push(c);
            }
        } else {
            if c == '"' {
                in_string = true;
            }
            out.push(c);
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(suffix: &str, contents: &str) -> tempfile::NamedTempFile {
        let mut file = tempfile::Builder::new().suffix(suffix).tempfile().unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        file.flush().unwrap();
        file
    }

    fn all_rows(provider: &JsonTableProvider, projection: Option<&[usize]>) -> Vec<Vec<Value>> {
        let mut stream = provider.scan(projection, &[]).unwrap();
        let mut out = Vec::new();
        while let Some(batch) = stream.next_batch().unwrap() {
            out.extend(batch.rows.into_iter().map(|r| r.0));
        }
        out
    }

    #[test]
    fn infers_schema_from_array_of_objects() {
        let file = write(
            ".json",
            r#"[{"id":1,"name":"a","score":9.5},{"id":2,"name":"b","score":null},{"id":3,"extra":true}]"#,
        );
        let p = JsonTableProvider::open(file.path()).unwrap();
        let s = p.schema();
        let names: Vec<_> = s.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["id", "name", "score", "extra"]);
        assert_eq!(s.fields[0].data_type, DataType::Int64);
        assert!(!s.fields[0].nullable);
        assert_eq!(s.fields[2].data_type, DataType::Float64);
        assert!(s.fields[2].nullable);
        assert_eq!(s.fields[3].data_type, DataType::Boolean);
        assert!(s.fields[3].nullable);
    }

    #[test]
    fn jsonc_comments_and_trailing_commas() {
        let file = write(
            ".jsonc",
            "// header\n[\n  /* first */ {\"url\": \"http://x//y\", \"n\": 1,},\n  {\"url\": \"a, ]\", \"n\": 2},\n]\n",
        );
        let p = JsonTableProvider::open(file.path()).unwrap();
        let rows = all_rows(&p, None);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], Value::utf8("http://x//y"));
        assert_eq!(rows[1][0], Value::utf8("a, ]"));
        assert_eq!(rows[1][1], Value::Int64(2));
    }

    #[test]
    fn ndjson_and_single_object() {
        let nd = write(".json", "{\"a\":1}\n{\"a\":2}\n");
        assert_eq!(
            all_rows(&JsonTableProvider::open(nd.path()).unwrap(), None).len(),
            2
        );
        let one = write(".json", "{\"a\":1}");
        assert_eq!(
            all_rows(&JsonTableProvider::open(one.path()).unwrap(), None).len(),
            1
        );
    }

    #[test]
    fn nested_and_mixed_become_text() {
        let file = write(".json", r#"[{"a":1,"b":{"x":1}},{"a":"s","b":[1]}]"#);
        let p = JsonTableProvider::open(file.path()).unwrap();
        assert_eq!(p.schema().fields[0].data_type, DataType::Utf8);
        let rows = all_rows(&p, None);
        assert_eq!(rows[0][0], Value::utf8("1"));
        assert_eq!(rows[0][1], Value::utf8(r#"{"x":1}"#));
    }

    #[test]
    fn projection_pushdown_selects_subset() {
        let file = write(".json", r#"[{"a":1,"b":2,"c":3}]"#);
        let p = JsonTableProvider::open(file.path()).unwrap();
        let rows = all_rows(&p, Some(&[2, 0]));
        assert_eq!(rows[0], vec![Value::Int64(3), Value::Int64(1)]);
    }

    #[test]
    fn rejects_non_object_records() {
        let file = write(".json", "[1,2,3]");
        assert!(JsonTableProvider::open(file.path()).is_err());
    }
}
