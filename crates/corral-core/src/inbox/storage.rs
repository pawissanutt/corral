use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use super::*;
use crate::{Store, StoreLock, Task};

/// All operations obey deliver -> inbox -> task lock order. Never call inside Store::update.
pub struct Inbox<'a> {
    pub store: &'a Store,
    pub recipient: Recipient,
    key: String,
}

impl<'a> Inbox<'a> {
    pub fn new(store: &'a Store, recipient: Recipient) -> Result<Self> {
        let key = recipient.key()?;
        Ok(Self {
            store,
            recipient,
            key,
        })
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn dir(&self) -> PathBuf {
        self.store.dir().join("inbox").join(&self.key)
    }

    pub fn lock(&self) -> Result<StoreLock> {
        self.store.lock(&format!("inbox-{}", self.key))
    }

    pub fn delivery_lock(&self) -> Result<StoreLock> {
        self.store.lock(&format!("deliver-{}", self.key))
    }

    pub fn state(&self) -> Result<InboxState> {
        match &self.recipient {
            Recipient::Pm => Ok(self.pm_state()?.inbox),
            _ => {
                let mut task = self
                    .store
                    .get(self.recipient.task().context("task recipient")?)?;
                Ok(state_mut(&mut task, &self.recipient)?.clone())
            }
        }
    }

    pub fn pm_state(&self) -> Result<PmInbox> {
        Ok(read_optional(&self.store.state_dir().join("pm-inbox.json"))?.unwrap_or_default())
    }

    /// Caller must hold the inbox lock (and deliver lock for envelope changes).
    pub fn update_state<T>(&self, f: impl FnOnce(&mut InboxState) -> Result<T>) -> Result<T> {
        match &self.recipient {
            Recipient::Pm => {
                let mut pm = self.pm_state()?;
                let result = f(&mut pm.inbox)?;
                fs::create_dir_all(self.store.state_dir())
                    .context("creating inbox state directory")?;
                write_json(&self.store.state_dir().join("pm-inbox.json"), &pm)?;
                Ok(result)
            }
            _ => {
                let mut result = None;
                self.store
                    .update(self.recipient.task().context("task recipient")?, |task| {
                        result = Some(f(state_mut(task, &self.recipient)?)?);
                        Ok(())
                    })?;
                result.context("inbox state update did not run")
            }
        }
    }

    pub fn post(&self, post: Post<'_>) -> Result<Message> {
        self.post_with(post, |_, _| Ok(()))
    }

    /// Kind and size validation, before any sequence or caller state is touched.
    pub fn check(&self, post: &Post<'_>) -> Result<()> {
        ensure!(
            !post.kind.is_empty()
                && post
                    .kind
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_'),
            "invalid message kind"
        );
        let cap = post.max_payload.min(MAX_PAYLOAD);
        // Reserve worst-case u64 header space, so every future envelope still fits.
        let overhead = format!(
            "[corral envelope {}@env{}]\n\n[corral {}#{} {}]\n",
            self.key,
            u64::MAX,
            self.key,
            u64::MAX,
            post.kind
        )
        .len();
        ensure!(
            post.body
                .len()
                .checked_add(overhead)
                .is_some_and(|n| n <= cap),
            "message size {} bytes plus {} header bytes exceeds payload cap {} bytes",
            post.body.len(),
            overhead,
            cap
        );
        Ok(())
    }

    fn build(&self, state: &mut InboxState, post: Post<'_>) -> Result<Message> {
        self.admit_owner(state, post.kind)?;
        let seq = state.next_seq;
        state.next_seq = seq.checked_add(1).context("inbox sequence exhausted")?;
        let message = Message {
            id: format!("{}#{seq}", self.key),
            recipient: self.recipient.clone(),
            seq,
            kind: post.kind.into(),
            urgency: post.urgency,
            author: post.author.into(),
            created: Timestamp::now(),
            binding: post.binding,
            subject: post.subject,
            body: format!("[corral {}#{seq} {}]\n{}", self.key, post.kind, post.body),
        };
        Self::retain_owner(state, &message);
        Ok(message)
    }

    fn write_message(&self, message: &Message) -> Result<()> {
        fs::create_dir_all(self.dir()).context("creating inbox directory")?;
        let yaml = serde_yaml_ng::to_string(message).context("serializing inbox message")?;
        crate::store::atomic_write(
            &self.message_path(message.seq),
            &format!("---\n{yaml}---\n{}", message.body),
        )
    }

    /// Reserve and publish inside a transaction the caller already holds on `task`, with the
    /// inbox lock taken before it. The file is written before the caller saves the task: an
    /// unsaved reservation leaves a file at `next_seq`, which `messages` ignores and the next
    /// reservation overwrites, so admission and publication commit together.
    pub fn reserve_in(&self, task: &mut Task, post: Post<'_>) -> Result<Message> {
        self.check(&post)?;
        let state = state_mut(task, &self.recipient)?;
        let mut next = state.clone();
        let message = self.build(&mut next, post)?;
        self.write_message(&message)?;
        #[cfg(test)]
        super::header::checkpoint("reserve_file_written")?;
        *state = next;
        Ok(message)
    }

    pub fn announce(&self, message: &Message) -> Result<()> {
        self.event("message_posted", json!({"id":message.id,"seq":message.seq}))
    }

    /// Reserve identity, message binding and caller admission in one task transaction.
    /// The PM has no task admission callback.
    pub fn post_with(
        &self,
        post: Post<'_>,
        admit: impl FnOnce(&mut Task, &Message) -> Result<()>,
    ) -> Result<Message> {
        self.post_with_reservation(post, admit, |_| Ok(()))
    }

    /// `reserving` runs after definitive validation/admission but before the sequence can
    /// commit. Once it succeeds, a later error does not prove publication was rolled back.
    /// It sees the exact replacement identity while the posting lock is held.
    pub(super) fn post_with_reservation(
        &self,
        post: Post<'_>,
        admit: impl FnOnce(&mut Task, &Message) -> Result<()>,
        reserving: impl FnOnce(&Message) -> Result<()>,
    ) -> Result<Message> {
        self.check(&post)?;
        let _lock = self.lock()?;
        let message = match &self.recipient {
            Recipient::Pm => self.update_state(|state| {
                let message = self.build(state, post)?;
                reserving(&message)?;
                Ok(message)
            })?,
            _ => {
                let mut message = None;
                self.store
                    .update(self.recipient.task().context("task recipient")?, |task| {
                        // Admission sees the task as the caller snapshotted it; the
                        // reserved seq is committed in the same transaction afterwards.
                        let mut next = state_mut(task, &self.recipient)?.clone();
                        let value = self.build(&mut next, post)?;
                        admit(task, &value)?;
                        reserving(&value)?;
                        *state_mut(task, &self.recipient)? = next;
                        message = Some(value);
                        Ok(())
                    })?;
                message.context("message reservation did not run")?
            }
        };
        #[cfg(test)]
        super::header::checkpoint("post_reserved")?;
        if let Err(error) = self.write_message(&message) {
            self.event(
                "message_gap",
                json!({"seq": message.seq, "error": error.to_string()}),
            )?;
            return Err(error);
        }
        self.announce(&message)?;
        Ok(message)
    }

    pub fn message_path(&self, seq: u64) -> PathBuf {
        self.dir().join(format!("{seq:06}.md"))
    }

    pub fn record_path(&self, seq: u64, suffix: &str) -> PathBuf {
        self.dir().join(format!("{seq:06}.{suffix}.json"))
    }

    pub fn message(&self, seq: u64) -> Result<Message> {
        crate::work::record(crate::work::Work::InboxMessageRead, 1);
        let text = fs::read_to_string(self.message_path(seq)).context("reading inbox message")?;
        let rest = text
            .strip_prefix("---\n")
            .context("missing message frontmatter")?;
        let (yaml, body) = rest
            .split_once("\n---\n")
            .context("unterminated message frontmatter")?;
        let mut message: Message =
            serde_yaml_ng::from_str(yaml).context("parsing inbox message")?;
        ensure!(
            message.seq == seq
                && message.recipient == self.recipient
                && message.id == format!("{}#{seq}", self.key),
            "message identity mismatch"
        );
        message.body = body.into();
        Ok(message)
    }

    /// Committed messages only: a file at or beyond `next_seq` is an unsaved reservation.
    pub fn messages(&self) -> Result<Vec<Message>> {
        let next = self.state()?.next_seq;
        let mut messages = Vec::new();
        for path in paths(&self.dir())? {
            if path.extension().is_some_and(|s| s == "md")
                && let Some(seq) = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.parse::<u64>().ok())
                && seq < next
            {
                messages.push(self.message(seq)?);
            }
        }
        messages.sort_by_key(|m| m.seq);
        Ok(messages)
    }

