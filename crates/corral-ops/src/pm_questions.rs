//! OmO question lifecycle, independent of tool-result acknowledgement and terminal activity.
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

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
    if !crate::omo::is_omo(run) || run.outcome.is_some() {
        return Ok(None);
    }
    let agent = crate::omo::agent_dir(ctx.store.config.sync.omo_agent_dir.as_deref());
    let directory = agent
        .join("sessions")
        .join(crate::omo::encode_cwd(&run.worktree));
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
    let Some(path) = paths.last() else {
        return Ok(None);
    };
    read_question(path)
}

fn read_question(path: &Path) -> Result<Option<HarnessQuestion>> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    if !file
        .metadata()
        .context("inspecting harness transcript")?
        .is_file()
    {
        bail!("harness transcript must be a regular file");
    }
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
    // A trailing partial line is a writer in flight, not a completed lifecycle entry.
    for line in bytes
        .split_inclusive(|byte| *byte == b'\n')
        .filter(|line| line.ends_with(b"\n"))
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
                    questions.insert(
                        id.to_string(),
                        calls
                            .get(id)
                            .cloned()
                            .unwrap_or_else(|| value["data"]["headers"].to_string()),
                    );
                }
                Some("ask-user:settlement") => {
                    questions.remove(id);
                }
                _ => {}
            }
        }
    }
    Ok(questions
        .into_iter()
        .next()
        .map(|(id, question)| HarnessQuestion {
            source: format!("{} request {id}", path.display()),
            question,
        }))
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
}
