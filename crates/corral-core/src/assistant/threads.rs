//! Durable app conversations. Each message and idempotency key is a separate Store record;
//! thread heads contain only scalar counters and a bounded unfinished-message index.
use super::attachments;
use crate::Store;
use crate::blob_ref::BlobRef;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const HEADS: &str = "app-thread-heads";
const MESSAGES: &str = "app-thread-messages";
const KEYS: &str = "app-thread-send-keys";
const QUICK: &str = "app-thread-quick";
pub const PENDING_MAX: usize = 256;
pub const TEXT_MAX: usize = 16 * 1024;
pub const REPLY_MAX: usize = 256 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub session: String,
    pub quick: bool,
    pub next: u64,
    pub rev: u64,
    pub pending: Vec<u64>,
    pub state: String,
    pub failure: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextRef {
    pub kind: String,
    pub id: String,
    pub rev: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub media_type: Option<String>,
    pub bytes: u64,
    pub sha256: Option<String>,
    pub blob: Option<BlobRef>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub session: String,
    pub seq: u64,
    pub cmd_id: String,
    pub device: String,
    pub name: String,
    pub target: String,
    pub text: String,
    pub typed_at: String,
    pub at: String,
    pub state: String,
    pub input: Option<String>,
    pub pm_item: Option<String>,
    pub saved_task: Option<String>,
    pub reply: Option<String>,
    pub failure: Option<String>,
    pub turn: Option<u64>,
    #[serde(default)]
    pub generation: Option<u32>,
    #[serde(default)]
    pub late: bool,
    #[serde(default)]
    pub tainted: bool,
    pub model: Option<String>,
    pub context: Vec<ContextRef>,
    pub rev: u64,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

#[derive(Serialize, Deserialize)]
struct SendKey {
    entry: String,
    digest: String,
}

fn entry(session: &str, seq: u64) -> String {
    format!("{session}/M{seq}")
}

impl Store {
    /// Resolve the shared quick conversation, creating its session without model/provider work.
    pub fn app_thread(&self, scope: &str) -> Result<Thread> {
        let _lock = self.lock("app-threads")?;
        let session = if scope == "quick" {
            match self.record::<String>(QUICK, "shared")? {
                Some(id) if self.get_assistant(&id)?.ended.is_none() => id,
                _ => {
                    let profile = self
                        .config
                        .assistant
                        .agent
                        .as_deref()
                        .unwrap_or(&self.config.pm.agent);
                    let session = self.create_assistant(profile, None, None, None)?;
                    self.save_record(QUICK, "shared", &session.id)?;
                    session.id
                }
            }
        } else {
            super::validate_id(scope)?;
            self.get_assistant(scope)?;
            scope.to_owned()
        };
        if let Some(head) = self.record(HEADS, &session)? {
            return Ok(head);
        }
        let head = Thread {
            session,
            quick: scope == "quick",
            next: 1,
            rev: 1,
            pending: Vec::new(),
            state: "cold".into(),
            failure: None,
        };
        self.save_record(HEADS, &head.session, &head)?;
        Ok(head)
    }

    /// Read-only scope resolution; absent quick conversations are not invented by a reader.
    pub fn app_thread_scope(&self, scope: &str) -> Result<Option<String>> {
        if scope == "quick" {
            return self.record(QUICK, "shared");
        }
        super::validate_id(scope)?;
        Ok(self
            .record::<Thread>(HEADS, scope)?
            .map(|head| head.session))
    }

    pub fn app_thread_head(&self, session: &str) -> Result<Thread> {
        super::validate_id(session)?;
        self.record(HEADS, session)?.context("unknown app thread")
    }

    pub fn app_message(&self, id: &str) -> Result<Message> {
        self.record(MESSAGES, id)?.context("unknown app message")
    }

    /// Key and message publish atomically. Reusing a key for different bytes is never a resend.
    pub fn append_app_message(
        &self,
        mut message: Message,
        digest: &str,
        uploads: &[(String, String)],
    ) -> Result<(Message, bool)> {
        let _lock = self.lock("app-threads")?;
        ensure!(
            attachments::key(&message.device) && attachments::key(&message.cmd_id),
            "invalid send identity"
        );
        let key = format!("{}/{}", message.device, message.cmd_id);
        if let Some(stored) = self.record::<SendKey>(KEYS, &key)? {
            ensure!(
                stored.digest == digest,
                "payload_mismatch: send key already used"
            );
            return Ok((self.app_message(&stored.entry)?, true));
        }
        ensure!(
            !message.text.trim().is_empty() && message.text.len() <= TEXT_MAX,
            "message must contain 1..16384 bytes"
        );
        ensure!(
            message.context.len() <= 4,
            "at most four context references"
        );
        // Only inline links and unresolved upload placeholders enter from callers.
        for attachment in &message.attachments {
            if attachment.kind == "upload" {
                continue;
            }
            ensure!(
                attachment.kind == "link"
                    && attachment.blob.is_none()
                    && attachment.sha256.is_none()
                    && attachment.media_type.is_none()
                    && attachment.bytes == 0
                    && attachment.name.len() <= 160
                    && !attachment.name.chars().any(char::is_control),
                "invalid_params: invalid attachment"
            );
            let url = attachment
                .url
                .as_deref()
                .context("invalid_params: missing link URL")?;
            let authority = url
                .strip_prefix("https://")
                .or_else(|| url.strip_prefix("http://"))
                .context("invalid_params: invalid link scheme")?
                .split(['/', '?', '#'])
                .next()
                .unwrap_or_default();
            ensure!(
                url.len() <= 2048
                    && !url.chars().any(|c| c.is_control() || c.is_whitespace())
                    && !url.contains('\\')
                    && !authority.is_empty()
                    && !authority.contains('@'),
                "invalid_params: invalid link URL"
            );
        }
        let mut head = self.app_thread_head(&message.session)?;
        ensure!(
            self.get_assistant(&message.session)?.ended.is_none(),
            "assistant_ended"
        );
        ensure!(head.pending.len() < PENDING_MAX, "thread queue is full");
        message.seq = head.next;
        message.id = entry(&head.session, head.next);
        head.next = head
            .next
            .checked_add(1)
            .context("thread sequence exhausted")?;
        head.rev += 1;
        head.pending.push(message.seq);
        if message.target == "assistant" && head.state == "paused" {
            head.state = "cold".into();
            head.failure = None;
        }
        let _attachments = if uploads.is_empty() {
            None
        } else {
            Some(self.lock("app-attachments")?)
        };
        self.records_transaction(|| {
            let mut taken = self.take_app_uploads(&message.device, uploads)?;
            let mut claims: HashMap<_, _> = uploads
                .iter()
                .map(|(id, _)| id.as_str())
                .zip(&mut taken)
                .collect();
            for attachment in &mut message.attachments {
                if attachment.kind == "upload" {
                    *attachment = claims
                        .remove(attachment.id.as_str())
                        .and_then(Option::take)
                        .context("attachment_unknown: missing upload reference")?;
                }
            }
            message.attachments.extend(taken.into_iter().flatten());
            for (index, attachment) in message.attachments.iter_mut().enumerate() {
                attachment.id = format!("a{}", index + 1);
            }
            self.save_record(MESSAGES, &message.id, &message)?;
            self.save_record(
                KEYS,
                &key,
                &SendKey {
                    entry: message.id.clone(),
                    digest: digest.into(),
                },
            )?;
            self.save_record(HEADS, &head.session, &head)
        })?;
        Ok((message, false))
    }

    /// Read-only lookup. Cancellation of unseen keys belongs to the bounded command journal.
    pub fn app_send_entry(&self, device: &str, cmd_id: &str) -> Result<Option<String>> {
        ensure!(
            attachments::key(device) && attachments::key(cmd_id),
            "invalid send identity"
        );
        Ok(self
            .record::<SendKey>(KEYS, &format!("{device}/{cmd_id}"))?
            .map(|stored| stored.entry))
    }

    /// Withdraw only an existing, undelivered assistant message. Never writes tombstones.
    pub fn cancel_app_send(&self, device: &str, cmd_id: &str) -> Result<Option<(bool, Message)>> {
        let _lock = self.lock("app-threads")?;
        let Some(entry) = self.app_send_entry(device, cmd_id)? else {
            return Ok(None);
        };
        let message = self.app_message(&entry)?;
        ensure!(
            message.device == device && message.cmd_id == cmd_id,
            "send key identity mismatch"
        );
        if message.target != "assistant" {
            return Ok(Some((false, message)));
        }
        if message.state == "withdrawn" {
            return Ok(Some((true, message)));
        }
        if !matches!(
            message.state.as_str(),
            "sent" | "waiting" | "late" | "failed"
        ) {
            return Ok(Some((false, message)));
        }
        let message = self.update_app_message_locked(&entry, |message| {
            message.state = "withdrawn".into();
            Ok(())
        })?;
        Ok(Some((true, message)))
    }

    pub fn update_app_message(
        &self,
        id: &str,
        change: impl FnOnce(&mut Message) -> Result<()>,
    ) -> Result<Message> {
        let _lock = self.lock("app-threads")?;
        self.update_app_message_locked(id, change)
    }

    fn update_app_message_locked(
        &self,
        id: &str,
        change: impl FnOnce(&mut Message) -> Result<()>,
    ) -> Result<Message> {
        let mut message = self.app_message(id)?;
        let identity = (message.id.clone(), message.session.clone(), message.seq);
        let immutable_attachments = message.attachments.clone();
        change(&mut message)?;
        ensure!(
            identity == (message.id.clone(), message.session.clone(), message.seq),
            "message identity is immutable"
        );
        ensure!(
            message.attachments == immutable_attachments,
            "message attachments are immutable"
        );
        ensure!(
            message.text.len() <= TEXT_MAX
                && message.reply.as_ref().is_none_or(|s| s.len() <= REPLY_MAX),
            "message text exceeds bound"
        );
        ensure!(
            message.failure.as_ref().is_none_or(|s| s.len() <= 2048),
            "failure exceeds bound"
        );
        message.rev += 1;
        let mut head = self.app_thread_head(&message.session)?;
        head.rev += 1;
        if matches!(
            message.state.as_str(),
            "done" | "withdrawn" | "saved" | "pm_replied" | "uncertain"
        ) {
            head.pending.retain(|seq| *seq != message.seq);
        }
        self.records_transaction(|| {
            self.save_record(MESSAGES, id, &message)?;
            self.save_record(HEADS, &head.session, &head)
        })?;
        Ok(message)
    }

    pub fn set_app_thread_state(
        &self,
        session: &str,
        state: &str,
        failure: Option<String>,
    ) -> Result<()> {
        self.set_app_thread_state_unless(session, state, failure, &[])
    }

    /// Preserve a concurrent control state while publishing a runtime observation.
    pub fn set_app_thread_state_unless(
        &self,
        session: &str,
        state: &str,
        failure: Option<String>,
        preserved: &[&str],
    ) -> Result<()> {
        ensure!(
            failure.as_ref().is_none_or(|s| s.len() <= 2048),
            "failure exceeds bound"
        );
        let _lock = self.lock("app-threads")?;
        let mut head = self.app_thread_head(session)?;
        if preserved.contains(&head.state.as_str()) {
            return Ok(());
        }
        head.state = state.into();
        head.failure = failure;
        head.rev += 1;
        self.save_record(HEADS, session, &head)
    }

    /// Descending, point-keyed paging; history size does not affect one read.
    pub fn app_messages(
        &self,
        session: &str,
        before: Option<u64>,
        limit: usize,
    ) -> Result<Vec<Message>> {
        let head = self.app_thread_head(session)?;
        let end = before.unwrap_or(head.next).min(head.next);
        let start = end.saturating_sub(limit.min(100) as u64).max(1);
        (start..end)
            .rev()
            .map(|seq| self.app_message(&entry(session, seq)))
            .collect()
    }

    pub fn pending_app_messages(&self, session: &str) -> Result<Vec<Message>> {
        self.app_thread_head(session)?
            .pending
            .iter()
            .map(|seq| self.app_message(&entry(session, *seq)))
            .collect()
    }
}
