//! OmO thread gateway (`omo thread`): bind by `thread list`, send with an idempotency key,
//! observe the typed `session_control_delivery` transcript entry.
//! Result shapes and exit codes are pinned from omo 5.1.35 (fixtures in `omo_fixtures/`).

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use corral_core::inbox::{Coverage, Inbox, ReceiptLevel, Recipient};
use jiff::Timestamp;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::Ctx;
use crate::delivery::{
    BindOutcome, Capabilities, DeliveryAdapter, Envelope, Observation, ObserveCursor, Readiness,
    SendOutcome, SessionBinding,
};

pub const HARNESS: &str = "omo";
/// Gateway store locks wait up to 30 s; a send that outlives this is uncertain, never failed.
const SEND_TIMEOUT: Duration = Duration::from_secs(90);
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_TRANSCRIPT_READ: u64 = 32 * 1024 * 1024;

/// Errors that prove nothing was accepted yet or that only the same key can settle.
const RETRYABLE: [&str; 5] = [
    "idempotency_uncertain",
    "idempotency_in_progress",
    "idempotency_conflict",
    "overloaded",
    "transport_closed",
];

pub struct OmoAdapter {
    program: Option<PathBuf>,
}

pub(crate) struct Target {
    pub cwd: PathBuf,
    pub pane: Option<String>,
    pub started: Timestamp,
    pub ended: Option<Timestamp>,
}

impl OmoAdapter {
    /// `CORRAL_OMO_PROGRAM` (absolute path) wins; otherwise `omo` from PATH.
    /// Test builds never resolve the owner's `omo` from PATH (hermetic harness guard).
    pub fn from_env() -> Self {
        let program = std::env::var_os("CORRAL_OMO_PROGRAM")
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| crate::harness::find_executable("omo"));
        Self { program }
    }

    pub fn with_program(program: impl Into<PathBuf>) -> Self {
        Self {
            program: Some(program.into()),
        }
    }

    fn program(&self) -> Result<&Path> {
        self.program
            .as_deref()
            .context("omo is not installed on PATH")
    }

    fn run(&self, args: &[&str], timeout: Duration) -> Result<(Option<i32>, String)> {
        crate::harness::run_status(self.program()?, args, timeout)
    }
}

pub(crate) fn target(ctx: &Ctx, recipient: &Recipient) -> Result<Target> {
    let task = ctx.store.get(
        recipient
            .task()
            .context("OmO delivery requires a task recipient")?,
    )?;
    match recipient {
        Recipient::Run { run, .. } => {
            let run = task
                .runs
                .iter()
                .find(|r| &r.id == run)
                .context("inbox run not found")?;
            Ok(Target {
                cwd: run.worktree.clone(),
                pane: run.herdr.as_ref().map(|h| h.pane_id.clone()),
                started: run.started,
                ended: run.ended,
            })
        }
        Recipient::PlanRun { plan_run, .. } => {
            let run = task
                .plan_runs
                .iter()
                .find(|r| &r.id == plan_run)
                .context("inbox plan run not found")?;
            Ok(Target {
                cwd: ctx
                    .store
                    .project_path(task.project.as_deref().context("planner has no project")?)?,
                pane: run.herdr.as_ref().map(|h| h.pane_id.clone()),
                started: run.started,
                ended: run.ended,
            })
        }
        _ => anyhow::bail!("OmO delivery is enabled only for coding and planning runs"),
    }
}

pub(crate) fn candidates(list: &Value, target: &Target, now: Timestamp) -> Result<Vec<Value>> {
    let rows = list.as_array().context("omo thread list is not an array")?;
    let mut out = Vec::new();
    for row in rows {
        let created = row["created_at"]
            .as_str()
            .and_then(|s| s.parse::<Timestamp>().ok());
        if row["status"] == "live"
            && row["alive"] == true
            && row["surface"] == "tui"
            && row["cwd"].as_str().map(Path::new) == Some(target.cwd.as_path())
            && row["controls"]
                .as_array()
                .is_some_and(|c| c.iter().any(|c| c == "send"))
            && row["durableSessionId"].as_str().is_some()
            && created.is_some_and(|at| at >= target.started && at <= target.ended.unwrap_or(now))
        {
            out.push(row.clone());
        }
    }
    Ok(out)
}

