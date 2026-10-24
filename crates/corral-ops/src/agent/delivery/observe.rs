//! Observation of a bound Pi session file: complete JSONL lines after the cursor, matched to an
//! envelope only by a `corral_delivery` custom entry naming its key, binding and SHA-256.
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use corral_core::inbox::{Coverage, Inbox, ObserveCursor, Recipient, SessionBinding};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{ADAPTER, ENTRY_TYPE, ORIGIN, endpoint_generation, envelope_key};
use crate::Ctx;
use crate::delivery::Observation;

const MAX_SESSION_READ: u64 = 32 * 1024 * 1024;

pub(super) fn sha256(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

pub(super) fn observe(
    ctx: &Ctx,
    binding: &SessionBinding,
    cursor: &mut ObserveCursor,
) -> Result<Vec<Observation>> {
    let path = binding
        .transcript
        .as_deref()
        .context("bound Pi session has no session file")?;
    let generation = binding
        .endpoint
        .as_deref()
        .and_then(endpoint_generation)
        .map(|(_, n)| n);
    let mut out = Vec::new();
    for line in read_new_lines(path, cursor)? {
        let Ok(entry) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        if let Some(observation) = observed(ctx, binding, generation, &entry)? {
            out.push(observation);
        }
    }
    Ok(out)
}

/// Pi writes its session file once the first conversation message exists and appends after
/// that, so a missing file is unreadable for now and a shrunken one restarts from its origin.
fn read_new_lines(path: &Path, cursor: &mut ObserveCursor) -> Result<Vec<Vec<u8>>> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            cursor.coverage = Coverage::Unreadable;
            return Ok(Vec::new());
        }
        Err(error) => return Err(error).with_context(|| format!("opening {}", path.display())),
    };
    let meta = file.metadata().context("inspecting the Pi session file")?;
    ensure!(meta.is_file(), "the Pi session file must be a regular file");
    if meta.len() < cursor.offset {
        *cursor = ObserveCursor::default();
        cursor.coverage = Coverage::Truncated;
    }
    file.seek(SeekFrom::Start(cursor.offset))
        .context("seeking the Pi session file")?;
    let mut bytes = Vec::new();
    file.take(MAX_SESSION_READ)
        .read_to_end(&mut bytes)
        .context("reading the Pi session file")?;
    let complete = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    let lines: Vec<Vec<u8>> = bytes[..complete]
        .split_inclusive(|b| *b == b'\n')
        .map(<[u8]>::to_vec)
        .collect();
    cursor.offset += u64::try_from(complete)?;
    cursor.last_complete_line += u64::try_from(lines.len())?;
    if cursor.coverage == Coverage::Unreadable {
        cursor.coverage = Coverage::Complete;
    }
    Ok(lines)
}

fn observed(
    ctx: &Ctx,
    binding: &SessionBinding,
    generation: Option<u32>,
    entry: &Value,
) -> Result<Option<Observation>> {
    if entry["type"] != "custom" || entry["customType"] != ENTRY_TYPE {
        return Ok(None);
    }
    let data = &entry["data"];
    if data["origin"] != ORIGIN
        || data["session_id"].as_str() != Some(binding.session.as_str())
        || generation.is_none_or(|n| data["generation"] != json!(n))
    {
        return Ok(None);
    }
    let Some((key, number)) = data["idempotency_key"].as_str().and_then(envelope_key) else {
        return Ok(None);
    };
    let Ok(recipient) = Recipient::parse_key(&key) else {
        return Ok(None);
    };
    let inbox = Inbox::new(&ctx.store, recipient)?;
    let Ok(envelope) = inbox.envelope(number) else {
        return Ok(None);
    };
    if envelope.adapter != ADAPTER
        || &envelope.binding != binding
        || data["text_sha256"].as_str() != Some(envelope.payload_sha256.as_str())
    {
        return Ok(None);
    }
    Ok(Some(Observation::Delivered {
        envelope: number,
        members: envelope.members.clone(),
        detail: json!({
            "entry_id": entry["id"], "timestamp": entry["timestamp"],
            "request_id": data["request_id"], "idempotency_key": data["idempotency_key"],
            "generation": data["generation"],
        }),
    }))
}
