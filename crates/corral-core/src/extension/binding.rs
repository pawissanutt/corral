//! Context binding v1 and its digest (plan T-381 section 6.1).

use super::error::{ErrorCode, ExtError};
use crate::CandidateIdentity;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const BINDING_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub v: u32,
    pub scope: BindingScope,
    pub task: BindingTask,
    pub code: CodeBinding,
    pub workflow: WorkflowBinding,
    pub provider: ProviderBinding,
    pub policy: PolicyBinding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingScope {
    pub workspace_id: String,
    pub project: String,
    pub repo: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingTask {
    pub id: String,
    pub spec_rev: String,
}

/// The code a context answers for. The pilot admits only `dispatch_base`; `candidate`
/// carries the comparator fields of a T-251 `CandidateIdentity`, never its environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CodeBinding {
    DispatchBase {
        base: String,
        base_sha: String,
        tree: String,
    },
    Candidate {
        version: u32,
        head: String,
        tree: String,
        spec_digest: String,
    },
}

impl From<&CandidateIdentity> for CodeBinding {
    fn from(identity: &CandidateIdentity) -> Self {
        Self::Candidate {
            version: identity.version,
            head: identity.head.clone(),
            tree: identity.tree.clone(),
            spec_digest: identity.spec_digest.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    CurrentAtCapture,
    Exact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowBinding {
    pub mode: Freshness,
    pub lineage: String,
    /// Encoded public-read cursor: the lower bound of the provider's workflow frontier.
    pub w_min: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderBinding {
    pub ext: String,
    pub registration_rev: u64,
    pub manifest: String,
    pub executable: String,
    pub capability: String,
    pub config: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyBinding {
    pub freshness: Freshness,
    pub required: bool,
}

impl Binding {
    /// `sha256:<hex>` over [`canonical_json`] of the binding.
    pub fn digest(&self) -> Result<String, ExtError> {
        let value = serde_json::to_value(self)
            .map_err(|e| ExtError::new(ErrorCode::Internal, format!("encoding binding: {e}")))?;
        Ok(sha256_tag(canonical_json(&value).as_bytes()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextBudget {
    pub max_items: u32,
    pub max_bytes: u64,
    pub max_evidence_items: u32,
    pub max_excerpt_bytes: u32,
}

/// `provider.invoke` params for `task.context@1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskContextParams {
    pub binding: Binding,
    pub budget: ContextBudget,
    pub deadline_unix_ms: u64,
}

/// One host-side context request: the wire params plus the request (attempt) ID and the
/// brief ceiling it renders under. Only `params.binding` contributes to the digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextRequest {
    pub request_id: String,
    pub brief_max_bytes: u64,
    pub params: TaskContextParams,
}

impl ContextRequest {
    pub fn binding_digest(&self) -> Result<String, ExtError> {
        self.params.binding.digest()
    }
}

/// `sha256:<lowercase hex>`.
pub fn sha256_tag(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(7 + 64);
    out.push_str("sha256:");
    for byte in digest {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

const HEX: &[u8; 16] = b"0123456789abcdef";

/// Canonical JSON: object keys sorted by their UTF-8 bytes, no whitespace, strings escaped
/// only where JSON requires (`"`, `\`, U+0000-U+001F) with the short escapes where they exist,
/// numbers in serde_json's shortest form. docs/extensions-protocol.md specifies it.
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            out.push('{');
            for (i, (key, item)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_canonical(item, out);
            }
            out.push('}');
        }
    }
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let code = u32::from(c);
                out.push_str("\\u00");
                out.push(char::from(HEX[(code >> 4) as usize]));
                out.push(char::from(HEX[(code & 0x0f) as usize]));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
