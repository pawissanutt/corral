//! Typed, schema-versioned areas for application records.

use std::marker::PhantomData;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::Store;

/// A named area whose documents have one application type.
pub struct RecordArea<T> {
    name: &'static str,
    marker: PhantomData<fn() -> T>,
}

#[derive(Serialize, Deserialize)]
struct Document<T> {
    doc_version: u32,
    value: T,
}

impl<T> RecordArea<T> {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            marker: PhantomData,
        }
    }

    pub fn delete(&self, store: &Store, key: &str) -> Result<()> {
        store.delete_record(self.name, key)
    }
}

impl<T: DeserializeOwned> RecordArea<T> {
    pub fn load(&self, store: &Store, key: &str) -> Result<Option<T>> {
        store
            .record::<Document<T>>(self.name, key)?
            .map(|doc| self.decode(doc))
            .transpose()
    }

    /// Output-sized enumeration of this area, ordered by key.
    pub fn list(&self, store: &Store) -> Result<Vec<(String, T)>> {
        store
            .record_rows(self.name)?
            .into_iter()
            .map(|row| {
                let doc = serde_json::from_value(row.doc)?;
                Ok((row.key, self.decode(doc)?))
            })
            .collect()
    }

    fn decode(&self, doc: Document<T>) -> Result<T> {
        ensure!(
            doc.doc_version == 1,
            "unsupported {} document version {}",
            self.name,
            doc.doc_version
        );
        Ok(doc.value)
    }
}

impl<T: Serialize> RecordArea<T> {
    pub fn save(&self, store: &Store, key: &str, value: &T) -> Result<()> {
        store.save_record(
            self.name,
            key,
            &Document {
                doc_version: 1,
                value,
            },
        )
    }

    /// Commit an intent before a separately committed effect. Unlike `save`, this
    /// survives a later callback refusal, so interrupted effects can be reconciled.
    pub fn save_intent(&self, store: &Store, key: &str, value: &T) -> Result<()> {
        store.commit_record_intent(
            self.name,
            key,
            &Document {
                doc_version: 1,
                value,
            },
        )
    }
}

#[cfg(test)]
#[path = "area_tests.rs"]
mod tests;
