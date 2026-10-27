//! Fixed-width derivation cursors keep measured document bytes independent of history
//! sequence magnitudes. Numeric cursors from earlier cache versions remain readable so
//! preparation can recognize their version and rebuild them.

use serde::{Deserialize, Deserializer, Serializer, de::Error};

pub(crate) mod number {
    use super::*;

    pub fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{value:016x}"))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Stored {
            Fixed(String),
            Legacy(u64),
        }
        match Stored::deserialize(deserializer)? {
            Stored::Fixed(value) => u64::from_str_radix(&value, 16).map_err(D::Error::custom),
            Stored::Legacy(value) => Ok(value),
        }
    }
}

pub(crate) mod sequence {
    use super::*;
    use corral_core::backend::ChangeSeq;

    pub fn serialize<S: Serializer>(value: &ChangeSeq, serializer: S) -> Result<S::Ok, S::Error> {
        number::serialize(&value.0, serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<ChangeSeq, D::Error> {
        number::deserialize(deserializer).map(ChangeSeq)
    }
}
