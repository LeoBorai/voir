use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataType {
    Null,
    Boolean,
    Int64,
    Float64,
    Utf8,
    Date,
    Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub data_type: DataType,
    pub nullable: bool,
}

impl Field {
    pub fn new(name: impl Into<String>, data_type: DataType, nullable: bool) -> Self {
        Self {
            name: name.into(),
            data_type,
            nullable,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Schema {
    pub fields: Vec<Field>,
}

impl Schema {
    pub fn new(fields: Vec<Field>) -> Self {
        Self { fields }
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.fields.iter().position(|f| f.name.eq_ignore_ascii_case(name))
    }
}

pub type SchemaRef = Arc<Schema>;
