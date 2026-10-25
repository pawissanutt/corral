//! Public-read cursor v1 (plan T-381 sections 5.3 and 5.4).

use super::binding::canonical_json;
use super::error::{ErrorCode, ExtError};
use serde::{Deserialize, Serialize};

pub const CURSOR_VERSION: u32 = 1;
pub const PROJECTION_VERSION: u32 = 1;
const MAX_CURSOR_CHARS: usize = 4096;

/// Opaque base64url (no padding) of canonical JSON. `seq` is a ChangeSeq and `eseq` an
/// EventSeq captured from the same view; the two are never compared with each other.
/// Not a credential: it carries no MAC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
    pub v: u32,
    pub workspace_id: String,
    pub lineage_id: String,
    pub project: String,
    pub projection: u32,
    pub seq: u64,
    pub eseq: u64,
}

impl Cursor {
    pub fn new(
        workspace_id: impl Into<String>,
        lineage_id: impl Into<String>,
        project: impl Into<String>,
        seq: u64,
        eseq: u64,
    ) -> Self {
        Self {
            v: CURSOR_VERSION,
            workspace_id: workspace_id.into(),
            lineage_id: lineage_id.into(),
            project: project.into(),
            projection: PROJECTION_VERSION,
            seq,
            eseq,
        }
    }

    pub fn encode(&self) -> String {
        let value = serde_json::json!({
            "v": self.v,
            "workspace_id": self.workspace_id,
            "lineage_id": self.lineage_id,
            "project": self.project,
            "projection": self.projection,
            "seq": self.seq,
            "eseq": self.eseq,
        });
        base64url_encode(canonical_json(&value).as_bytes())
    }

    /// Decodes a cursor. Anything malformed, missing `seq`/`eseq`, of another version, or
    /// of another projection is `invalid_request`.
    pub fn decode(text: &str) -> Result<Self, ExtError> {
        if text.len() > MAX_CURSOR_CHARS {
            return Err(ExtError::invalid_request("cursor is too long"));
        }
        let bytes = base64url_decode(text)
            .ok_or_else(|| ExtError::invalid_request("cursor is not unpadded base64url"))?;
        let cursor: Self = serde_json::from_slice(&bytes)
            .map_err(|e| ExtError::invalid_request(format!("cursor is malformed: {e}")))?;
        if cursor.v != CURSOR_VERSION {
            return Err(ExtError::invalid_request(format!(
                "cursor version {} is not {CURSOR_VERSION}",
                cursor.v
            )));
        }
        if cursor.projection != PROJECTION_VERSION {
            return Err(ExtError::invalid_request(format!(
                "cursor projection {} is not {PROJECTION_VERSION}",
                cursor.projection
            )));
        }
        Ok(cursor)
    }

    /// Accepts the cursor only for the reader's workspace, lineage and project. Another
    /// lineage is `lineage_changed`; another workspace or project is `invalid_request`.
    pub fn check_scope(
        &self,
        workspace_id: &str,
        lineage_id: &str,
        project: &str,
    ) -> Result<(), ExtError> {
        if self.workspace_id != workspace_id || self.project != project {
            return Err(ExtError::invalid_request(
                "cursor belongs to another workspace or project",
            ));
        }
        if self.lineage_id != lineage_id {
            return Err(ExtError::new(
                ErrorCode::LineageChanged,
                "cursor belongs to another lineage",
            ));
        }
        Ok(())
    }
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn base64url_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(char::from(ALPHABET[(n >> (18 - 6 * i)) as usize & 63]));
        }
    }
    out
}

fn sextet(c: u8) -> Option<u32> {
    let value = match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'-' => 62,
        b'_' => 63,
        _ => return None,
    };
    Some(u32::from(value))
}

/// Strict decode: unpadded alphabet only, and unused trailing bits must be zero, so every
/// byte string has exactly one accepted encoding.
fn base64url_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if bytes.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3 + 2);
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            n |= sextet(*c)? << (18 - 6 * i);
        }
        let produced = chunk.len() - 1;
        if n & ((1u32 << (24 - 8 * produced)) - 1) != 0 {
            return None;
        }
        for i in 0..produced {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    Some(out)
}