    pub fn entry(&self, seq: u64) -> Result<InboxEntry> {
        let _lock = self.lock()?;
        Ok(InboxEntry {
            message: self.message(seq)?,
            admitted: self.receipt(seq, ReceiptLevel::Admitted)?,
            observed: self.receipt(seq, ReceiptLevel::Observed)?,
            terminal: self.terminal(seq)?,
            effect: read_optional(&self.record_path(seq, "effect"))?,
            envelope: self
                .envelopes()?
                .into_iter()
                .rev()
                .find(|e| e.members.contains(&seq)),
        })
    }

    pub fn receipt(&self, seq: u64, level: ReceiptLevel) -> Result<Option<Receipt>> {
        crate::work::record(crate::work::Work::InboxRecordRead, 1);
        read_optional(&self.record_path(seq, &format!("receipt.{}", level.suffix())))
    }

    pub fn terminal(&self, seq: u64) -> Result<Option<TerminalState>> {
        if self.receipt(seq, ReceiptLevel::Observed)?.is_some() {
            return Ok(None);
        }
        for state in [
            TerminalState::Cancelled,
            TerminalState::Failed,
            TerminalState::Abandoned,
        ] {
            if self.record_path(seq, state.suffix()).exists() {
                return Ok(Some(state));
            }
        }
        Ok(None)
    }

