//! Read-only compatibility for the former harness and inbox adapter identifiers.
use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[cfg(test)]
#[path = "harness_compat_tests.rs"]
mod tests;

pub fn canonical(value: &str) -> &str {
    match value {
        "pi-native" => crate::CORRAL_PI_HARNESS,
        "pi_native" => "corral_pi",
        _ => value,
    }
}

/// Read old installations in place so old and new binaries can coexist during rollout.
/// New installations use `target`. Discovery must never move, merge or create directories.
pub fn directory(target: &std::path::Path) -> std::path::PathBuf {
    let legacy = target.with_file_name("pi-native");
    if !target.exists() && legacy.is_dir() {
        legacy
    } else {
        target.to_owned()
    }
}

pub mod string {
    use super::*;
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
        String::deserialize(d).map(|value| canonical(&value).to_owned())
    }
    pub fn serialize<S: Serializer>(value: &str, s: S) -> Result<S::Ok, S::Error> {
        canonical(value).serialize(s)
    }
}

pub mod optional {
    use super::*;
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
        Option::<String>::deserialize(d)
            .map(|value| value.map(|value| canonical(&value).to_owned()))
    }
    pub fn serialize<S: Serializer>(value: &Option<String>, s: S) -> Result<S::Ok, S::Error> {
        value.as_deref().map(canonical).serialize(s)
    }
}

/// Harness table keys are identities too; profile *names* must never pass through here.
pub mod keys {
    use super::*;
    pub fn deserialize<'de, D, T>(d: D) -> Result<BTreeMap<String, T>, D::Error>
    where
        D: Deserializer<'de>,
        T: Deserialize<'de>,
    {
        let values = BTreeMap::<String, T>::deserialize(d)?;
        let mut result = BTreeMap::new();
        for (key, value) in values {
            let key = canonical(&key).to_owned();
            if result.insert(key.clone(), value).is_some() {
                return Err(serde::de::Error::custom(format!("duplicate harness {key}")));
            }
        }
        Ok(result)
    }
    pub fn serialize<S: Serializer, T: Serialize>(
        values: &BTreeMap<String, T>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        values
            .iter()
            .map(|(key, value)| (canonical(key), value))
            .collect::<BTreeMap<_, _>>()
            .serialize(s)
    }
}

/// Events retain arbitrary historical payloads; normalize only identifier fields.
pub mod event_detail {
    use super::*;
    fn normalize(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(fields) => {
                for (key, value) in fields {
                    if matches!(key.as_str(), "harness" | "adapter" | "via")
                        && let Some(text) = value.as_str()
                    {
                        *value = canonical(text).into();
                    } else {
                        normalize(value);
                    }
                }
            }
            serde_json::Value::Array(values) => values.iter_mut().for_each(normalize),
            _ => {}
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<serde_json::Value, D::Error> {
        let mut value = serde_json::Value::deserialize(d)?;
        normalize(&mut value);
        Ok(value)
    }
    pub fn serialize<S: Serializer>(value: &serde_json::Value, s: S) -> Result<S::Ok, S::Error> {
        let mut value = value.clone();
        normalize(&mut value);
        value.serialize(s)
    }
}
