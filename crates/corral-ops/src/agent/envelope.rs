//! Envelope v1 of `corral agent call`: parsing with bounds, the canonical payload digest and
//! the request-ID derivations. Nothing here reads the store.
use corral_core::Store;
use corral_core::agent::{AgentRole, RequestIdentity, paths::sha256_hex};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::call::{AgentOp, RejectCode, Rejection};

pub const ENVELOPE_VERSION: u32 = 1;
pub const ENVELOPE_MAX_BYTES: usize = 64 * 1024;
pub const PAYLOAD_MAX_BYTES: usize = 32 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub version: u32,
    pub op: String,
    pub request_id: String,
    pub payload_sha256: String,
    pub identity: RequestIdentity,
    pub auth: Auth,
    #[serde(default)]
    pub payload: Value,
}

/// Bearer secrets of one generation. Never logged, stored or echoed; only digests persist.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Auth {
    pub host_cap: String,
    /// hello only: the one-shot nonce from the generation's bootstrap.
    #[serde(default)]
    pub bind_nonce: Option<String>,
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Auth")
            .field("host_cap", &"<redacted>")
            .field(
                "bind_nonce",
                &self.bind_nonce.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// What a rejection can still report about an envelope that failed to parse.
#[derive(Debug, Default)]
pub(crate) struct Partial {
    pub request_id: Option<String>,
    pub identity: Option<RequestIdentity>,
}

/// Object keys sorted, no insignificant whitespace, serde_json scalar formatting.
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write_canonical(&map[key], out);
            }
            out.push('}');
        }
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
        scalar => out.push_str(&scalar.to_string()),
    }
}

pub fn payload_sha256(payload: &Value) -> String {
    sha256_hex(canonical_json(payload).as_bytes())
}

/// `rq-` plus 32 hex digits of SHA-256(session_id, NUL, tool_call_id): stable across host
/// retries, generations and resume.
pub fn tool_request_id(session_id: &str, tool_call_id: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(session_id.as_bytes());
    hash.update([0u8]);
    hash.update(tool_call_id.as_bytes());
    let hex: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    format!("rq-{}", &hex[..32])
}

pub fn event_request_id(generation: u32, seq: u64) -> String {
    format!("ev-{generation}-{seq}")
}

/// The `(generation, seq)` of a lifecycle-event request ID.
pub(crate) fn event_id_parts(request_id: &str) -> Option<(u32, u64)> {
    let (generation, seq) = request_id.strip_prefix("ev-")?.split_once('-')?;
    let canonical = |s: &str| {
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'))
    };
    (canonical(generation) && canonical(seq)).then_some(())?;
    Some((generation.parse().ok()?, seq.parse().ok()?))
}

fn bad(reason: impl Into<String>) -> Rejection {
    Rejection::new(RejectCode::BadRequest, reason)
}

fn token(field: &str, value: &str, max: usize, extra: &str) -> Result<(), Rejection> {
    let ok = !value.is_empty()
        && value.len() <= max
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || extra.contains(c));
    ok.then_some(()).ok_or_else(|| {
        bad(format!(
            "{field} must be 1-{max} characters of [A-Za-z0-9{extra}]"
        ))
    })
}

pub(crate) fn secret(field: &str, value: &str) -> Result<(), Rejection> {
    let ok = (32..=256).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    ok.then_some(()).ok_or_else(|| {
        bad(format!(
            "{field} must be 32-256 characters of [A-Za-z0-9_-]"
        ))
    })
}

pub(crate) fn hex64(field: &str, value: &str) -> Result<(), Rejection> {
    let ok = value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    ok.then_some(())
        .ok_or_else(|| bad(format!("{field} must be 64 lowercase hex digits")))
}

/// A request ID of any generation, as a lookup by a later generation names it.
pub(crate) fn request_id_shape(value: &str) -> Result<(), Rejection> {
    match event_id_parts(value) {
        Some((generation, _)) => request_id(value, generation),
        None => request_id(value, 0),
    }
}

pub(crate) fn request_id(value: &str, generation: u32) -> Result<(), Rejection> {
    if let Some(hex) = value.strip_prefix("rq-") {
        let ok = hex.len() == 32 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        return ok
            .then_some(())
            .ok_or_else(|| bad("request_id rq- needs 32 lowercase hex digits"));
    }
    match event_id_parts(value) {
        Some((g, _)) if g == generation => Ok(()),
        Some(_) => Err(bad("an ev- request_id must name the envelope's generation")),
        None => Err(bad(
            "request_id must be rq-<32 hex> or ev-<generation>-<seq>",
        )),
    }
}