    pub fn record_receipt(&self, seq: u64, level: ReceiptLevel, receipt: &Receipt) -> Result<bool> {
        let _lock = self.lock()?;
        self.message(seq)?;
        self.project_members(&[seq], |entry| {
            entry.retry_evidence.bar(
                match level {
                    ReceiptLevel::Admitted => WitnessReason::Admitted,
                    ReceiptLevel::Observed => WitnessReason::Observed,
                },
                None,
            );
        })?;
        #[cfg(test)]
        super::header::checkpoint("receipt_barred")?;
        let inserted = exclusive_json(
            &self.record_path(seq, &format!("receipt.{}", level.suffix())),
            receipt,
        )?;
        #[cfg(test)]
        super::header::checkpoint("receipt_written")?;
        if level == ReceiptLevel::Observed {
            for state in [
                TerminalState::Cancelled,
                TerminalState::Failed,
                TerminalState::Abandoned,
            ] {
                let path = self.record_path(seq, state.suffix());
                match fs::remove_file(&path) {
                    Ok(()) => self.event(
                        "message_terminal_noop",
                        json!({"seq":seq,"state":state,"reason":"observed wins"}),
                    )?,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e).context("removing superseded terminal record"),
                }
            }
        }
        if inserted {
            self.event(
                match level {
                    ReceiptLevel::Admitted => "message_admitted",
                    ReceiptLevel::Observed => "message_observed",
                },
                json!({"id":format!("{}#{seq}",self.key),"receipt":receipt}),
            )?;
        }
        Ok(inserted)
    }

    pub fn mark_terminal(
        &self,
        seq: u64,
        state: TerminalState,
        reason: &str,
        via: &str,
    ) -> Result<bool> {
        let _lock = self.lock()?;
        self.message(seq)?;
        if self.receipt(seq, ReceiptLevel::Observed)?.is_some() || self.terminal(seq)?.is_some() {
            self.event(
                "message_terminal_noop",
                json!({"seq":seq,"state":state,"via":via}),
            )?;
            return Ok(false);
        }
        let inserted = exclusive_json(
            &self.record_path(seq, state.suffix()),
            &json!({"reason":reason,"via":via,"at":Timestamp::now()}),
        )?;
        if inserted {
            self.event(
                &format!("message_{}", state.suffix()),
                json!({"seq":seq,"reason":reason,"via":via}),
            )?;
        }
        Ok(inserted)
    }

    pub fn event(&self, kind: &str, mut detail: Value) -> Result<()> {
        detail["key"] = json!(self.key);
        self.store.log_event(self.recipient.task(), kind, detail)
    }
}

pub(super) fn state_mut<'a>(
    task: &'a mut Task,
    recipient: &Recipient,
) -> Result<&'a mut InboxState> {
    match recipient {
        Recipient::Run { run, .. } => Ok(&mut task
            .runs
            .iter_mut()
            .find(|r| &r.id == run)
            .context("inbox run not found")?
            .inbox),
        Recipient::PlanRun { plan_run, .. } => Ok(&mut task
            .plan_runs
            .iter_mut()
            .find(|r| &r.id == plan_run)
            .context("inbox plan run not found")?
            .inbox),
        Recipient::PlanReview { plan, review, .. } => Ok(&mut task
            .plans
            .iter_mut()
            .find(|p| &p.id == plan)
            .context("inbox plan not found")?
            .reviews
            .iter_mut()
            .find(|r| &r.id == review)
            .context("inbox plan review not found")?
            .inbox),
        Recipient::ReviewRun { run, review, .. } => Ok(&mut task
            .runs
            .iter_mut()
            .find(|r| &r.id == run)
            .context("inbox run not found")?
            .review_runs
            .iter_mut()
            .find(|r| &r.id == review)
            .context("inbox review child not found")?
            .inbox),
        Recipient::Pm => anyhow::bail!("PM has no task state"),
    }
}

pub(super) fn paths(dir: &Path) -> Result<Vec<PathBuf>> {
    crate::work::record(crate::work::Work::InboxList, 1);
    match fs::read_dir(dir) {
        Ok(entries) => entries
            .map(|e| e.map(|e| e.path()).context("reading inbox directory entry"))
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", dir.display())),
    }
}

pub(super) fn read_optional<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", path.display()))?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

pub(super) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    crate::store::atomic_write(
        path,
        &serde_json::to_string_pretty(value).context("serializing inbox record")?,
    )
}

fn exclusive_json(path: &Path, value: &impl Serialize) -> Result<bool> {
    let bytes = serde_json::to_vec_pretty(value).context("serializing exclusive inbox record")?;
    let mut file = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(e).with_context(|| format!("creating {}", path.display())),
    };
    file.write_all(&bytes)
        .with_context(|| format!("writing {}", path.display()))?;
    file.sync_all().context("syncing exclusive inbox record")?;
    Ok(true)
}
