//! What a lost host left in its Pi session (T-252 recovery, F1.b): tool calls without results,
//! read from the bound session file with byte bounds and never written, and the resume class
//! of each, read from its receipt after the fence, the drain and reconciliation. Upstream
//! edits, writes and shell commands run in the host process, so their outcome is never claimed.
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use corral_core::agent::{ReceiptState, RequestReceipt, ResumeClass};
use serde::Serialize;
use serde_json::Value;

const SESSION_MAX_BYTES: u64 = 256 * 1024 * 1024;
const LINE_MAX_BYTES: u64 = 16 * 1024 * 1024;
const CALLS_MAX: usize = 256;
const DETAIL_MAX_BYTES: usize = 512;
/// Two-phase operations: without a reservation nothing ran.
const TWO_PHASE: [&str; 3] = ["gate", "capture", "reply"];
const NO_EFFECT: [&str; 4] = ["read", "grep", "find", "ls"];
pub const MUTATING: [&str; 3] = ["edit", "write", "bash"];

pub fn class_name(class: ResumeClass) -> &'static str {
    match class {
        ResumeClass::Result => "result",
        ResumeClass::Rejected => "rejected",
        ResumeClass::Prepared => "prepared",
        ResumeClass::InProgress => "in_progress",
        ResumeClass::OutcomeUnknown => "outcome_unknown",
        ResumeClass::NotCommitted => "not_committed",
        ResumeClass::NotExecuted => "not_executed",
        ResumeClass::InterruptedNoEffect => "interrupted_no_effect",
    }
}

/// One tool call of the lost generation's session with no tool result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DanglingCall {
    pub id: String,
    pub name: String,
}

/// A bound that prevented the scan from observing every call or result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanLimit {
    LineBytes,
    SessionBytes,
    Calls,
    GenerationBoundary,
}

impl ScanLimit {
    pub fn reason(self) -> &'static str {
        match self {
            Self::LineBytes => "a line was skipped at the line byte limit",
            Self::SessionBytes => "the scan stopped at the session byte limit",
            Self::Calls => "calls were omitted at the call limit",
            Self::GenerationBoundary => {
                "legacy session has no generation boundary; unmatched calls may belong to older generations"
            }
        }
    }
}

/// Observed unmatched calls; bounds may have hidden additional calls or their results.
#[derive(Debug, Default)]
pub struct DanglingScan {
    pub calls: Vec<DanglingCall>,
    pub limits: Vec<ScanLimit>,
}

fn skip_line(reader: &mut impl BufRead) -> Result<()> {
    loop {
        let buf = reader.fill_buf().context("reading the Pi session")?;
        if buf.is_empty() {
            return Ok(());
        }
        match buf.iter().position(|b| *b == b'\n') {
            Some(at) => {
                reader.consume(at + 1);
                return Ok(());
            }
            None => {
                let len = buf.len();
                reader.consume(len);
            }
        }
    }
}

/// Unmatched calls after this binding's committed hello marker, in session order.
/// The one-shot bind nonce digest distinguishes review children sharing a conversation
/// and restarting their generation numbers. Generation and session alone are not unique.
/// Never compare cross-process wall clocks: they can step backwards. Legacy files with no
/// marker are scanned conservatively in full and reported as having an attribution limit.
/// The session file is streamed with byte bounds and never written.
pub fn dangling_calls(
    path: &Path,
    generation: u32,
    session_id: &str,
    bind_nonce_sha256: &str,
) -> Result<DanglingScan> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DanglingScan::default());
        }
        Err(error) => return Err(error).with_context(|| format!("opening {}", path.display())),
    };
    ensure!(
        file.metadata()?.is_file(),
        "{} is not a regular file",
        path.display()
    );
    let mut reader = BufReader::new(file.take(SESSION_MAX_BYTES));
    let mut limits = Vec::new();
    let mut open: Vec<DanglingCall> = Vec::new();
    let mut active = true; // Conservative legacy scan until the first binding marker.
    let mut saw_marker = false;
    let mut found_generation = false;
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = (&mut reader)
            .take(LINE_MAX_BYTES)
            .read_until(b'\n', &mut line)
            .context("reading the Pi session")?;
        if read == 0 {
            break;
        }
        if read as u64 == LINE_MAX_BYTES && line.last() != Some(&b'\n') {
            if !limits.contains(&ScanLimit::LineBytes) {
                limits.push(ScanLimit::LineBytes);
            }
            skip_line(&mut reader)?;
            continue;
        }
        let Ok(entry) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        if entry["type"] == "custom"
            && entry["customType"] == "corral_host_bound"
            && entry["data"]["bind_nonce_sha256"].as_str().is_some()
        {
            let matches = entry["data"]["generation"].as_u64() == Some(u64::from(generation))
                && entry["data"]["session_id"].as_str() == Some(session_id)
                && entry["data"]["bind_nonce_sha256"].as_str() == Some(bind_nonce_sha256);
            if found_generation && !matches {
                break;
            }
            saw_marker = true;
            found_generation |= matches;
            active = matches;
            open.clear();
            continue;
        }
        if !active || entry["type"] != "message" {
            continue;
        }
        let message = &entry["message"];
        match message["role"].as_str() {
            Some("assistant") => {
                let items = message["content"].as_array().into_iter().flatten();
                for item in items.filter(|i| i["type"] == "toolCall") {
                    if let (Some(id), Some(name)) = (item["id"].as_str(), item["name"].as_str()) {
                        if open.len() < CALLS_MAX {
                            open.push(DanglingCall {
                                id: id.into(),
                                name: name.into(),
                            });
                        } else if !limits.contains(&ScanLimit::Calls) {
                            limits.push(ScanLimit::Calls);
                        }
                    }
                }
            }
            Some("toolResult") => {
                if let Some(id) = message["toolCallId"].as_str() {
                    open.retain(|c| c.id != id);
                }
            }
            _ => {}
        }
    }
    if !saw_marker {
        limits.push(ScanLimit::GenerationBoundary);
    }
    if reader.get_ref().limit() == 0
        && reader
            .get_ref()
            .get_ref()
            .metadata()
            .context("reading the Pi session size")?
            .len()
            > SESSION_MAX_BYTES
    {
        limits.push(ScanLimit::SessionBytes);
    }
    Ok(DanglingScan {
        calls: open,
        limits,
    })
}

