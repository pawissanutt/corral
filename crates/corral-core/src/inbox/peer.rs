//! Agent-authored information has its own generation budget, independent of owner headers
//! and receipt retention. Publication, its replay key and the budget debit commit together.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{Inbox, Message, Post, Recipient};
use crate::agent::paths::sha256_hex;

pub const AGENT_MESSAGES_PER_GENERATION: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentPostRefusal {
    Limit,
    PayloadMismatch,
    RecipientClosed,
}

impl std::fmt::Display for AgentPostRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Limit => "agent_message_limit: at most 60 messages per generation",
            Self::PayloadMismatch => "payload_mismatch: peer request was already used",
            Self::RecipientClosed => "recipient_closed: recipient no longer accepts input",
        })
    }
}
impl std::error::Error for AgentPostRefusal {}

#[derive(Serialize, Deserialize)]
struct Publication {
    payload_sha256: String,
    recipient: Recipient,
    seq: u64,
}

impl Inbox<'_> {
    /// The caller authenticates `post.author` from its bound container, not from model text.
    /// Replays neither reserve another sequence nor consume another budget slot.
    pub fn post_agent(
        &self,
        post: Post<'_>,
        generation: u32,
        request_id: &str,
        payload_sha256: &str,
    ) -> Result<Message> {
        ensure!(post.kind == "agent_message", "expected agent_message");
        self.check(&post)?;
        let sender = sha256_hex(format!("{}/{generation}", post.author).as_bytes());
        let request = sha256_hex(format!("{sender}/{request_id}").as_bytes());
        // Sender before recipient: concurrent sends share the quota even across recipients.
        let _sender = self.store.lock(&format!("peer-send-{sender}"))?;
        if let Some(previous) = self
            .store
            .record::<Publication>("peer-publications", &request)?
        {
            if previous.payload_sha256 != payload_sha256 || previous.recipient != self.recipient {
                return Err(AgentPostRefusal::PayloadMismatch.into());
            }
            return self.message(previous.seq);
        }
        let used = self
            .store
            .record::<u64>("peer-send-budgets", &sender)?
            .unwrap_or(0);
        if used >= AGENT_MESSAGES_PER_GENERATION {
            return Err(AgentPostRefusal::Limit.into());
        }
        let _posting = self.lock()?;
        let retain = |message: &Message| -> Result<()> {
            self.store
                .save_record("peer-send-budgets", &sender, &(used + 1))?;
            self.store.save_record(
                "peer-publications",
                &request,
                &Publication {
                    payload_sha256: payload_sha256.to_owned(),
                    recipient: self.recipient.clone(),
                    seq: message.seq,
                },
            )?;
            if self.recipient.task().is_none() {
                self.store
                    .save_record("peer-standalone", self.key(), &self.recipient)?;
            }
            self.announce(message)
        };
        match &self.recipient {
            Recipient::Pm | Recipient::Assistant { .. } => self.store.records_transaction(|| {
                if let Recipient::Assistant { session } = &self.recipient
                    && self.store.get_assistant(session)?.ended.is_some()
                {
                    return Err(AgentPostRefusal::RecipientClosed.into());
                }
                let message = self.update_state(|state| self.build(state, post))?;
                self.write_message(&message)?;
                retain(&message)?;
                Ok(message)
            }),
            _ => {
                let mut posted = None;
                self.store.update_head(
                    self.recipient.task().context("task recipient")?,
                    |head| {
                        if self.recipient_ended_ref(head)? {
                            return Err(AgentPostRefusal::RecipientClosed.into());
                        }
                        let message = self.reserve_in_head(head, post)?;
                        retain(&message)?;
                        posted = Some(message);
                        Ok(())
                    },
                )?;
                posted.context("agent publication produced no message")
            }
        }
    }
}

#[cfg(test)]
#[path = "peer_tests.rs"]
mod tests;
