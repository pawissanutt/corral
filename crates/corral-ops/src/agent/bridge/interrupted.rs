//! What a lost host left in its Pi session (T-252 recovery, F1.b): tool calls without results,
//! read from the bound session file with byte bounds and never written, and the resume class
//! of each, read from its receipt after the fence, the drain and reconciliation. Upstream
//! edits, writes and shell commands run in the host process, so their outcome is never claimed.
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use corral_core::agent::{
    HookSettings, InterruptedCall, ReceiptState, RequestReceipt, ResumeClass,
};
use serde::Serialize;
use serde_json::Value;

const SESSION_MAX_BYTES: u64 = 256 * 1024 * 1024;
const LINE_MAX_BYTES: u64 = 16 * 1024 * 1024;
const CALLS_MAX: usize = 256;
const HOOKS_MAX: usize = 256;
/// The custom session entry the host appends before, at and after each hook launch (T-572).
pub const HOOK_ENTRY: &str = "corral_hook";
const HOOK_PHASES: [&str; 3] = ["pre_tool", "post_tool", "turn_end"];
const DETAIL_MAX_BYTES: usize = 512;
/// Two-phase operations: without a reservation nothing ran.
const TWO_PHASE: [&str; 5] = ["gate", "capture", "reply", "plan_submit", "plan_verdict"];
// Web tools are GET-only; tool_search reads only the host's local cached catalog; job_output
// reads the host's output cache.
const NO_EFFECT: [&str; 8] = [
    "read",
    "grep",
    "find",
    "ls",
    "web_fetch",
    "web_search",
    "tool_search",
    "job_output",
];
pub const MUTATING: [&str; 5] = ["edit", "write", "bash", "job_start", "job_kill"];

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
    Hooks,
    GenerationBoundary,
}

impl ScanLimit {
    pub fn reason(self) -> &'static str {
        match self {
            Self::LineBytes => "a line was skipped at the line byte limit",
            Self::SessionBytes => "the scan stopped at the session byte limit",
            Self::Calls => "calls were omitted at the call limit",
            Self::Hooks => "hook records were omitted at the hook limit",
            Self::GenerationBoundary => {
                "legacy session has no generation boundary; unmatched calls may belong to older generations"
            }
        }
    }
}

/// One hook invocation the host recorded in the bound session before launching it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRecord {
    pub invocation: String,
    pub phase: String,
    pub hook: String,
    pub tool_call_id: Option<String>,
    pub turn: Option<u64>,
    pub state: String,
}

impl HookRecord {
    fn parse(data: &Value) -> Option<Self> {
        let text = |key: &str, max: usize| {
            data[key]
                .as_str()
                .filter(|value| !value.is_empty() && value.len() <= max)
                .map(str::to_owned)
        };
        let phase = text("phase", 16).filter(|phase| HOOK_PHASES.contains(&phase.as_str()))?;
        let state = text("state", 16)
            .filter(|state| ["prepared", "started", "completed"].contains(&state.as_str()))?;
        Some(Self {
            invocation: text("invocation", 128)?,
            phase,
            hook: text("hook", 64)?,
            tool_call_id: text("tool_call_id", 512),
            turn: data["turn"].as_u64(),
            state,
        })
    }

    fn completed(&self) -> bool {
        self.state == "completed"
    }

    fn reference(&self) -> String {
        format!(
            "{} ({} {}, {})",
            self.invocation, self.phase, self.hook, self.state
        )
    }
}

/// Observed unmatched calls; bounds may have hidden additional calls or their results.
/// `hooks` holds the invocations of open calls and every hook that never completed.
#[derive(Debug, Default)]
pub struct DanglingScan {
    pub calls: Vec<DanglingCall>,
    pub limits: Vec<ScanLimit>,
    pub hooks: Vec<HookRecord>,
}

