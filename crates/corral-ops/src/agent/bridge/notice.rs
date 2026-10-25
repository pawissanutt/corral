//! The recovery notice (T-252 recovery): the one message that tells generation n+1 how each
//! dangling call of the lost generation n is classified, and which messages n was sent but never
//! confirmed. Its whole wire envelope fits the Pi adapter's 8 KiB input cap. When the
//! classifications do not fit, the notice lists what fits and names an evidence file that keeps
//! it complete, so every class stays available and nothing queued waits behind it.
//!
//! An envelope generation n was sent and never confirmed would hold its recipient's inbox, so
//! nothing (the notice included) could reach n+1. Resume settles it without a resend: a
//! `corral_delivery` entry in the continuing Pi session observes its members, and otherwise
//! each unobserved member is marked failed `delivery_unknown`, which only an explicit
//! `corral inbox retry` resends. The notice reports those as delivery unknown.
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use anyhow::{Context, Result};
use corral_core::ArtifactKind;
use corral_core::agent::ContainerRef;
use corral_core::inbox::{Inbox, Receipt, ReceiptLevel, Recipient, TerminalState};
use jiff::Timestamp;
use serde_json::{Value, json};

use super::interrupted::class_name;
use crate::Ctx;
use crate::agent::delivery::{ENTRY_TYPE, INPUT_MAX_BYTES, envelope_key};
use crate::agent::recovery::{Interrupted, NOTICE_KIND};

/// The session read for an unconfirmed envelope's entry stops here; a longer line is skipped.
const MAX_SESSION_BYTES: u64 = 256 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 1024 * 1024;

/// A message the lost generation was sent but never confirmed, as resume settled it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unconfirmed {
    pub id: String,
    pub kind: String,
    /// The continuing Pi session holds its `corral_delivery` entry: it was delivered.
    pub observed: bool,
}

/// Settles the open envelope generation `lost` (bound at `endpoint`) was sent and never
/// confirmed, from the evidence of the session file both generations share.
pub fn settle_unconfirmed(
    ctx: &Ctx,
    recipient: &Recipient,
    endpoint: &Path,
    session_file: &Path,
) -> Result<Vec<Unconfirmed>> {
    let inbox = Inbox::new(&ctx.store, recipient.clone())?;
    let _deliver = inbox.delivery_lock()?;
    inbox.recover_publication()?;
    let Some(number) = inbox.state()?.open_envelope else {
        return Ok(Vec::new());
    };
    let envelope = inbox.envelope(number)?;
    if envelope.attempts.is_empty() || envelope.binding.endpoint.as_deref() != Some(endpoint) {
        return Ok(Vec::new());
    }
    let entry = delivery_entry(session_file, inbox.key(), number, &envelope.payload_sha256)?;
    let reason = format!(
        "delivery_unknown: sent to the host at {} that was lost before it confirmed the \
         message; never resent automatically",
        endpoint.display()
    );
    let mut settled = Vec::new();
    for seq in &envelope.members {
        if inbox.receipt(*seq, ReceiptLevel::Observed)?.is_some() || inbox.terminal(*seq)?.is_some()
        {
            continue;
        }
        let message = inbox.message(*seq)?;
        let observed = match &entry {
            Some(detail) => {
                let receipt = Receipt {
                    envelope: Some(number),
                    binding: Some(envelope.binding.clone()),
                    observed_at: Timestamp::now(),
                    via: "recovery".into(),
                    detail: detail.clone(),
                };
                inbox.record_receipt(*seq, ReceiptLevel::Observed, &receipt)?;
                true
            }
            None => {
                inbox.mark_terminal(*seq, TerminalState::Failed, &reason, "recovery")?;
                false
            }
        };
        settled.push(Unconfirmed {
            id: message.id,
            kind: message.kind,
            observed,
        });
    }
    inbox.close_settled(&envelope)?;
    let members: Vec<Value> = settled
        .iter()
        .map(|m| json!({"id": m.id, "observed": m.observed}))
        .collect();
    inbox.event(
        "delivery_fenced",
        json!({"envelope": number, "endpoint": endpoint, "members": members}),
    )?;
    Ok(settled)
}

/// The `corral_delivery` entry the host wrote for envelope `number` of `key`, when the session
/// holds one; its fields become the observation's detail.
fn delivery_entry(path: &Path, key: &str, number: u64, sha256: &str) -> Result<Option<Value>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("opening {}", path.display())),
    };
    let mut reader = BufReader::new(file.take(MAX_SESSION_BYTES));
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(None);
        }
        if line.len() > MAX_LINE_BYTES {
            continue;
        }
        let Ok(entry) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        let data = &entry["data"];
        let named = data["idempotency_key"].as_str().and_then(envelope_key);
        if entry["type"] == "custom"
            && entry["customType"] == ENTRY_TYPE
            && named
                .as_ref()
                .is_some_and(|(k, n)| k == key && *n == number)
            && data["text_sha256"].as_str() == Some(sha256)
        {
            return Ok(Some(json!({
                "entry_id": entry["id"], "timestamp": entry["timestamp"],
                "request_id": data["request_id"], "generation": data["generation"],
            })));
        }
    }
}