fn identity(identity: &RequestIdentity) -> Result<(), Rejection> {
    token("identity.store", &identity.store, 64, "-")?;
    // An assistant is workspace-scoped: it names its session and no project.
    match (identity.role, identity.assistant.as_deref()) {
        (AgentRole::Assistant, Some(session)) => {
            corral_core::assistant::validate_id(session)
                .map_err(|_| bad("identity.assistant must be A-<n>"))?;
            if !identity.project.is_empty() {
                return Err(bad("an assistant identity names no project"));
            }
        }
        (AgentRole::Assistant, None) => return Err(bad("identity.assistant is required")),
        (_, Some(_)) => return Err(bad("identity.assistant is for the assistant role only")),
        // Workspace PMs carry no project; older project-bound sessions remain valid.
        (AgentRole::Pm, None) if identity.project.is_empty() => {}
        (_, None) => token("identity.project", &identity.project, 128, "._-")?,
    }
    token("identity.session_id", &identity.session_id, 128, "._:-")?;
    if identity.generation == 0 {
        return Err(bad("identity.generation starts at 1"));
    }
    let (task, run, review, callback) = (
        identity.task.as_deref(),
        identity.run.as_deref(),
        identity.review.as_deref(),
        identity.callback.as_deref(),
    );
    let shaped = match identity.role {
        AgentRole::Worker | AgentRole::Planner => {
            task.is_some() && run.is_some() && review.is_none() && callback.is_none()
        }
        AgentRole::Reviewer => {
            task.is_some() && run.is_some() && review.is_some() && callback.is_some()
        }
        AgentRole::PlanReviewer => {
            task.is_some() && run.is_some() && review.is_some() && callback.is_none()
        }
        AgentRole::Pm => task.is_none() && run.is_none() && review.is_none() && callback.is_none(),
        AgentRole::Assistant => {
            task.is_none() && run.is_none() && review.is_none() && callback.is_none()
        }
    };
    if !shaped {
        return Err(bad(format!(
            "identity for role {} must name {}",
            identity.role.as_str(),
            match identity.role {
                AgentRole::Worker | AgentRole::Planner => "task and run only",
                AgentRole::Reviewer => "task, run, review and callback",
                AgentRole::PlanReviewer => "task, plan in run, and review only",
                AgentRole::Pm => "no task, run, review or callback",
                AgentRole::Assistant => "its session and no task, run, review or callback",
            }
        )));
    }
    if let Some(task) = task
        && Store::normalize_id(task).ok().as_deref() != Some(task)
    {
        return Err(bad(
            "identity.task must be a canonical task ID such as T-001",
        ));
    }
    let prefix = match identity.role {
        AgentRole::Planner => 'D',
        AgentRole::PlanReviewer => 'P',
        AgentRole::Worker | AgentRole::Reviewer | AgentRole::Pm | AgentRole::Assistant => 'R',
    };
    if let Some(run) = run
        && !run
            .strip_prefix(prefix)
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(bad(format!("identity.run must be {prefix}<n>")));
    }
    if let Some(review) = review {
        token("identity.review", review, 64, "._-")?;
    }
    if let Some(callback) = callback {
        token("identity.callback", callback, 256, "._:-")?;
    }
    Ok(())
}

pub(crate) fn error_category(error: &serde_json::Error) -> &'static str {
    match error.classify() {
        serde_json::error::Category::Io => "io",
        serde_json::error::Category::Syntax => "syntax",
        serde_json::error::Category::Data => "data",
        serde_json::error::Category::Eof => "eof",
    }
}

/// Input-independent: serde's message can name unknown fields and variants from the input,
/// which may hold secret bytes, so only the category and position are reported.
fn parse_error(error: &serde_json::Error) -> Rejection {
    bad(format!(
        "malformed envelope ({} error at line {} column {}); expected fields: version, op, \
         request_id, payload_sha256, identity, auth {{host_cap, bind_nonce?}}, payload",
        error_category(error),
        error.line(),
        error.column()
    ))
}

/// Bounds first: size, shape, version, op, request ID, identity, secrets and payload digest.
pub(crate) fn parse(
    op_flag: &str,
    input: &[u8],
) -> Result<(Envelope, AgentOp), Box<(Rejection, Partial)>> {
    if input.len() > ENVELOPE_MAX_BYTES {
        return Err(Box::new((
            bad(format!("envelope exceeds {ENVELOPE_MAX_BYTES} bytes")),
            Partial::default(),
        )));
    }
    let envelope: Envelope = serde_json::from_slice(input)
        .map_err(|e| Box::new((parse_error(&e), Partial::default())))?;
    let partial = Partial {
        request_id: request_id_shape(&envelope.request_id)
            .is_ok()
            .then(|| envelope.request_id.clone()),
        identity: identity(&envelope.identity)
            .is_ok()
            .then(|| envelope.identity.clone()),
    };
    match check(op_flag, &envelope) {
        Ok(op) => Ok((envelope, op)),
        Err(rejection) => Err(Box::new((rejection, partial))),
    }
}

fn check(op_flag: &str, envelope: &Envelope) -> Result<AgentOp, Rejection> {
    if envelope.version != ENVELOPE_VERSION {
        return Err(bad(format!("envelope version must be {ENVELOPE_VERSION}")));
    }
    let op = AgentOp::parse(&envelope.op).ok_or_else(|| bad("unknown op"))?;
    if envelope.op != op_flag {
        return Err(bad("envelope op differs from --op"));
    }
    identity(&envelope.identity)?;
    request_id(&envelope.request_id, envelope.identity.generation)?;
    secret("auth.host_cap", &envelope.auth.host_cap)?;
    if let Some(nonce) = &envelope.auth.bind_nonce {
        secret("auth.bind_nonce", nonce)?;
        if op != AgentOp::Hello {
            return Err(bad("auth.bind_nonce is accepted only on hello"));
        }
    }
    hex64("payload_sha256", &envelope.payload_sha256)?;
    let canonical = canonical_json(&envelope.payload);
    if canonical.len() > PAYLOAD_MAX_BYTES {
        return Err(bad(format!("payload exceeds {PAYLOAD_MAX_BYTES} bytes")));
    }
    if sha256_hex(canonical.as_bytes()) != envelope.payload_sha256 {
        return Err(bad("payload_sha256 does not match the canonical payload"));
    }
    Ok(op)
}
