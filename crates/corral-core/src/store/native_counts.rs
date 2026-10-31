//! Native family footer compatibility: the original seven positions keep their meaning.
use serde::{Deserialize, Deserializer, Serializer, de::Error};
use serde_json::Value;

pub fn serialize<S: Serializer>(counts: &[u64; 10], s: S) -> Result<S::Ok, S::Error> {
    crate::container_facts::fixed::array::serialize(counts, s)
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u64; 10], D::Error> {
    let mut values = Vec::<Value>::deserialize(d)?;
    match values.len() {
        7 => values.resize(10, Value::from(0)),
        10 => {}
        _ => {
            return Err(D::Error::custom(
                "native counts need seven legacy or ten current positions",
            ));
        }
    }
    crate::container_facts::fixed::array::deserialize(Value::Array(values))
        .map_err(D::Error::custom)
}
