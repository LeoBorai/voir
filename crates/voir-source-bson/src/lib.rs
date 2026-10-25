use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use bson::{Bson, Document};
use voir_catalog::{BatchStream, FilterExpr, TableProvider};
use voir_types::{Batch, DataType, Field, Row, Schema, SchemaRef, Value, VoirError};

/// Reads `.bson` files: a sequence of concatenated BSON documents, as written
/// by `mongodump`. Each document is one row; columns are the union of keys in
/// first-seen order. `Int32`/`Int64` map to `Int64`, `Double` to `Float64`,
/// `DateTime` to `Timestamp` (milliseconds since the Unix epoch). `ObjectId`
/// surfaces as hex text; other types (nested documents, arrays, ...) as
/// relaxed extended JSON text in a `Utf8` column.
///
/// The file is parsed once at `open` and held in memory.
pub struct BsonTableProvider {
    schema: SchemaRef,
    rows: Arc<Vec<Vec<Bson>>>,
}

impl BsonTableProvider {
    /// # Errors
    ///
    /// Returns an error if the file cannot be read, is not valid BSON, or
    /// contains no documents.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, VoirError> {
        let path = path.as_ref();
        let source =
            |e: &dyn std::fmt::Display| VoirError::Source(format!("{}: {e}", path.display()));
        let mut reader = BufReader::new(File::open(path)?);
        let mut docs: Vec<Document> = Vec::new();
        loop {
            match Document::from_reader(&mut reader) {
                Ok(doc) => docs.push(doc),
                // A clean EOF at a document boundary ends the stream.
                Err(bson::error::Error { .. }) if at_eof(&mut reader) => break,
                Err(e) => return Err(source(&e)),
            }
        }
        if docs.is_empty() {
            return Err(VoirError::Source(format!(
                "{}: no records, cannot infer schema",
                path.display()
            )));
        }

        let mut names: Vec<String> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        for doc in &docs {
            for key in doc.keys() {
                if !index.contains_key(key) {
                    index.insert(key.clone(), names.len());
                    names.push(key.clone());
                }
            }
        }

