//! OmO question lifecycle, independent of tool-result acknowledgement and terminal activity.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use corral_core::Run;
use serde_json::Value;

use crate::Ctx;

const MAX_TRANSCRIPT_BYTES: u64 = 32 * 1024 * 1024;

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
        .into_iter()
        .next()
        .map(|(id, question)| HarnessQuestion {
            source: format!("{} request {id}", path.display()),
            question,
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

type Lifecycle = (BTreeMap<String, String>, BTreeSet<String>);

/// A transcript is append-only, so an unchanged size, mtime and inode means unchanged content.
#[derive(Clone, PartialEq, Eq)]
struct Seen {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    inode: u64,
}

/// Parsed lifecycles by transcript, so a long-running board re-reads a transcript only after it grows.
fn parsed() -> &'static Mutex<HashMap<PathBuf, (Seen, Lifecycle)>> {
    static PARSED: OnceLock<Mutex<HashMap<PathBuf, (Seen, Lifecycle)>>> = OnceLock::new();
    PARSED.get_or_init(Mutex::default)
}

fn read_lifecycle(path: &Path) -> Result<Lifecycle> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let metadata = file.metadata().context("inspecting harness transcript")?;
    if !metadata.is_file() {
        bail!("harness transcript must be a regular file");
    }
    let seen = Seen {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        #[cfg(unix)]
        inode: std::os::unix::fs::MetadataExt::ino(&metadata),
    };
    let cache = parsed()
        .lock()
        .map_err(|e| anyhow::anyhow!("locking transcript cache: {e}"))?
        .get(path)
        .filter(|(known, _)| *known == seen)
        .map(|(_, lifecycle)| lifecycle.clone());
    if let Some(lifecycle) = cache {
        return Ok(lifecycle);
    }
    let lifecycle = parse_lifecycle(file, path)?;
    parsed()
        .lock()
        .map_err(|e| anyhow::anyhow!("locking transcript cache: {e}"))?
        .insert(path.to_path_buf(), (seen, lifecycle.clone()));
    Ok(lifecycle)
}

fn parse_lifecycle(file: std::fs::File, path: &Path) -> Result<Lifecycle> {
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
    // A trailing partial line is a writer in flight, not a completed lifecycle entry.
    for line in bytes
        .split_inclusive(|byte| *byte == b'\n')
        .filter(|line| line.ends_with(b"\n"))
        .filter(|line| call.find(line).is_some() || ask.find(line).is_some())
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
                    // Timeout, dismissal and transport acknowledgement are not an answer.
                    if matches!(
                        value["data"]["status"].as_str(),
                        Some("comment-submitted" | "answered")
                    ) {
                        questions.remove(id);
                        settled.insert(id.to_string());
                    }
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

    #[test]
    fn timeout_and_unrelated_answers_do_not_resolve_the_request() {
        use std::io::Write;
        // Given an acknowledged request with no matching owner answer.
        let (_temp, path) = transcript(false);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        for (id, status) in [
            ("full|request-id", "timeout"),
            ("other|request-id", "comment-submitted"),
        ] {
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "type":"custom","customType":"ask-user:settlement",
                    "data":{"requestId":id,"status":status}
                })
            )
            .unwrap();
        }
        // When replayed after a restart.
        let pending = read_question(&path).unwrap().unwrap();
        // Then only the exact answered request can settle it.
        assert!(pending.source.ends_with(" request full|request-id"));
        assert!(!source_resolved(&pending.source).unwrap());
    }
}
