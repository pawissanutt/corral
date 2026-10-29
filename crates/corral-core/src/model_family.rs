//! Model family versus routing vendor. The vendor selects the host provider; the family judges
//! reviewer independence. For `amazon-bedrock` the family is the model publisher named in the
//! Bedrock model ID; for every other vendor it is the vendor itself.
use std::fmt;

use crate::{ResolvedProfile, Run};

const BEDROCK: &str = "amazon-bedrock";
const BEDROCK_GEOS: [&str; 8] = [
    "us.", "eu.", "apac.", "global.", "au.", "jp.", "ca.", "us-gov.",
];

/// The model family reviewer independence compares. Equals the vendor for non-Bedrock vendors.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModelFamily(String);

impl ModelFamily {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for ModelFamily {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl fmt::Display for ModelFamily {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn bedrock_parts(model: &str) -> Option<(&str, &str)> {
    let id = model.strip_prefix("amazon-bedrock/").unwrap_or(model);
    let id = BEDROCK_GEOS
        .iter()
        .find_map(|geo| id.strip_prefix(geo))
        .unwrap_or(id);
    let (publisher, name) = id.split_once('.')?;
    let valid = !publisher.is_empty()
        && publisher
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    (valid && !name.is_empty()).then_some((publisher, name))
}

/// None for a blank vendor, or a Bedrock model whose publisher cannot be read (an ARN, a blank ID).
pub fn model_family(vendor: Option<&str>, model: Option<&str>) -> Option<ModelFamily> {
    let vendor = vendor.filter(|v| !v.trim().is_empty())?;
    if vendor != BEDROCK {
        return Some(vendor.into());
    }
    bedrock_parts(model?).map(|(publisher, _)| publisher.into())
}

/// The model identity the plan-review same-model check compares, lowercased. Bedrock drops the
/// prefix, geo, publisher and a trailing `-v<n>(:<n>)?`; other vendors keep the last segment.
pub fn model_key(vendor: Option<&str>, model: &str) -> String {
    if vendor == Some(BEDROCK)
        && let Some((_, name)) = bedrock_parts(model)
    {
        return strip_bedrock_version(name).to_ascii_lowercase();
    }
    model
        .rsplit(['/', ':'])
        .next()
        .unwrap_or(model)
        .to_ascii_lowercase()
}

fn strip_bedrock_version(name: &str) -> &str {
    let base = match name.rsplit_once(':') {
        Some((base, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => base,
        _ => name,
    };
    match base.rsplit_once("-v") {
        Some((stem, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => stem,
        _ => base,
    }
}

impl ResolvedProfile {
    pub fn family(&self) -> Option<ModelFamily> {
        model_family(self.vendor.as_deref(), self.model.as_deref())
    }
}

impl Run {
    /// The model the run actually executed: its admitted snapshot, else the recorded model.
    pub fn executed_model(&self) -> Option<&str> {
        self.execution
            .as_ref()
            .and_then(|e| e.resolved.model.as_deref())
            .or(self.model.as_deref())
    }

    pub fn family(&self, vendor: Option<&str>) -> Option<ModelFamily> {
        model_family(vendor, self.executed_model())
    }
}
