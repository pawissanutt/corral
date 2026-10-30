//! OmO question lifecycle, independent of tool-result acknowledgement and terminal activity.
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use corral_core::Run;
use corral_core::file_stamp::{FileStamp, SETTLE_WINDOW};
use serde_json::Value;

use crate::Ctx;

const MAX_TRANSCRIPT_BYTES: u64 = 32 * 1024 * 1024;

/// Pending questions by id, and settled request ids.
type Lifecycle = (BTreeMap<String, String>, BTreeSet<String>);

/// Finished runs keep their transcripts, so every board refresh would otherwise re-read each
/// one. A settled stamp stands in for unchanged bytes (see `corral_core::file_stamp`).
static LIFECYCLES: Mutex<BTreeMap<PathBuf, (FileStamp, Arc<Lifecycle>)>> =
    Mutex::new(BTreeMap::new());
static SETTLE_NANOS: AtomicU64 = AtomicU64::new(SETTLE_WINDOW.as_nanos() as u64);

/// Benchmark fixtures cannot age a transcript's ctime; this process-wide override shortens the
/// window after which an unchanged transcript is trusted. Never set outside fixtures.
#[doc(hidden)]
pub fn set_transcript_settle_window(window: Duration) {
    SETTLE_NANOS.store(window.as_nanos() as u64, Ordering::Relaxed);
}

#[derive(Debug, Clone)]
pub(crate) struct HarnessQuestion {
    pub source: String,
    pub question: String,
}

pub(crate) fn pending(ctx: &Ctx, run: &Run) -> Result<Option<HarnessQuestion>> {
    if !crate::omo::is_omo(run) {
        return Ok(None);
    }
    pending_at(ctx, &run.worktree)
}

pub(crate) fn pending_at(ctx: &Ctx, cwd: &Path) -> Result<Option<HarnessQuestion>> {
    let agent = crate::omo::agent_dir(ctx.store.config.sync.omo_agent_dir.as_deref());
    let directory = agent.join("sessions").join(crate::omo::encode_cwd(cwd));
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", directory.display()));
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("listing {}", directory.display()))?;
        if entry
            .file_type()
            .context("inspecting harness transcript")?
            .is_file()
            && entry.path().extension().is_some_and(|ext| ext == "jsonl")
        {
            paths.push(entry.path());
        }
    }
    paths.sort();
    for path in paths {
        if let Some(question) = read_question(&path)? {
            return Ok(Some(question));
        }
    }
    Ok(None)
}

fn read_question(path: &Path) -> Result<Option<HarnessQuestion>> {
    Ok(read_lifecycle(path)?
        .0
        .iter()
        .next()
        .map(|(id, question)| HarnessQuestion {
            source: format!("{} request {id}", path.display()),
            question: question.clone(),
        }))
}

pub(crate) fn source_resolved(source: &str) -> Result<bool> {
    let Some((path, id)) = source.rsplit_once(" request ") else {
        return Ok(false);
    };
    if !Path::new(path)
        .try_exists()
        .context("checking native decision source")?
    {
        return Ok(false);
    }
    Ok(read_lifecycle(Path::new(path))?.1.contains(id))
}

fn read_lifecycle(path: &Path) -> Result<Arc<Lifecycle>> {
    let observed = SystemTime::now();
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let metadata = file.metadata().context("inspecting harness transcript")?;
    if !metadata.is_file() {
        bail!("harness transcript must be a regular file");
    }
    if metadata.len() > MAX_TRANSCRIPT_BYTES {
        bail!(
            "harness transcript exceeds {MAX_TRANSCRIPT_BYTES} bytes: {}",
            path.display()
        );
    }
    let cache = |lifecycles: &BTreeMap<PathBuf, (FileStamp, Arc<Lifecycle>)>| {
        lifecycles
            .get(path)
            .filter(|(stamp, _)| FileStamp::of(&metadata) == Some(*stamp))
            .map(|(_, lifecycle)| Arc::clone(lifecycle))
    };
    let lifecycles = LIFECYCLES
        .lock()
        .map_err(|e| anyhow::anyhow!("locking transcript cache: {e}"))?;
    if let Some(lifecycle) = cache(&lifecycles) {
        return Ok(lifecycle);
    }
    drop(lifecycles);
    let lifecycle = Arc::new(parse_lifecycle(path, file)?);
    let window = Duration::from_nanos(SETTLE_NANOS.load(Ordering::Relaxed));
    if let Some(stamp) = FileStamp::settled(&metadata, observed, window) {
        LIFECYCLES
            .lock()
            .map_err(|e| anyhow::anyhow!("locking transcript cache: {e}"))?
            .insert(path.to_path_buf(), (stamp, Arc::clone(&lifecycle)));
    }
    Ok(lifecycle)
}