fn bounded(text: String) -> String {
    if text.len() <= DETAIL_MAX_BYTES {
        return text;
    }
    let mut end = DETAIL_MAX_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

fn compact(value: &Value) -> String {
    bounded(serde_json::to_string(value).unwrap_or_default())
}

/// Effects a single-phase operation may have made before its final transaction; each is
/// idempotent, so a retry repeats none of them twice.
fn pre_commit_effects(op: &str) -> &'static str {
    match op {
        "done" => "a commit_all commit of the worktree",
        "artifact" | "evidence" | "checkpoint" | "progress" => "content-addressed blobs",
        _ => "none",
    }
}

/// The class of a dangling corral tool call from its receipt, read after fence, drain and
/// reconciliation. `running` says whether a reserved receipt's executor is still alive.
pub fn classify_corral(
    op: &str,
    receipt: Option<&RequestReceipt>,
    running: bool,
) -> (ResumeClass, String) {
    let Some(receipt) = receipt else {
        return if TWO_PHASE.contains(&op) {
            (
                ResumeClass::NotExecuted,
                "no reservation was committed, so nothing ran".into(),
            )
        } else {
            let effects = pre_commit_effects(op);
            let detail = format!(
                "no receipt after the fence, so it did not commit; possible idempotent \
                 pre-commit effects: {effects}"
            );
            (ResumeClass::NotCommitted, detail)
        };
    };
    let result = &receipt.result;
    let outcome = result["outcome"].as_str().unwrap_or_default();
    match receipt.state {
        ReceiptState::Committed => (ResumeClass::Result, compact(result)),
        ReceiptState::Rejected => (ResumeClass::Rejected, compact(result)),
        ReceiptState::Reserved if running => (
            ResumeClass::InProgress,
            "its executor still ran after drain_secs; one recovery_update reports the final \
             class"
                .into(),
        ),
        ReceiptState::Reserved if outcome == "prepared" => (
            ResumeClass::Prepared,
            format!(
                "attempt {} is prepared and was never submitted",
                result["attempt"].as_str().unwrap_or_default()
            ),
        ),
        ReceiptState::Reserved | ReceiptState::Uncertain => {
            (ResumeClass::OutcomeUnknown, compact(result))
        }
        ReceiptState::Abandoned if outcome == "not_submitted" => (
            ResumeClass::Prepared,
            format!(
                "attempt {} was prepared and never submitted (binding {}); the next \
                 corral_gate adopts it and starts it once",
                result["attempt"].as_str().unwrap_or_default(),
                result["binding"].as_str().unwrap_or_default()
            ),
        ),
        ReceiptState::Abandoned => (ResumeClass::NotExecuted, compact(result)),
    }
}

/// The class of a dangling upstream or local tool call: edits, writes and shell commands may
/// have run in the host process, so their outcome is never claimed.
pub fn classify_upstream(tool: &str, snapshot: Option<&str>) -> (ResumeClass, Option<String>) {
    let mut detail = snapshot.map_or_else(
        || "the worktree may have changed".to_string(),
        |s| format!("the worktree may have changed; see {s}"),
    );
    if tool == "bash" {
        detail.push_str("; processes it started may still run");
    }
    if NO_EFFECT.contains(&tool) || tool.starts_with("lsp_") {
        (ResumeClass::InterruptedNoEffect, None)
    } else if MUTATING.contains(&tool) {
        (ResumeClass::OutcomeUnknown, Some(detail))
    } else {
        (
            ResumeClass::OutcomeUnknown,
            Some(format!("unknown tool; {detail}")),
        )
    }
}

pub fn update_body(receipt: &RequestReceipt, class: ResumeClass, detail: &str) -> String {
    format!(
        "Update to the recovery notice: corral_{} (request {}) settled as {} - {detail}",
        receipt.op,
        receipt.request_id,
        class_name(class)
    )
}