pub(crate) fn classify(status: Option<i32>, stdout: &str) -> SendOutcome {
    let parsed: Option<Value> = serde_json::from_str(stdout.trim()).ok();
    let code = parsed
        .as_ref()
        .and_then(|v| v["error"]["code"].as_str())
        .unwrap_or("")
        .to_string();
    match (status, parsed) {
        (Some(0), Some(value)) if value["kind"] == "ok" && value["delivery_id"].is_string() => {
            if value["delivery"]["kind"] == "queued_offline" {
                SendOutcome::Held {
                    reason: format!("gateway holds delivery {} offline", value["delivery_id"]),
                }
            } else {
                SendOutcome::Admitted { detail: value }
            }
        }
        (Some(1), Some(_)) if RETRYABLE.contains(&code.as_str()) => SendOutcome::Uncertain {
            reason: format!("omo thread send: {code}"),
        },
        (Some(1 | 2), Some(value)) => SendOutcome::Rejected {
            reason: format!(
                "omo thread send: {code}: {}",
                value["error"]["message"].as_str().unwrap_or("refused")
            ),
            retryable_after_fix: matches!(code.as_str(), "not_found" | "queue_full"),
        },
        // Unsupported runtime after publication: definitely not delivered, never fallback.
        (Some(4), _) => SendOutcome::Rejected {
            reason: format!("omo thread send unsupported: {code}"),
            retryable_after_fix: true,
        },
        // 3 host_unavailable, 5 internal_error, a crash, timeout or unparsable output: the
        // gateway may already hold the delivery; only the same key may settle it.
        (status, _) => SendOutcome::Uncertain {
            reason: format!("omo thread send exit {status:?}: {code}"),
        },
    }
}

impl DeliveryAdapter for OmoAdapter {
    fn harness(&self) -> &'static str {
        HARNESS
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            idempotent_retry: true,
            admission: true,
            non_waking: false,
            max_payload: corral_core::inbox::MAX_PAYLOAD,
            send_budget: None,
            transcript_reader: true,
        }
    }

    fn readiness(&self, _ctx: &Ctx, _recipient: &Recipient) -> Result<Readiness> {
        if self.program.is_none() {
            return Ok(Readiness::Unavailable {
                reason: "omo is not installed on PATH".into(),
            });
        }
        Ok(match self.run(&["thread", "--help"], PROBE_TIMEOUT) {
            Ok((Some(0), out))
                if out
                    .lines()
                    .any(|l| l.trim_start().starts_with("send ") || l.contains("|send|")) =>
            {
                Readiness::Native
            }
            Ok((status, _)) => Readiness::Unavailable {
                reason: format!("omo thread gateway unsupported (exit {status:?})"),
            },
            Err(error) => Readiness::Unavailable {
                reason: format!("omo thread gateway unavailable: {error:#}"),
            },
        })
    }

    fn bind(&self, ctx: &Ctx, recipient: &Recipient) -> Result<BindOutcome> {
        let target = target(ctx, recipient)?;
        let (status, out) =
            self.run(&["thread", "list", "--all-scope", "--json"], PROBE_TIMEOUT)?;
        ensure!(
            status == Some(0),
            "omo thread list failed (exit {status:?})"
        );
        let list: Value = serde_json::from_str(out.trim()).context("parsing omo thread list")?;
        let mut rows = candidates(&list, &target, Timestamp::now())?;
        if rows.len() > 1 {
            return Ok(BindOutcome::Ambiguous {
                candidates: rows
                    .iter()
                    .map(|r| r["durableSessionId"].as_str().unwrap_or("").to_string())
                    .collect(),
            });
        }
        let Some(row) = rows.pop() else {
            return Ok(BindOutcome::Unbound {
                reason: format!("no live OmO session for {}", target.cwd.display()),
            });
        };
        let socket = PathBuf::from(row["socket"].as_str().context("thread has no socket")?);
        let pane = target.pane.as_deref().context("recipient has no pane")?;
        if !crate::question_reply::pane_sockets(ctx, pane)?.contains(&socket) {
            return Ok(BindOutcome::Unbound {
                reason: format!("OmO session endpoint is not the foreground of pane {pane}"),
            });
        }
        Ok(BindOutcome::Bound(SessionBinding {
            session: row["durableSessionId"]
                .as_str()
                .context("thread has no durable id")?
                .into(),
            endpoint: Some(socket),
            transcript: row["sessionPath"].as_str().map(PathBuf::from),
        }))
    }

    fn send(&self, _ctx: &Ctx, binding: &SessionBinding, envelope: &Envelope) -> SendOutcome {
        let payload = match std::fs::read_to_string(&envelope.payload_path) {
            Ok(payload) if hash(&payload) == envelope.payload_sha256 => payload,
            Ok(_) | Err(_) => {
                return SendOutcome::Rejected {
                    reason: "frozen envelope payload unreadable or changed".into(),
                    retryable_after_fix: true,
                };
            }
        };
        let args = [
            "thread",
            "send",
            "--all-scope",
            "--mode",
            gateway_mode(&envelope.mode),
            "--idempotency-key",
            envelope.idempotency_key.as_str(),
            "--json",
            "--",
            binding.session.as_str(),
            payload.as_str(),
        ];
        match self.run(&args, SEND_TIMEOUT) {
            Ok((status, out)) => classify(status, &out),
            Err(error) => SendOutcome::Uncertain {
                reason: format!("omo thread send: {error:#}"),
            },
        }
    }

    fn observe(
        &self,
        ctx: &Ctx,
        binding: &SessionBinding,
        cursor: &mut ObserveCursor,
    ) -> Result<Vec<Observation>> {
        let path = binding
            .transcript
            .as_deref()
            .context("bound OmO session has no transcript")?;
        let lines = read_new_lines(path, cursor)?;
        let mut out = Vec::new();
        for line in lines {
            let Ok(entry) = serde_json::from_slice::<Value>(&line) else {
                continue;
            };
            if let Some(observation) = observed(ctx, binding, &entry)? {
                out.push(observation);
            }
        }
        Ok(out)
    }
}

