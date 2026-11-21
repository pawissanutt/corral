//! Direct submission of an engine operation.
use corral_types::Op;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const OP_SUBMIT: &str = "op.submit";

/// For `usage_add`, absent token counters are unknown unless listed in
/// `explicit_zero_tokens`. A sparse payload's supplied zero counters are evidence.
/// Legacy clients serialized all four counters even when unknown, so an all-four
/// payload must list its reported zeros in `explicit_zero_tokens` to disambiguate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OpSubmit {
    #[schemars(with = "crate::schema_types::Op")]
    #[serde(with = "submission_op")]
    pub op: Op,
    #[serde(default)]
    pub force: bool,
}

// Keep field-presence inference at submission ingress, not on the shared Usage
// serde path (which may see defaulted counters from historical data).
mod submission_op {
    use corral_types::{Op, UsageToken};
    use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser};
    use serde_json::Value;

    const TOKEN_FIELDS: [(&str, UsageToken); 4] = [
        ("input", UsageToken::Input),
        ("output", UsageToken::Output),
        ("cache_read", UsageToken::CacheRead),
        ("cache_write", UsageToken::CacheWrite),
    ];
    const USAGE_PATH: &str = "/usage_add/usage";

    pub(super) fn serialize<S: Serializer>(op: &Op, serializer: S) -> Result<S::Ok, S::Error> {
        let Op::UsageAdd { usage } = op else {
            return op.serialize(serializer);
        };
        let mut wire = serde_json::to_value(op).map_err(ser::Error::custom)?;
        let fields = wire
            .pointer_mut(USAGE_PATH)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| ser::Error::custom("expected usage_add.usage object"))?;
        for (field, token) in TOKEN_FIELDS {
            if usage.token_count(token).is_none() {
                fields.remove(field);
            }
        }
        wire.serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Op, D::Error> {
        let wire = Value::deserialize(deserializer)?;
        let mut op = Op::deserialize(&wire).map_err(de::Error::custom)?;
        if let Op::UsageAdd { usage } = &mut op
            && let Some(fields) = wire.pointer(USAGE_PATH).and_then(Value::as_object)
            && !TOKEN_FIELDS
                .iter()
                .all(|(field, _)| fields.contains_key(*field))
        {
            for (field, token) in TOKEN_FIELDS {
                if fields.get(field).and_then(Value::as_u64) == Some(0) {
                    usage.explicit_zero_tokens.insert(token);
                }
            }
        }
        Ok(op)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OpReply {
    Done { seq: u64, task: Option<String> },
    Running { seq: u64 },
}

#[cfg(test)]
#[path = "op_tests.rs"]
mod tests;
