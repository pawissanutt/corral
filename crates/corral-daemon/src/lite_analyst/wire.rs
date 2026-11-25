//! Closed work messages; auth belongs to the inherited channel, not these DTOs.
use anyhow::{Context, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{
    Deserialize, Serialize,
    de::{self, DeserializeOwned, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};
use std::fmt;

pub(super) const MAX_FRAME: usize = 131_072;
pub(super) const MAX_SAFE: u64 = (1 << 53) - 1;
pub(super) const PROFILE: &str = "lite-analyst-v1";
pub(super) const POLICY_REF: &str =
    "b363edfa14cbf9f9f32cf9578d260c99e762e12719df2b0505358f12e94e0104";
pub(super) const PROMPT_SHA256: &str =
    "103ccacd5a80abbd9f83e4d64441cbb6b07e5d7a8c28c793d15bc16aaded8f02";
pub(super) const TOOL_POLICY_SHA256: &str =
    "8879af6aeb66bf98b2533bbe6c99b12c5c5949e37d71622c0267277843f96b4d";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Fault {
    Invalid,
    Conflict,
    Expired,
    StaleGeneration,
    Stopped,
    NotFound,
    BudgetExceeded,
    Busy,
    Unavailable,
    StaleScope,
    Unsupported,
    PolicyUnavailable,
}

pub(super) type Result<T> = std::result::Result<T, Fault>;

impl From<anyhow::Error> for Fault {
    fn from(_: anyhow::Error) -> Self {
        Self::Unavailable
    }
}
impl From<rusqlite::Error> for Fault {
    fn from(_: rusqlite::Error) -> Self {
        Self::Unavailable
    }
}

pub(super) fn valid_id(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

pub(super) fn valid_digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(super) fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn fresh_id() -> anyhow::Result<String> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|_| anyhow::anyhow!("fixture entropy unavailable"))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

// Deserialize without serde_json::Value's last-wins object semantics. Never include peer
// input in diagnostics. Standard serde_json recursion limits apply before typed decoding.
struct Strict(Value);
impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct StrictVisitor;
        impl<'de> Visitor<'de> for StrictVisitor {
            type Value = Strict;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("strict fixture JSON")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Strict, E> {
                if v > MAX_SAFE {
                    return Err(E::custom("integer exceeds safe range"));
                }
                Ok(Strict(v.into()))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Strict, A::Error> {
                let mut values = Vec::new();
                while let Some(Strict(v)) = seq.next_element()? {
                    values.push(v);
                }
                Ok(Strict(values.into()))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Strict, A::Error> {
                let mut out = Map::new();
                while let Some((k, Strict(v))) = map.next_entry()? {
                    if out.insert(k, v).is_some() {
                        return Err(de::Error::custom("duplicate key"));
                    }
                }
                Ok(Strict(out.into()))
            }
        }
        d.deserialize_any(StrictVisitor)
    }
}

pub(super) fn decode<T: DeserializeOwned>(bytes: &[u8]) -> anyhow::Result<T> {
    let Strict(value) =
        serde_json::from_slice(bytes).map_err(|_| anyhow::anyhow!("invalid fixture JSON"))?;
    serde_json::from_value(value).map_err(|_| anyhow::anyhow!("invalid fixture message"))
}

// JCS for the fixture's restricted domain (no floats or signed integers in these DTOs).
pub(super) fn canonical(value: &impl Serialize) -> anyhow::Result<Vec<u8>> {
    fn append(out: &mut Vec<u8>, value: &Value) -> anyhow::Result<()> {
        match value {
            Value::Object(map) => {
                let mut fields: Vec<_> = map.iter().collect();
                fields.sort_unstable_by(|(a, _), (b, _)| a.encode_utf16().cmp(b.encode_utf16()));
                out.push(b'{');
                for (i, (key, value)) in fields.into_iter().enumerate() {
                    if i != 0 {
                        out.push(b',');
                    }
                    serde_json::to_writer(&mut *out, key)?;
                    out.push(b':');
                    append(out, value)?;
                }
                out.push(b'}');
            }
            Value::Array(values) => {
                out.push(b'[');
                for (i, value) in values.iter().enumerate() {
                    if i != 0 {
                        out.push(b',');
                    }
                    append(out, value)?;
                }
                out.push(b']');
            }
            v => {
                ensure!(
                    !v.is_number() || v.as_u64().is_some_and(|n| n <= MAX_SAFE),
                    "invalid canonical integer"
                );
                serde_json::to_writer(out, v)?;
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    append(&mut out, &serde_json::to_value(value)?)?;
    Ok(out)
}

pub(super) fn digest(value: &impl Serialize) -> anyhow::Result<String> {
    Ok(sha256(&canonical(value)?))
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Message {
    Request {
        v: u64,
        id: String,
        op: String,
        params: Value,
    },
    #[serde(rename = "response")]
    Success { v: u64, id: String, result: Value },
    // Deserialize responses manually: the discriminator is shared by success and failure.
    #[serde(skip_deserializing, rename = "response")]
    Failure { v: u64, id: String, error: Fault },
}

impl Message {
    pub(super) fn decode(bytes: &[u8]) -> anyhow::Result<Self> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Failure {
            v: u64,
            #[serde(rename = "type")]
            kind: String,
            id: String,
            error: Fault,
        }
        let value: Value = decode(bytes)?;
        let result = if value.get("error").is_some()
            && value.get("type").and_then(Value::as_str) == Some("response")
        {
            let Failure { v, kind, id, error } =
                serde_json::from_value(value).context("invalid failure envelope")?;
            ensure!(kind == "response", "wrong failure discriminator");
            Self::Failure { v, id, error }
        } else {
            serde_json::from_value(value).map_err(|_| anyhow::anyhow!("invalid work envelope"))?
        };
        let (v, id, body) = match &result {
            Self::Request { v, id, params, .. } => (v, id, Some(params)),
            Self::Success { v, id, result } => (v, id, Some(result)),
            Self::Failure { v, id, .. } => (v, id, None),
        };
        ensure!(
            *v == 1 && valid_id(id) && body.is_none_or(Value::is_object),
            "invalid envelope fields"
        );
        Ok(result)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Hello {
    pub(super) versions: Vec<u64>,
    pub(super) profiles: Vec<String>,
}

pub(super) fn hello(instance: &str, args: Hello) -> Result<Value> {
    if !args.versions.contains(&1) || !args.profiles.iter().any(|p| p == PROFILE) {
        return Err(Fault::Unsupported);
    }
    let manifest = json!({"v":1,"kind":"lite_analyst_policy","role":"lite-analyst",
        "prompt_sha256":PROMPT_SHA256,"tool_policy_sha256":TOOL_POLICY_SHA256,
        "route":{"provider":"scripted","model":"fixture-v1","recipient":"synthetic-local","external":false,"tariff_revision":"demo-1"},
        "retention":{"transcript":"off","report":"until_owner_removal"}});
    if digest(&manifest)? != POLICY_REF {
        return Err(Fault::PolicyUnavailable);
    }
    Ok(
        json!({"v":1,"profile":PROFILE,"corral_instance":instance,"policies":[{"policy_ref":POLICY_REF,"manifest":manifest}]}),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Start {
    request_b64: String,
    pub(super) request_digest: String,
}

impl Start {
    pub(super) fn body(&self) -> Result<StartBody> {
        let raw = STANDARD
            .decode(&self.request_b64)
            .map_err(|_| Fault::Invalid)?;
        if STANDARD.encode(&raw) != self.request_b64 || sha256(&raw) != self.request_digest {
            return Err(Fault::Invalid);
        }
        let body: StartBody = decode(&raw).map_err(|_| Fault::Invalid)?;
        if body.v != 1
            || body.kind != "start_investigation"
            || !valid_id(&body.request_id)
            || !valid_id(&body.grant_id)
            || !valid_digest(&body.grant_digest)
            || body.grant_generation == 0
            || !valid_digest(&body.policy_ref)
            || body.question.len() > 4096
            || canonical(&body)? != raw
        {
            return Err(Fault::Invalid);
        }
        let l = &body.limits;
        if l.calls == 0 || l.response_bytes == 0 || l.total_response_bytes == 0 || l.turns == 0 {
            return Err(Fault::Invalid);
        }
        Ok(body)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StartBody {
    v: u64,
    kind: String,
    pub(super) request_id: String,
    pub(super) grant_id: String,
    grant_digest: String,
    pub(super) grant_generation: u64,
    pub(super) policy_ref: String,
    pub(super) question: String,
    pub(super) deadline_ms: u64,
    pub(super) limits: Limits,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Limits {
    pub(super) calls: u64,
    pub(super) response_bytes: u64,
    pub(super) total_response_bytes: u64,
    pub(super) turns: u64,
    pub(super) run_cost_microusd: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Read {
    pub(super) run_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Cancel {
    pub(super) run_id: String,
    pub(super) generation: u64,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum FenceReason {
    Stopped,
    StaleScope,
    Expired,
}
impl FenceReason {
    pub(super) fn code(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::StaleScope => "stale_scope",
            Self::Expired => "expired",
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Fence {
    pub(super) run_id: String,
    pub(super) generation: u64,
    pub(super) reason: FenceReason,
}

#[derive(Serialize)]
pub(super) struct Binding {
    pub(super) request_id: String,
    pub(super) request_digest: String,
    pub(super) run_id: String,
    pub(super) generation: u64,
    pub(super) grant_id: String,
    pub(super) grant_generation: u64,
    pub(super) policy_ref: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Bound {
    pub(super) bound: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Fenced {
    pub(super) fenced: bool,
}

pub(super) fn params<T: DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|_| Fault::Invalid)
}
