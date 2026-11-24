use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataKind {
    Record,
    Enum,
    Table,
    Message,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataSource {
    JsonSchema,
    Sql,
    RustSerde,
    Extension,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DataTypeId {
    pub path: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataType {
    pub path: String,
    pub name: String,
    pub line: u64,
    pub kind: DataKind,
    pub source: DataSource,
    pub fields: Vec<DataField>,
}

impl DataType {
    pub fn id(&self) -> DataTypeId {
        DataTypeId {
            path: self.path.clone(),
            name: self.name.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataField {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub line: u64,
    /// Source-level optionality, not the result of running a serializer or database.
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataEdgeKind {
    Has,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DataEdge {
    pub kind: DataEdgeKind,
    pub from: DataTypeId,
    pub to: DataTypeId,
    pub field: String,
    pub line: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataChange {
    pub path: String,
    pub name: String,
    /// Declaration line at the base on removals, at the head otherwise.
    pub line: u64,
    pub kind: DataKind,
    pub source: DataSource,
    /// A declared schema or serialized source type; not a compatibility verdict.
    pub contract: bool,
    #[serde(flatten)]
    pub change: DataChangeKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum DataChangeKind {
    TypeAdded,
    TypeRemoved,
    FieldAdded { field: DataField },
    FieldRemoved { field: DataField },
    FieldRetyped { before: DataField, after: DataField },
    FieldOptionalityChanged { before: DataField, after: DataField },
}