const ANSWER_FRAME: &str = "[Answer to question ";

/// Tool-result statuses that end a native question; `pending` is only an acknowledgement.
const TERMINAL_RESULTS: [&str; 6] = [
    "answered",
    "comment-submitted",
    "timed_out",
    "cancelled",
    "orphaned-after-restart",
    "unavailable",
];

/// The request id of a question tool's terminal result, such as a local UI answer.
fn finished_request(message: &Value) -> Option<&str> {
    if message["role"] != "toolResult"
        || !matches!(
            message["toolName"].as_str(),
            Some("ask_user_question" | "request_user_input")
        )
        || !TERMINAL_RESULTS.contains(&message["details"]["status"].as_str()?)
    {
        return None;
    }
    message["toolCallId"].as_str()
}

/// The request id of a user message framed as `[Answer to question <id>]`.
fn answered_request(content: &Value) -> Option<&str> {
    let text = match content {
        Value::String(text) => text.as_str(),
        other => other.as_array()?.first()?["text"].as_str()?,
    };
    text.strip_prefix(ANSWER_FRAME)?
        .split_once(']')
        .map(|(id, _)| id)
}

fn parse_lifecycle(path: &Path, file: std::fs::File) -> Result<Lifecycle> {
    corral_core::work::record(corral_core::work::Work::TranscriptRead, 1);
    let mut bytes = Vec::new();
    file.take(MAX_TRANSCRIPT_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {}", path.display()))?;
    if u64::try_from(bytes.len())? > MAX_TRANSCRIPT_BYTES {
        bail!(
            "harness transcript exceeds {MAX_TRANSCRIPT_BYTES} bytes: {}",
            path.display()
        );
    }
    let mut calls = BTreeMap::new();
    let mut questions = BTreeMap::new();
    let mut settled = BTreeSet::new();
    // Only question calls and ask-user records matter; JSON never escapes these ASCII markers,
    // so every other line (nearly all of a long transcript) is skipped without parsing.
    let call = memchr::memmem::Finder::new("\"request_user_input\"");
    let ask = memchr::memmem::Finder::new("\"ask-user:");
    let answer = memchr::memmem::Finder::new(ANSWER_FRAME);
    let ask_tool = memchr::memmem::Finder::new("\"ask_user_question\"");
    // A trailing partial line is a writer in flight, not a completed lifecycle entry.
    for line in bytes
        .split_inclusive(|byte| *byte == b'\n')
        .filter(|line| line.ends_with(b"\n"))
        .filter(|line| {
            call.find(line).is_some()
                || ask.find(line).is_some()
                || answer.find(line).is_some()
                || ask_tool.find(line).is_some()
        })
    {
        let value: Value =
            serde_json::from_slice(line).with_context(|| format!("parsing {}", path.display()))?;
        if let Some(blocks) = value["message"]["content"].as_array() {
            for block in blocks {
                if block["type"] == "toolCall"
                    && block["name"] == "request_user_input"
                    && let Some(id) = block["id"].as_str()
                {
                    calls.insert(id.to_string(), block["arguments"]["questions"].to_string());
                }
            }
        }
        // An answer delivered as a user message settles its request even when the transcript
        // records no settlement (or records a timeout first).
        if value["message"]["role"] == "user"
            && let Some(id) = answered_request(&value["message"]["content"])
        {
            questions.remove(id);
            settled.insert(id.to_string());
        }
        // A blocking question resolved in the harness (for example its local UI) records only
        // its terminal tool result for that exact call, with no async settlement record.
        if let Some(id) = finished_request(&value["message"]) {
            questions.remove(id);
            settled.insert(id.to_string());
        }
        if value["type"] == "custom" {
            let Some(id) = value["data"]["requestId"].as_str() else {
                continue;
            };
            match value["customType"].as_str() {
                Some("ask-user:question") => {
                    settled.remove(id);
                    questions.insert(
                        id.to_string(),
                        calls
                            .get(id)
                            .cloned()
                            .unwrap_or_else(|| value["data"]["headers"].to_string()),
                    );
                }
                Some("ask-user:settlement") => {
                    // Any settlement (answered, timed_out, cancelled, ...) ends the request: a
                    // question nobody can still answer must not block the task. Tool-result
                    // acknowledgements are not settlements and never reach this arm.
                    questions.remove(id);
                    settled.insert(id.to_string());
                }
                _ => {}
            }
        }
    }
    Ok((questions, settled))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript(settled: bool) -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("question.jsonl");
        let mut lines = vec![
            serde_json::json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","id":"full|request-id","name":"request_user_input","arguments":{"questions":[{"question":"A or B?","options":["A","B"]}]}}]}}),
            serde_json::json!({"type":"custom","customType":"ask-user:question","data":{"requestId":"full|request-id","headers":["Choice"]}}),
            serde_json::json!({"type":"message","message":{"role":"toolResult","toolCallId":"full|request-id","content":[{"text":"Question accepted"}]}}),
        ];
        if settled {
            lines.push(serde_json::json!({"type":"custom","customType":"ask-user:settlement","data":{"requestId":"full|request-id","status":"comment-submitted"}}));
        }
        std::fs::write(
            &path,
            lines.iter().map(|v| format!("{v}\n")).collect::<String>(),
        )
        .unwrap();
        (temp, path)
    }

    #[test]
    fn acknowledgement_does_not_settle_native_question() {
        // Given the real native lifecycle shape, including an acknowledged tool call.
        let (_temp, path) = transcript(false);
        // When inspecting the pending state.
        let question = read_question(&path).unwrap();
        // Then the request remains pending.
        assert!(question.is_some());
    }

    #[test]
    fn matching_native_settlement_clears_question() {
        // Given the same request with its terminal settlement.
        let (_temp, path) = transcript(true);
        // When inspecting the pending state.
        let question = read_question(&path).unwrap();
        // Then the acknowledgement is not confused with an unanswered question.
        assert!(question.is_none());
    }

    #[test]
    fn an_appended_settlement_clears_an_already_read_question() {
        use std::io::Write;
        // Given a pending question that was already read, among unrelated transcript lines.
        let (_temp, path) = transcript(false);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "{}", serde_json::json!({"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"working"}]}})).unwrap();
        assert!(read_question(&path).unwrap().is_some());
        // When the owner's answer is appended.
        writeln!(
            file,
            "{}",
            serde_json::json!({"type":"custom","customType":"ask-user:settlement","data":{"requestId":"full|request-id","status":"answered"}})
        )
        .unwrap();
        // Then the next read sees the settlement instead of the earlier result.
        assert!(read_question(&path).unwrap().is_none());
    }

    fn append(path: &std::path::Path, lines: &[serde_json::Value]) {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    #[test]
    fn settlements_and_answers_of_other_requests_do_not_resolve_the_request() {
        // Given an acknowledged request, then settlements and an answer for other requests.
        let (_temp, path) = transcript(false);
        append(
            &path,
            &[
                serde_json::json!({"type":"custom","customType":"ask-user:settlement","data":{"requestId":"other|request-id","status":"comment-submitted"}}),
                serde_json::json!({"type":"message","message":{"role":"user","content":[{"type":"text","text":"[Answer to question other|request-id]\nA"}]}}),
            ],
        );
        // When replayed after a restart.
        let pending = read_question(&path).unwrap().unwrap();
        // Then only the exact request can settle it.
        assert!(pending.source.ends_with(" request full|request-id"));
        assert!(!source_resolved(&pending.source).unwrap());
    }

    #[test]
    fn t207_timed_out_then_user_answer_is_not_pending() {
        // Given the T-207 shape: accepted as pending, then a timed_out settlement.
        let (_temp, path) = transcript(false);
        append(
            &path,
            &[
                serde_json::json!({"type":"custom","customType":"ask-user:settlement","data":{"requestId":"full|request-id","status":"timed_out"}}),
            ],
        );
        // Then the timed-out question is already settled.
        assert!(read_question(&path).unwrap().is_none());
        assert!(source_resolved(&format!("{} request full|request-id", path.display())).unwrap());
    }

    #[test]
    fn user_answer_message_alone_settles_the_question() {
        // Given a pending question and a later answer message without any settlement record.
        let (_temp, path) = transcript(false);
        assert!(read_question(&path).unwrap().is_some());
        append(
            &path,
            &[
                serde_json::json!({"type":"message","message":{"role":"user","content":[{"type":"text","text":"[Answer to question full|request-id]\nB"},{"type":"text","text":"[The user's comment for question full|request-id]"},{"type":"text","text":"B"}]}}),
            ],
        );
        // Then the answered question no longer counts as pending.
        assert!(read_question(&path).unwrap().is_none());
    }

    /// The T-389 shape: a blocking `ask_user_question` call, its marker, and later a result.
    fn ask_tool_transcript(result: Option<serde_json::Value>) -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("ask.jsonl");
        let mut lines = vec![
            serde_json::json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","id":"toolu_ask","name":"ask_user_question","arguments":{"questions":[{"header":"Write scope","question":"Which?","options":[{"label":"A"},{"label":"B"}]}],"waitForAnswer":true}}]}}),
            serde_json::json!({"type":"custom","customType":"ask-user:question","data":{"requestId":"toolu_ask","headers":["Write scope"]}}),
        ];
        lines.extend(result);
        std::fs::write(
            &path,
            lines.iter().map(|v| format!("{v}\n")).collect::<String>(),
        )
        .unwrap();
        (temp, path)
    }

    fn ask_tool_result(id: &str, status: &str) -> serde_json::Value {
        serde_json::json!({"type":"message","message":{"role":"toolResult","toolCallId":id,"toolName":"ask_user_question","details":{"resolvedBy":"local_ui","answers":{"Which?":"A"},"unanswered":[],"status":status},"content":[{"type":"text","text":"Write scope: A"}],"isError":false}})
    }

    #[test]
    fn local_ui_answered_tool_result_settles_the_question() {
        // Given an ask_user_question answered in the harness's local UI.
        let (_temp, path) = ask_tool_transcript(Some(ask_tool_result("toolu_ask", "answered")));
        // Then nothing is pending and a saved decision for it is resolved.
        assert!(read_question(&path).unwrap().is_none());
        assert!(source_resolved(&format!("{} request toolu_ask", path.display())).unwrap());
    }

    #[test]
    fn unanswered_or_foreign_tool_results_keep_the_question_pending() {
        for result in [
            None,
            Some(ask_tool_result("toolu_other", "answered")),
            Some(ask_tool_result("toolu_ask", "pending")),
        ] {
            // Given a fresh question, an answer to another request, or a mere acknowledgement.
            let (_temp, path) = ask_tool_transcript(result);
            // Then the exact request still blocks.
            let pending = read_question(&path).unwrap().unwrap();
            assert!(pending.source.ends_with(" request toolu_ask"));
            assert!(!source_resolved(&pending.source).unwrap());
        }
    }

    #[test]
    fn t207_full_shape_pending_then_timeout_then_user_answer() {
        // Given accepted, then timed_out, then an answer arriving as a user message.
        let (_temp, path) = transcript(false);
        append(
            &path,
            &[
                serde_json::json!({"type":"custom","customType":"ask-user:settlement","data":{"requestId":"full|request-id","status":"timed_out"}}),
                serde_json::json!({"type":"message","message":{"role":"user","content":[{"type":"text","text":"[Answer to question full|request-id]\nA"}]}}),
            ],
        );
        // Then nothing is pending.
        assert!(read_question(&path).unwrap().is_none());
    }
}
