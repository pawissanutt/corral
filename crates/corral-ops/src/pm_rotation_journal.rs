//! What the bound Pi session journal says the old conversation still owes. Read the way the
//! host restores it: the latest record per identity, consumption proven by a persisted user
//! message or a terminal queue event, never by intent entries.
use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use corral_core::agent::paths::sha256_hex;
use serde::Deserialize;
use serde_json::Value;

use super::{Blocker, DialogState};

/// `host/pending-questions.ts`: one record per owner-dialog transition.
const DIALOG_ENTRY: &str = "corral_owner_question";
/// `host/queue.ts`: one record per steer/follow-up queue transition.
const QUEUE_ENTRY: &str = "corral_queue";
/// `delivery/index.ts`: written before Pi sees a bridge input's text.
const DELIVERY_ENTRY: &str = crate::agent::delivery::ENTRY_TYPE;
const JOURNAL_MAX_BYTES: u64 = crate::agent::usage::SESSION_MAX_BYTES;
const LINE_MAX_BYTES: usize = 1024 * 1024;

#[derive(Deserialize)]
struct Dialog {
    version: u32,
    id: String,
    session: String,
    status: String,
    /// Milliseconds since the epoch.
    expires: f64,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Deserialize)]
struct Queued {
    version: u32,
    session_id: String,
    generation: u32,
    item: String,
    event: String,
    origin: String,
    mode: String,
    #[serde(default)]
    idempotency_key: Option<String>,
}

#[derive(Deserialize)]
struct Delivery {
    version: u32,
    idempotency_key: String,
    text_sha256: String,
    generation: u32,
    session_id: String,
}

/// Owner dialogs of `session` pending and unexpired or answered but not yet a user message;
/// queue items of generation `generation` whose latest event is `queued`; and deliveries of
/// that generation that neither became a user message nor reached a terminal queue event.
/// A missing file owes nothing; a file that cannot be read completely is an error.
pub(super) fn owed(
    file: &Path,
    session: &str,
    generation: u32,
    now_ms: i64,
) -> Result<Vec<Blocker>> {
    let handle = match std::fs::File::open(file) {
        Ok(handle) => handle,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("opening {}", file.display())),
    };
    let mut reader = BufReader::new(handle.take(JOURNAL_MAX_BYTES + 1));
    let mut dialogs = BTreeMap::new();
    let mut queue: BTreeMap<String, Queued> = BTreeMap::new();
    let mut deliveries = BTreeMap::new();
    let mut settled = HashSet::new();
    let mut users = HashSet::new();
    let mut read = 0u64;
    let mut line = Vec::new();
    loop {
        line.clear();
        let n = reader
            .read_until(b'\n', &mut line)
            .with_context(|| format!("reading {}", file.display()))?;
        if n == 0 {
            break;
        }
        read += u64::try_from(n).context("journal length overflow")?;
        ensure!(
            read <= JOURNAL_MAX_BYTES,
            "{} exceeds {JOURNAL_MAX_BYTES} bytes",
            file.display()
        );
        // A skipped line can only make something look owed, never consumed.
        if line.len() > LINE_MAX_BYTES
            || ![
                DIALOG_ENTRY.as_bytes(),
                QUEUE_ENTRY.as_bytes(),
                DELIVERY_ENTRY.as_bytes(),
                b"\"user\"".as_slice(),
            ]
            .iter()
            .any(|needle| contains(&line, needle))
        {
            continue;
        }
        let Ok(entry) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        let data = &entry["data"];
        if entry["type"] == "message" && entry["message"]["role"] == "user" {
            users.insert(sha256_hex(
                user_text(&entry["message"]["content"]).as_bytes(),
            ));
            continue;
        }
        if entry["type"] != "custom" {
            continue;
        }
        if entry["customType"] == DIALOG_ENTRY {
            if let Ok(dialog) = Dialog::deserialize(data)
                && dialog.version == 1
                && dialog.session == session
            {
                dialogs.insert(dialog.id.clone(), dialog);
            }
        } else if entry["customType"] == QUEUE_ENTRY {
            if let Ok(item) = Queued::deserialize(data)
                && item.version == 1
                && item.session_id == session
                && item.generation == generation
            {
                if item.event != "queued"
                    && let Some(key) = &item.idempotency_key
                {
                    settled.insert(key.clone());
                }
                queue.insert(item.item.clone(), item);
            }
        } else if entry["customType"] == DELIVERY_ENTRY
            && let Ok(delivery) = Delivery::deserialize(data)
            && delivery.version == 1
            && delivery.session_id == session
            && delivery.generation == generation
        {
            deliveries.insert(delivery.idempotency_key, delivery.text_sha256);
        }
    }
    let now = now_ms as f64;
    let mut owed: Vec<Blocker> = dialogs
        .into_values()
        .filter_map(|dialog| {
            let state = match dialog.status.as_str() {
                "pending" if dialog.expires > now => DialogState::Pending,
                "answered"
                    if dialog
                        .message
                        .as_deref()
                        .is_none_or(|message| !users.contains(&sha256_hex(message.as_bytes()))) =>
                {
                    DialogState::AnsweredUndelivered
                }
                _ => return None,
            };
            Some(Blocker::OwnerDialog {
                id: dialog.id,
                state,
            })
        })
        .collect();
    owed.extend(
        queue
            .into_values()
            .filter(|item| item.event == "queued")
            .map(|item| Blocker::QueuedInput {
                item: item.item,
                origin: item.origin,
                mode: item.mode,
            }),
    );
    owed.extend(
        deliveries
            .into_iter()
            .filter(|(key, sha)| !settled.contains(key) && !users.contains(sha))
            .map(|(key, _)| Blocker::UnconsumedDelivery { key }),
    );
    Ok(owed)
}

/// A user message's text exactly as the host compares it with a recorded answer.
fn user_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .map(|part| {
                if part["type"] == "text" {
                    part["text"].as_str().unwrap_or("")
                } else {
                    ""
                }
            })
            .collect(),
        _ => String::new(),
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
