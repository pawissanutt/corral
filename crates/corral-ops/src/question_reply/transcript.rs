use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::Question;

const MAX_BYTES: u64 = 32 * 1024 * 1024;

pub(super) fn read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    ensure!(
        file.metadata()
            .context("inspecting native metadata")?
            .is_file(),
        "native metadata must be a regular file"
    );
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .context("reading native metadata")?;
    ensure!(
        u64::try_from(bytes.len())? <= limit,
        "native metadata exceeds size limit"
    );
    Ok(bytes)
}

pub(super) struct Snapshot {
    pub session_id: String,
    pub cwd: std::path::PathBuf,
    pub pending: BTreeMap<String, String>,
    identities: BTreeMap<String, String>,
    settlements: BTreeMap<String, String>,
    messages: Vec<Value>,
}

impl Snapshot {
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = read(path, MAX_BYTES)?;
        let mut snapshot = Self {
            session_id: String::new(),
            cwd: Default::default(),
            pending: BTreeMap::new(),
            identities: BTreeMap::new(),
            settlements: BTreeMap::new(),
            messages: Vec::new(),
        };
        for line in bytes
            .split_inclusive(|byte| *byte == b'\n')
            .filter(|line| line.ends_with(b"\n"))
        {
            let value: Value = serde_json::from_slice(line).context("parsing native transcript")?;
            match value["type"].as_str() {
                Some("session") => {
                    snapshot.session_id = value["id"].as_str().context("session has no id")?.into();
                    snapshot.cwd = value["cwd"].as_str().context("session has no cwd")?.into();
                }
                Some("message") => {
                    let message = &value["message"];
                    if let Some(blocks) = message["content"].as_array() {
                        for block in blocks {
                            if block["type"] == "toolCall"
                                && matches!(
                                    block["name"].as_str(),
                                    Some("request_user_input" | "ask_user_question")
                                )
                            {
                                let id = block["id"].as_str().context("question call has no id")?;
                                let identity = format!(
                                    "{:x}",
                                    Sha256::digest(serde_json::to_vec(&block["arguments"])?)
                                );
                                snapshot.identities.insert(id.to_string(), identity);
                            }
                        }
                    }
                    if message["role"] == "toolResult"
                        && let Some(id) = message["toolCallId"].as_str()
                        && snapshot.identities.contains_key(id)
                        && matches!(
                            message["details"]["status"].as_str(),
                            Some(
                                "answered"
                                    | "comment-submitted"
                                    | "timed_out"
                                    | "cancelled"
                                    | "orphaned-after-restart"
                                    | "unavailable"
                            )
                        )
                    {
                        snapshot.pending.remove(id);
                    }
                    snapshot.messages.push(message.clone());
                }
                Some("custom") => {
                    if let Some(id) = value["data"]["requestId"].as_str() {
                        match value["customType"].as_str() {
                            Some("ask-user:question") => {
                                let identity = snapshot.identities.get(id).context(
                                    "native question has no complete question/option identity",
                                )?;
                                snapshot.pending.insert(id.into(), identity.clone());
                            }
                            Some("ask-user:settlement") => {
                                snapshot.pending.remove(id);
                                snapshot.settlements.insert(
                                    id.into(),
                                    value["data"]["status"].as_str().unwrap_or("").into(),
                                );
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        ensure!(
            !snapshot.session_id.is_empty(),
            "native transcript has no session header"
        );
        Ok(snapshot)
    }

    pub fn check_session(&self, question: &Question) -> Result<()> {
        ensure!(
            self.session_id == question.session_id
                && self.identities.get(&question.request) == Some(&question.identity),
            "native session or question identity was replaced"
        );
        Ok(())
    }

    pub fn answered(&self, question: &Question, text: &str) -> bool {
        if self.messages.iter().any(|message| {
            message["role"] == "toolResult"
                && message["toolCallId"] == question.request
                && message["details"]["status"] == "comment-submitted"
                && message["details"]["resolvedBy"] == "control_endpoint"
                && message["details"]["comment"] == text
        }) {
            return true;
        }
        // The native endpoint submits corral text as one comment. The durable
        // lifecycle status alone is insufficient: someone else may have answered.
        self.settlements
            .get(&question.request)
            .is_some_and(|status| status == "comment-submitted")
            && self.messages.iter().any(|message| {
                if message["role"] != "user" {
                    return false;
                }
                let Some(blocks) = message["content"].as_array() else {
                    return false;
                };
                blocks
                    .first()
                    .and_then(|block| block["text"].as_str())
                    .is_some_and(|frame| {
                        frame.starts_with(&format!("[Answer to question {}]\n", question.request))
                    })
                    && blocks.windows(2).any(|pair| {
                        pair[0]["text"]
                            == format!("[The user's comment for question {}]", question.request)
                            && pair[1]["text"] == text.trim()
                    })
            })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::Snapshot;

    fn snapshot(result_for: &str, status: &str) -> Snapshot {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("session.jsonl");
        let lines = [
            json!({"type":"session","id":"session","cwd":"/app"}),
            json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","id":"toolu_ask","name":"ask_user_question","arguments":{"questions":[{"header":"Scope","question":"Which?","options":[{"label":"A"}]}]}}]}}),
            json!({"type":"custom","customType":"ask-user:question","data":{"requestId":"toolu_ask","headers":["Scope"]}}),
            json!({"type":"message","message":{"role":"toolResult","toolCallId":result_for,"toolName":"ask_user_question","details":{"resolvedBy":"local_ui","status":status},"content":[{"type":"text","text":"Scope: A"}]}}),
        ];
        std::fs::write(
            &path,
            lines.iter().map(|l| format!("{l}\n")).collect::<String>(),
        )
        .unwrap();
        Snapshot::read(&path).unwrap()
    }

    #[test]
    fn local_ui_answered_request_is_not_an_active_reply_endpoint() {
        // Given a question answered in the local UI, an acknowledgement, and another answer.
        // Then only the answered request stops being offered for a native reply.
        assert!(snapshot("toolu_ask", "answered").pending.is_empty());
        assert!(
            snapshot("toolu_ask", "pending")
                .pending
                .contains_key("toolu_ask")
        );
        assert!(
            snapshot("toolu_other", "answered")
                .pending
                .contains_key("toolu_ask")
        );
    }
}