        let rows: Vec<Vec<Bson>> = docs
            .into_iter()
            .map(|mut doc| {
                names
                    .iter()
                    .map(|n| doc.remove(n).unwrap_or(Bson::Null))
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
                        Bson::Null | Bson::Undefined => nullable = true,
                        v => {
                            let t = bson_type(v);
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

/// True when no bytes remain, i.e. the previous read failed on a missing
/// document rather than a malformed one.
fn at_eof(reader: &mut BufReader<File>) -> bool {
    use std::io::BufRead;
    reader.fill_buf().is_ok_and(<[u8]>::is_empty)
}

impl TableProvider for BsonTableProvider {
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
        Ok(Box::new(BsonBatchStream {
            rows: self.rows.clone(),
            source_schema: self.schema.clone(),
            output_schema,
            projection: projection.map(<[usize]>::to_vec),
            pos: 0,
        }))
    }
}

struct BsonBatchStream {
    rows: Arc<Vec<Vec<Bson>>>,
    source_schema: SchemaRef,
    output_schema: SchemaRef,
    projection: Option<Vec<usize>>,
    pos: usize,
}

impl BatchStream for BsonBatchStream {
    fn next_batch(&mut self) -> Result<Option<Batch>, VoirError> {
        let end = (self.pos + Batch::MAX_ROWS).min(self.rows.len());
        if self.pos >= end {
            return Ok(None);
        }
        let convert =
            |src: &[Bson], i: usize| to_value(&src[i], self.source_schema.fields[i].data_type);
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

fn bson_type(v: &Bson) -> DataType {
    match v {
        Bson::Boolean(_) => DataType::Boolean,
        Bson::Int32(_) | Bson::Int64(_) => DataType::Int64,
        Bson::Double(_) => DataType::Float64,
        Bson::DateTime(_) => DataType::Timestamp,
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
fn to_value(v: &Bson, data_type: DataType) -> Value {
    match (v, data_type) {
        (Bson::Null | Bson::Undefined, _) => Value::Null,
        (Bson::Boolean(b), DataType::Boolean) => Value::Boolean(*b),
        (Bson::Int32(n), DataType::Int64) => Value::Int64(i64::from(*n)),
        (Bson::Int64(n), DataType::Int64) => Value::Int64(*n),
        (Bson::Int32(n), DataType::Float64) => Value::Float64(f64::from(*n)),
        (Bson::Int64(n), DataType::Float64) => Value::Float64(*n as f64),
        (Bson::Double(n), DataType::Float64) => Value::Float64(*n),
        (Bson::DateTime(d), DataType::Timestamp) => Value::Timestamp(d.timestamp_millis()),
        (Bson::String(s), _) => Value::utf8(s.as_str()),
        (Bson::ObjectId(id), _) => Value::utf8(id.to_hex()),
        // Mixed-type column, or composite value: fall back to its JSON text.
        (other, _) => Value::utf8(other.clone().into_relaxed_extjson().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bson::{doc, oid::ObjectId};
    use std::io::Write;

    fn write(docs: &[Document]) -> tempfile::NamedTempFile {
        let mut file = tempfile::Builder::new().suffix(".bson").tempfile().unwrap();
        for d in docs {
            file.write_all(&bson::serialize_to_vec(d).unwrap()).unwrap();
        }
        file.flush().unwrap();
        file
    }

    fn all_rows(provider: &BsonTableProvider, projection: Option<&[usize]>) -> Vec<Vec<Value>> {
        let mut stream = provider.scan(projection, &[]).unwrap();
        let mut out = Vec::new();
        while let Some(batch) = stream.next_batch().unwrap() {
            out.extend(batch.rows.into_iter().map(|r| r.0));
        }
        out
    }

    #[test]
    fn infers_schema_from_documents() {
        let file = write(&[
            doc! {"id": 1_i32, "name": "a", "score": 9.5},
            doc! {"id": 2_i64, "name": "b", "score": Bson::Null},
            doc! {"id": 3_i32, "extra": true},
        ]);
        let p = BsonTableProvider::open(file.path()).unwrap();
        let s = p.schema();
        let names: Vec<_> = s.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["id", "name", "score", "extra"]);
        assert_eq!(s.fields[0].data_type, DataType::Int64);
        assert!(!s.fields[0].nullable);
        assert_eq!(s.fields[2].data_type, DataType::Float64);
        assert!(s.fields[2].nullable);
        assert_eq!(s.fields[3].data_type, DataType::Boolean);
        assert!(s.fields[3].nullable);
        assert_eq!(all_rows(&p, None).len(), 3);
    }

    #[test]
    fn special_types() {
        let oid = ObjectId::new();
        let file = write(&[doc! {
            "_id": oid,
            "at": bson::DateTime::from_millis(1_500),
            "nested": {"x": 1_i32},
        }]);
        let p = BsonTableProvider::open(file.path()).unwrap();
        assert_eq!(p.schema().fields[1].data_type, DataType::Timestamp);
        let rows = all_rows(&p, None);
        assert_eq!(rows[0][0], Value::utf8(oid.to_hex()));
        assert_eq!(rows[0][1], Value::Timestamp(1_500));
        assert_eq!(rows[0][2], Value::utf8(r#"{"x":1}"#));
    }

    #[test]
    fn projection_pushdown_selects_subset() {
        let file = write(&[doc! {"a": 1_i32, "b": 2_i32, "c": 3_i32}]);
        let p = BsonTableProvider::open(file.path()).unwrap();
        let rows = all_rows(&p, Some(&[2, 0]));
        assert_eq!(rows[0], vec![Value::Int64(3), Value::Int64(1)]);
    }

    #[test]
    fn rejects_empty_and_truncated() {
        assert!(BsonTableProvider::open(write(&[]).path()).is_err());
        let file = write(&[doc! {"a": 1_i32}]);
        let mut bytes = std::fs::read(file.path()).unwrap();
        bytes.truncate(bytes.len() - 2);
        std::fs::write(file.path(), bytes).unwrap();
        assert!(BsonTableProvider::open(file.path()).is_err());
    }
}