/// The notice body budget: its envelope header and its own header line, with the widest
/// sequence numbers, leave this much of the adapter's input cap.
fn budget(key: &str) -> usize {
    let envelope = format!("[corral envelope {key}@env{}]\n\n", u64::MAX).len();
    let header = format!("[corral {key}#{} {NOTICE_KIND}]\n", u64::MAX).len();
    INPUT_MAX_BYTES.saturating_sub(envelope + header)
}

struct Parts {
    head: String,
    lines: Vec<String>,
    foot: &'static str,
}

fn parts(lost: u32, next: u32, scan: &Interrupted, unconfirmed: &[Unconfirmed]) -> Parts {
    let calls = &scan.calls;
    let mut head = format!(
        "corral resumed this host as generation {next} after generation {lost} was lost. \
         Nothing was replayed. "
    );
    if scan.scan_limits.is_empty() {
        head.push_str(match calls.is_empty() {
            true => "No tool call was left without a result.\n",
            false => "Calls left without a result:\n",
        });
    } else {
        let reasons = scan
            .scan_limits
            .iter()
            .map(|limit| limit.reason())
            .collect::<Vec<_>>()
            .join("; ");
        head.push_str(&format!(
            "Dangling-call scan incomplete: {reasons}. Calls or results may be missing; \
             no unobserved call is claimed absent.\n"
        ));
        if !calls.is_empty() {
            head.push_str("Calls observed without a result in the incomplete scan:\n");
        }
    }
    let mut lines: Vec<String> = calls
        .iter()
        .map(|call| {
            let mut line = format!(
                "- {} (call {}): {}",
                call.tool,
                call.tool_call_id,
                class_name(call.class)
            );
            if let Some(detail) = &call.detail {
                line.push_str(&format!(" - {detail}"));
            }
            line
        })
        .collect();
    lines.extend(unconfirmed.iter().filter(|m| !m.observed).map(|m| {
        format!(
            "- message {} ({}): delivery unknown - sent to generation {lost}, which never \
             confirmed it; it is not resent (ask the owner if you need it)",
            m.id, m.kind
        )
    }));
    Parts {
        head,
        lines,
        foot: "Check each before relying on it. A prepared gate starts once when you call \
               corral_gate again; never repeat an edit, write or command without inspecting the \
               worktree first.",
    }
}

fn render(parts: &Parts, kept: usize, complete: Option<&str>) -> String {
    let mut body = parts.head.clone();
    for line in &parts.lines[..kept] {
        body.push_str(line);
        body.push('\n');
    }
    if let Some(path) = complete {
        let more = parts.lines.len() - kept;
        body.push_str(&format!(
            "- {more} more not shown here: read the complete notice at {path}\n"
        ));
    }
    body.push_str(parts.foot);
    body
}

/// The notice body for `key`'s inbox. A body over the budget keeps as many leading lines as
/// fit and names the evidence file `keep` wrote with the complete notice.
pub fn body(
    key: &str,
    lost: u32,
    next: u32,
    scan: &Interrupted,
    unconfirmed: &[Unconfirmed],
    keep: impl FnOnce(&str) -> Result<String>,
) -> Result<String> {
    let parts = parts(lost, next, scan, unconfirmed);
    let full = render(&parts, parts.lines.len(), None);
    let budget = budget(key);
    if full.len() <= budget {
        return Ok(full);
    }
    let path = keep(&full)?;
    let mut kept = parts.lines.len();
    while kept > 0 && render(&parts, kept, Some(&path)).len() > budget {
        kept -= 1;
    }
    let body = render(&parts, kept, Some(&path));
    anyhow::ensure!(
        body.len() <= budget,
        "the recovery notice cannot fit {budget} bytes even without its call lines"
    );
    Ok(body)
}

/// Writes the complete notice under the store's state and, for a task, attaches it to the run
/// as evidence; returns the absolute path the notice names.
pub fn keep_complete(
    ctx: &Ctx,
    container: &ContainerRef,
    lost: u32,
    artifact_run: Option<&str>,
    text: &str,
) -> Result<String> {
    let dir = ctx.store.state_dir().join("agent-recovery");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let name = container.key().replace('/', "-");
    let path = dir.join(format!("{name}-g{lost}-notice.md"));
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    let (Some(task), Some(run)) = (container.task(), artifact_run) else {
        return Ok(path.display().to_string());
    };
    let title = format!(
        "Complete recovery notice ({} generation {lost})",
        container.key()
    );
    let artifact = crate::artifacts::add_artifact_for_run(
        ctx,
        task,
        run,
        &path,
        Some(ArtifactKind::Evidence),
        Some(&title),
    )?;
    Ok(ctx.store.dir().join(&artifact.path).display().to_string())
}