/// Incremental, complete-line reader. A shrunken file restarts from its origin.
fn read_new_lines(path: &Path, cursor: &mut ObserveCursor) -> Result<Vec<Vec<u8>>> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            cursor.coverage = Coverage::Unreadable;
            return Ok(Vec::new());
        }
        Err(error) => return Err(error).with_context(|| format!("opening {}", path.display())),
    };
    let meta = file.metadata().context("inspecting OmO transcript")?;
    ensure!(meta.is_file(), "OmO transcript must be a regular file");
    if meta.len() < cursor.offset {
        *cursor = ObserveCursor::default();
        cursor.coverage = Coverage::Truncated;
    }
    file.seek(SeekFrom::Start(cursor.offset))
        .context("seeking OmO transcript")?;
    let mut bytes = Vec::new();
    file.take(MAX_TRANSCRIPT_READ)
        .read_to_end(&mut bytes)
        .context("reading OmO transcript")?;
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

/// The gateway refuses `steer` without the target's current turn epoch, which neither
/// `thread list` nor `thread read` reports (captured omo 5.1.35). `auto` starts an idle
/// session, queues behind a running turn and holds while the user drafts.
fn gateway_mode(envelope_mode: &str) -> &'static str {
    match envelope_mode {
        "follow_up" => "follow_up",
        _ => "auto",
    }
}

fn hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// The delivered text as the gateway recorded it: `display_text`, or the JSON-quoted
/// tail of `content` after the two gateway header lines.
fn delivered_texts(entry: &Value) -> Vec<String> {
    let mut texts = Vec::new();
    if let Some(text) = entry["details"]["display_text"].as_str() {
        texts.push(text.to_string());
    }
    if let Some(tail) = entry["content"]
        .as_str()
        .and_then(|c| c.splitn(3, '\n').nth(2))
        .and_then(|t| serde_json::from_str::<String>(t).ok())
    {
        texts.push(tail);
    }
    texts
}

fn observed(ctx: &Ctx, binding: &SessionBinding, entry: &Value) -> Result<Option<Observation>> {
    if entry["type"] != "custom_message"
        || entry["customType"] != "session_control_delivery"
        || entry["details"]["source"] != "session_control"
    {
        return Ok(None);
    }
    let Some(delivery_id) = entry["details"]["delivery_id"].as_str() else {
        return Ok(None);
    };
    for text in delivered_texts(entry) {
        let Some((key, number)) = header(&text) else {
            continue;
        };
        let Ok(recipient) = Recipient::parse_key(&key) else {
            continue;
        };
        let inbox = Inbox::new(&ctx.store, recipient)?;
        let Ok(envelope) = inbox.envelope(number) else {
            continue;
        };
        if envelope.adapter != HARNESS
            || &envelope.binding != binding
            || envelope.payload_sha256 != hash(&text)
        {
            continue;
        }
        // An admitted receipt pins the gateway's delivery id; a lost result does not.
        let mut admitted_ids = Vec::new();
        for seq in &envelope.members {
            if let Some(receipt) = inbox.receipt(*seq, ReceiptLevel::Admitted)?
                && let Some(id) = receipt.detail["delivery_id"].as_str()
            {
                admitted_ids.push(id.to_string());
            }
        }
        if admitted_ids.iter().any(|id| id != delivery_id) {
            continue;
        }
        return Ok(Some(Observation::Delivered {
            envelope: number,
            members: envelope.members.clone(),
            detail: json!({
                "delivery_id": delivery_id,
                "entry_id": entry["id"],
                "timestamp": entry["timestamp"],
                "deliver_as": entry["details"]["deliverAs"],
            }),
        }));
    }
    Ok(None)
}

fn header(text: &str) -> Option<(String, u64)> {
    let rest = text.lines().next()?.strip_prefix("[corral envelope ")?;
    let (key, number) = rest.strip_suffix(']')?.rsplit_once("@env")?;
    Some((key.to_string(), number.parse().ok()?))
}

#[cfg(test)]
#[path = "omo_tests.rs"]
mod tests;
