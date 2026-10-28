use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
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
        Ok(self
            .store
            .record("pm-inbox", "current")?
            .unwrap_or_default())
    }

    /// Caller must hold the inbox lock (and deliver lock for envelope changes).
    pub fn update_state<T>(&self, f: impl FnOnce(&mut InboxState) -> Result<T>) -> Result<T> {
        match &self.recipient {
            Recipient::Pm => self.store.records_transaction(|| {
                let mut pm = self.pm_state()?;
                let result = f(&mut pm.inbox)?;
                self.store.save_record("pm-inbox", "current", &pm)?;
                Ok(result)
            }),
            _ => {
                let id = self.recipient.task().context("task recipient")?;
                // A current container's inbox is a projection: its scalars are edited in place
                // and applied effects or cursors become keyed rows, so no history is read.
                let mut f = Some(f);
                let mut result = None;
                self.store.try_update_head(id, |head| {
                    let Some(inbox) = current_inbox(head, &self.recipient) else {
                        return Ok(false);
                    };
                    let mut state = InboxState {
                        next_seq: inbox.next_seq,
                        next_envelope: inbox.next_envelope,
                        open_envelope: inbox.open_envelope,
                        replacement_envelope: inbox.replacement_envelope,
                        effects_applied: Default::default(),
                        binding: inbox.binding.clone(),
                        observe_cursor: inbox.observe_cursor.clone(),
                        observe_cursors: Vec::new(),
                        settled: inbox.settled,
                    };
                    let update = f.take().context("inbox state update ran twice")?;
                    result = Some(update(&mut state)?);
                    *inbox = crate::InboxHead {
                        next_seq: state.next_seq,
                        next_envelope: state.next_envelope,
                        open_envelope: state.open_envelope,
                        replacement_envelope: state.replacement_envelope,
                        binding: state.binding,
                        observe_cursor: state.observe_cursor,
                        settled: state.settled,
                    };
                    for seq in state.effects_applied {
                        head.mark_effect_applied(&self.recipient, seq)?;
                    }
                    for cursor in state.observe_cursors {
                        head.set_observe_cursor(&self.recipient, cursor)?;
                    }
                    Ok(true)
                })?;
                if let Some(f) = f {
                    // An earlier container's inbox: the full form, as before.
                    self.store.update(id, |task| {
                        result = Some(f(state_mut(task, &self.recipient)?)?);
                        Ok(())
                    })?;
                }
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
        self.build_at(&mut state.next_seq, post)
    }

    fn build_at(&self, next_seq: &mut u64, post: Post<'_>) -> Result<Message> {
        let seq = *next_seq;
        *next_seq = seq.checked_add(1).context("inbox sequence exhausted")?;
        Ok(Message {
            id: format!("{}#{seq}", self.key),
            recipient: self.recipient.clone(),
            seq,
            kind: post.kind.into(),
            urgency: post.urgency,
            author: post.author.into(),
            created: self.store.now(),
            binding: post.binding,
            subject: post.subject,
            body: format!("[corral {}#{seq} {}]\n{}", self.key, post.kind, post.body),
        })
    }

    /// Reserve inside the caller's task unit, with the inbox lease taken first. The Inbox's
    /// own Store handle joins that unit, including an Authorization operation.
    pub fn reserve_in(&self, task: &mut Task, post: Post<'_>) -> Result<Message> {
        self.check(&post)?;
        ensure!(
            self.store.has_record_unit(),
            "inbox reservation requires an open task transaction"
        );
        let state = state_mut(task, &self.recipient)?;
        let mut next = state.clone();
        let message = self.build(&mut next, post)?;
        self.write_message(&message)?;
        *state = next;
        self.store.submission_effect_barrier("inbox_reserved");
        Ok(message)
    }

    /// [`Self::reserve_in`] on a head: the recipient's current container reserves the seq.
    ///
    /// # Errors
    /// Returns validation, transaction, inbox lookup or message write errors.
    pub fn reserve_in_head(
        &self,
        head: &mut crate::TaskHeadMut,
        post: Post<'_>,
    ) -> Result<Message> {
        use crate::TaskMut;
        self.check(&post)?;
        ensure!(
            self.store.has_record_unit(),
            "inbox reservation requires an open task transaction"
        );
        let message = head.inbox_state_mut(&self.recipient, |inbox| {
            let mut next_seq = inbox.next_seq;
            let message = self.build_at(&mut next_seq, post)?;
            self.write_message(&message)?;
            inbox.next_seq = next_seq;
            Ok(message)
        })?;
        self.store.submission_effect_barrier("inbox_reserved");
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
        self.check(&post)?;
        let _lock = self.lock()?;
        let message = match &self.recipient {
            Recipient::Pm => self.store.records_transaction(|| {
                let message = self.update_state(|state| self.build(state, post))?;
                self.write_message(&message)?;
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
                        *state_mut(task, &self.recipient)? = next;
                        self.write_message(&value)?;
                        message = Some(value);
                        Ok(())
                    })?;
                message.context("message reservation did not run")?
            }
        };
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
        let row: super::records::MessageRecord = self
            .record("messages", &format!("{seq:020}"))?
            .context("inbox message not found")?;
        let mut message = row.message;
        ensure!(
            message.seq == seq
                && message.recipient == self.recipient
                && message.id == format!("{}#{seq}", self.key),
            "message identity mismatch"
        );
        message.body = row.body;
        Ok(message)
    }

    /// Output-sized committed message history for this recipient.
    pub fn messages(&self) -> Result<Vec<Message>> {
        let next = self.state_head()?.next_seq;
        let mut messages = Vec::new();
        for row in self.records("messages")? {
            let record: super::records::MessageRecord =
                serde_json::from_value(row.doc).context("decoding inbox message")?;
            let mut message = record.message;
            message.body = record.body;
            ensure!(
                message.recipient == self.recipient
                    && message.id == format!("{}#{}", self.key, message.seq)
                    && row.key == format!("{:020}", message.seq),
                "message identity mismatch"
            );
            if message.seq < next {
                messages.push(message);
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
            effect: self.record("effects", &format!("{seq:020}"))?,
            envelope: self
                .record::<u64>("message-envelope", &format!("{seq:020}"))?
                .map(|number| self.envelope(number))
                .transpose()?,
        })
    }

    pub fn receipt(&self, seq: u64, level: ReceiptLevel) -> Result<Option<Receipt>> {
        self.record(
            &format!("receipt.{}", level.suffix()),
            &format!("{seq:020}"),
        )
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
            if self
                .record::<Value>(state.suffix(), &format!("{seq:020}"))?
                .is_some()
            {
                return Ok(Some(state));
            }
        }
        Ok(None)
    }

    pub fn record_receipt(&self, seq: u64, level: ReceiptLevel, receipt: &Receipt) -> Result<bool> {
        let _lock = self.lock()?;
        self.message(seq)?;
        self.store.records_transaction(|| {
            let kind = format!("receipt.{}", level.suffix());
            let key = format!("{seq:020}");
            let inserted = if self.record::<Receipt>(&kind, &key)?.is_some() {
                false
            } else {
                self.save_record(&kind, &key, receipt)?;
                true
            };
            if level == ReceiptLevel::Observed {
                for state in [
                    TerminalState::Cancelled,
                    TerminalState::Failed,
                    TerminalState::Abandoned,
                ] {
                    if self.record::<Value>(state.suffix(), &key)?.is_some() {
                        self.delete_record(state.suffix(), &key)?;
                        self.event(
                            "message_terminal_noop",
                            json!({"seq":seq,"state":state,"reason":"observed wins"}),
                        )?;
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
            self.refresh_message_index(seq)?;
            Ok(inserted)
        })
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
        self.store.records_transaction(|| {
            self.save_record(
                state.suffix(),
                &format!("{seq:020}"),
                &json!({"reason":reason,"via":via,"at":self.store.now()}),
            )?;
            self.event(
                &format!("message_{}", state.suffix()),
                json!({"seq":seq,"reason":reason,"via":via}),
            )?;
            self.refresh_message_index(seq)?;
            Ok(true)
        })
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

/// The inbox projection of `recipient` when it is one of the head's current containers.
fn current_inbox<'h>(
    head: &'h mut crate::TaskHeadMut,
    recipient: &Recipient,
) -> Option<&'h mut crate::InboxHead> {
    match recipient {
        Recipient::Run { run, .. } => head
            .current_run_mut()
            .filter(|r| &r.id == run)
            .map(|r| &mut r.inbox),
        Recipient::ReviewRun { run, review, .. } => head
            .current_run_mut()
            .filter(|r| &r.id == run)
            .and_then(|r| r.current_child.as_deref_mut())
            .filter(|c| &c.id == review)
            .map(|c| &mut c.inbox),
        Recipient::PlanRun { plan_run, .. } => head
            .last_plan_run_mut()
            .filter(|r| &r.id == plan_run)
            .map(|r| &mut r.inbox),
        Recipient::PlanReview { plan, review, .. } => head
            .last_plan_mut()
            .filter(|p| &p.id == plan)
            .and_then(|p| p.current_review.as_mut())
            .filter(|r| &r.id == review)
            .map(|r| &mut r.inbox),
        Recipient::Pm => None,
    }
}
