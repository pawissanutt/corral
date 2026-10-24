//! Typed evidence references (plan T-253 section 7).
//!
//! Every evidence reference is a [`BlobRef`] that serializes as `blob:sha256:<64 hex>`.
//! [`scan`] finds every reference held at any depth of a serialized document, so a backend can
//! keep its reference set exact without understanding the document's shape. Only a whole JSON
//! string in the canonical form is a reference: a bare 64-hex digest, an uppercase digest or a
//! reference embedded in longer text is not.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use anyhow::{Result, bail};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// Serialized prefix of every [`BlobRef`].
pub const BLOB_REF_PREFIX: &str = "blob:sha256:";

/// A SHA-256 content hash as 64 lowercase hex characters.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlobHash(String);

impl BlobHash {
    /// Parses a digest, refusing anything but exactly 64 lowercase hex characters.
    pub fn parse(hex: &str) -> Result<Self> {
        if !is_digest(hex) {
            bail!("not a sha256 digest (64 lowercase hex): {hex:?}");
        }
        Ok(Self(hex.to_owned()))
    }

    pub fn of(bytes: &[u8]) -> Self {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(bytes);
        Self(digest.iter().map(|b| format!("{b:02x}")).collect())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn to_ref(&self) -> BlobRef {
        BlobRef(self.clone())
    }
}

impl fmt::Display for BlobHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for BlobHash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for BlobHash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// A typed reference to a stored blob; serializes as `blob:sha256:<64 hex>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlobRef(BlobHash);

impl BlobRef {
    pub fn hash(&self) -> &BlobHash {
        &self.0
    }
}

impl fmt::Display for BlobRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{BLOB_REF_PREFIX}{}", self.0)
    }
}

impl FromStr for BlobRef {
    type Err = anyhow::Error;

    fn from_str(text: &str) -> Result<Self> {
        match parse_ref(text) {
            Some(hash) => Ok(Self(hash)),
            None => bail!("not a blob reference (blob:sha256:<64 lowercase hex>): {text:?}"),
        }
    }
}

impl Serialize for BlobRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for BlobRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// Every blob hash referenced anywhere in `doc`, in hash order.
pub fn scan(doc: &Value) -> BTreeSet<BlobHash> {
    let mut found = BTreeSet::new();
    collect(doc, &mut found);
    found
}

/// [`scan`] over any serializable document.
pub fn scan_doc<T: Serialize + ?Sized>(doc: &T) -> Result<BTreeSet<BlobHash>> {
    Ok(scan(&serde_json::to_value(doc)?))
}

fn collect(value: &Value, found: &mut BTreeSet<BlobHash>) {
    match value {
        Value::String(text) => {
            if let Some(hash) = parse_ref(text) {
                found.insert(hash);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| collect(item, found)),
        Value::Object(fields) => fields.values().for_each(|field| collect(field, found)),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn parse_ref(text: &str) -> Option<BlobHash> {
    let hex = text.strip_prefix(BLOB_REF_PREFIX)?;
    is_digest(hex).then(|| BlobHash(hex.to_owned()))
}

fn is_digest(hex: &str) -> bool {
    hex.len() == 64
        && hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