fn observe_hook(hooks: &mut Vec<HookRecord>, limits: &mut Vec<ScanLimit>, data: &Value) {
    let Some(record) = HookRecord::parse(data) else {
        return;
    };
    let finished_turn_hook = record.completed() && record.tool_call_id.is_none();
    match hooks.iter().position(|h| h.invocation == record.invocation) {
        Some(at) if finished_turn_hook => {
            hooks.remove(at);
        }
        Some(at) => hooks[at] = record,
        None if finished_turn_hook => {}
        None if hooks.len() < HOOKS_MAX => hooks.push(record),
        None => {
            if !limits.contains(&ScanLimit::Hooks) {
                limits.push(ScanLimit::Hooks);
            }
        }
    }
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
    let mut hooks: Vec<HookRecord> = Vec::new();
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
            hooks.clear();
            continue;
        }
        if active && entry["type"] == "custom" && entry["customType"] == HOOK_ENTRY {
            observe_hook(&mut hooks, &mut limits, &entry["data"]);
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
                    hooks.retain(|h| !h.completed() || h.tool_call_id.as_deref() != Some(id));
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
    hooks.retain(|h| {
        !h.completed()
            || h.tool_call_id
                .as_ref()
                .is_some_and(|id| open.iter().any(|c| &c.id == id))
    });
    Ok(DanglingScan {
        calls: open,
        limits,
        hooks,
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
        "plan_write" => "the plan or notes file, rewritten identically by a retry",
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
        ReceiptState::Queued => (ResumeClass::NotExecuted, compact(result)),
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
    if tool == "job_start" || tool == "job_kill" {
        detail.push_str("; corral reaps a recorded job of the lost generation");
    }
    if NO_EFFECT.contains(&tool) {
        (ResumeClass::InterruptedNoEffect, None)
    } else if tool == "codemode" {
        (
            ResumeClass::OutcomeUnknown,
            Some(format!(
                "nested tools may have run; reconcile their effects before retrying; {detail}"
            )),
        )
    } else if tool.starts_with("lsp_") {
        (
            ResumeClass::OutcomeUnknown,
            Some("a language server may have started or acted; reconcile before retrying".into()),
        )
    } else if tool.starts_with("mcp_") {
        (
            ResumeClass::OutcomeUnknown,
            Some("the MCP connection may have started or the server may have acted; reconcile before retrying".into()),
        )
    } else if tool.starts_with("browser_") {
        (
            ResumeClass::OutcomeUnknown,
            Some("the browser may have loaded the page and run its scripts, which can send requests; \
                  its subprocess exits with the host and nothing is replayed".into()),
        )
    } else if MUTATING.contains(&tool) {
        (ResumeClass::OutcomeUnknown, Some(detail))
    } else {
        (
            ResumeClass::OutcomeUnknown,
            Some(format!("unknown tool; {detail}")),
        )
    }
}

/// `classify_upstream` with the hook overlay: a hook recorded around the call, or a configured
/// pre/post-tool hook of the lost generation that wraps it, may have changed the worktree, so
/// even a read is `outcome_unknown`. Hook scripts are never replayed.
pub fn classify_local(
    call: &DanglingCall,
    (snapshot, hooks, settings): (Option<&str>, &[HookRecord], Option<&HookSettings>),
) -> (ResumeClass, Option<String>) {
    let (class, detail) = classify_upstream(&call.name, snapshot);
    let attributed: Vec<String> = hooks
        .iter()
        .filter(|hook| hook.tool_call_id.as_deref() == Some(call.id.as_str()))
        .map(HookRecord::reference)
        .collect();
    if attributed.is_empty() && !settings.is_some_and(|hooks| hooks.wraps(&call.name)) {
        return (class, detail);
    }
    let note = if attributed.is_empty() {
        "a configured pre/post-tool hook may have run around it".to_owned()
    } else {
        format!(
            "hook invocations {} ran around it; corral does not replay hook scripts",
            attributed.join(", ")
        )
    };
    let detail = match detail {
        Some(detail) => format!("{detail}; {note}"),
        None => match snapshot {
            Some(snapshot) => format!("{note}; the worktree may have changed; see {snapshot}"),
            None => format!("{note}; the worktree may have changed"),
        },
    };
    (ResumeClass::OutcomeUnknown, Some(bounded(detail)))
}

/// Whether recovery should cite a worktree snapshot: a mutating call, a recorded hook, or a call
/// a configured hook of the lost generation wraps.
pub fn needs_snapshot(scan: &DanglingScan, hooks: Option<&HookSettings>) -> bool {
    !scan.hooks.is_empty()
        || scan.calls.iter().any(|call| {
            MUTATING.contains(&call.name.as_str())
                || call.name == "codemode"
                || hooks.is_some_and(|h| h.wraps(&call.name))
        })
}

/// Hooks that never completed and belong to no open call (turn-end hooks, or hooks around a
/// call that has a result): each is reported `outcome_unknown` under its invocation ID.
pub fn unfinished_hooks(scan: &DanglingScan) -> Vec<InterruptedCall> {
    scan.hooks
        .iter()
        .filter(|hook| {
            !hook
                .tool_call_id
                .as_ref()
                .is_some_and(|id| scan.calls.iter().any(|call| &call.id == id))
        })
        .map(|hook| {
            let around = match (&hook.tool_call_id, hook.turn) {
                (Some(id), _) => format!(" around tool call {id}"),
                (None, Some(turn)) => format!(" for turn {turn}"),
                (None, None) => String::new(),
            };
            InterruptedCall {
                tool_call_id: hook.invocation.clone(),
                tool: format!("hook:{}:{}", hook.phase, hook.hook),
                request_id: None,
                class: ResumeClass::OutcomeUnknown,
                detail: Some(bounded(format!(
                    "{} hook {} was {}{around} and did not complete; its effects are unknown and \
                     corral does not replay hook scripts",
                    hook.phase, hook.hook, hook.state
                ))),
            }
        })
        .collect()
}

pub fn update_body(receipt: &RequestReceipt, class: ResumeClass, detail: &str) -> String {
    format!(
        "Update to the recovery notice: corral_{} (request {}) settled as {} - {detail}",
        receipt.op,
        receipt.request_id,
        class_name(class)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_search_is_no_effect_but_mcp_and_unknown_tools_are_uncertain() {
        // Given unmatched cached discovery, connection-backed calls, and unknown names.
        for (tool, class) in [
            ("tool_search", ResumeClass::InterruptedNoEffect),
            ("mcp_connect", ResumeClass::OutcomeUnknown),
            ("mcp_activate", ResumeClass::OutcomeUnknown),
            ("mcp_read_resource", ResumeClass::OutcomeUnknown),
            ("mcp_list_resources", ResumeClass::OutcomeUnknown),
            ("mcp_docs_search", ResumeClass::OutcomeUnknown),
            ("mcp_tool_search", ResumeClass::OutcomeUnknown),
            ("tool_search_remote", ResumeClass::OutcomeUnknown),
            ("unknown", ResumeClass::OutcomeUnknown),
        ] {
            // When classifying recovery, then only exact local discovery promises no effect.
            let (actual, detail) = classify_upstream(tool, None);
            assert_eq!(actual, class, "{tool}");
            assert_eq!(detail.is_none(), class == ResumeClass::InterruptedNoEffect);
        }
    }

    #[test]
    fn interrupted_codemode_is_unknown_even_when_its_script_looks_read_only() {
        // The outer call has no final result: nested calls may already have acted.
        let scan = DanglingScan {
            calls: vec![DanglingCall {
                id: "script-1".into(),
                name: "codemode".into(),
            }],
            ..Default::default()
        };
        assert!(needs_snapshot(&scan, None));
        let (class, detail) = classify_local(&scan.calls[0], (Some("saved.diff"), &[], None));
        assert_eq!(class, ResumeClass::OutcomeUnknown);
        let detail = detail.unwrap();
        assert!(detail.contains("nested tools may have run"));
        assert!(detail.contains("saved.diff"));
    }

    #[test]
    fn get_only_web_tools_are_interrupted_without_local_effects() {
        // Given known GET-only, no-credential web tools and an unknown tool.
        for tool in ["web_fetch", "web_search"] {
            // When classifying their unmatched calls, then retry needs no effect reconciliation.
            assert_eq!(
                classify_upstream(tool, Some("saved.diff")),
                (ResumeClass::InterruptedNoEffect, None)
            );
        }
        assert_eq!(
            classify_upstream("web_post", None).0,
            ResumeClass::OutcomeUnknown
        );
    }
}
