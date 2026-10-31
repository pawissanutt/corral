//! Typed recipient areas and the output-sized unfinished-message indexes.

use std::collections::BTreeSet;

use anyhow::{Context, Result};
use serde::{Serialize, de::DeserializeOwned};

use super::*;
use crate::InboxHead;
use crate::backend::{RowQuery, StoredRecord, TaskPart};
use crate::work::Counter;

#[derive(Serialize, Deserialize)]
pub(super) struct MessageRecord {
    pub message: Message,
    pub body: String,
}

impl Inbox<'_> {
    /// Inbox scalars without decoding applied effects or scoped cursor history.
    pub fn state_head(&self) -> Result<InboxHead> {
        if let Some((area, key)) = self.standalone_state_key() {
            #[derive(Default, Deserialize)]
            struct PmHead {
                #[serde(default)]
                inbox: InboxHead,
            }
            return Ok(self
                .standalone_record::<PmHead>(area, key)?
                .unwrap_or_default()
                .inbox);
        }
        let head = self
            .store
            .head(self.recipient.task().context("task recipient")?)?;
        let (part, id) = match &self.recipient {
            Recipient::Run { run, .. } => {
                if let Some(current) = head.current_run.as_ref().filter(|r| r.id == *run) {
                    return Ok(current.inbox.clone());
                }
                (TaskPart::Run, run)
            }
            Recipient::PlanRun { plan_run, .. } => {
                if let Some(current) = head.last_plan_run.as_ref().filter(|r| r.id == *plan_run) {
                    return Ok(current.inbox.clone());
                }
                (TaskPart::PlanRun, plan_run)
            }
            Recipient::ReviewRun { run, review, .. } => {
                return Ok(head
                    .review_child(run, review)?
                    .context("inbox review child not found")?
                    .inbox);
            }
            Recipient::PlanReview { plan, review, .. } => {
                return Ok(head
                    .plan_review(plan, review)?
                    .context("inbox plan review not found")?
                    .inbox);
            }
            Recipient::Pm | Recipient::Assistant { .. } => {
                anyhow::bail!("standalone inbox has no task recipient")
            }
        };
        let row = head
            .query(
                &RowQuery::part(part).with_index(format!("id:{id}")),
                1,
                Counter::HistoryRowDecodes,
            )?
            .pop()
            .context("inbox run not found")?;
        serde_json::from_value(row.doc.get("inbox").cloned().unwrap_or_default())
            .context("decoding inbox head")
    }

    /// The scoped observe cursor for one adapter and binding, read by key.
    pub fn observe_cursor_of(
        &self,
        adapter: &str,
        binding: &SessionBinding,
    ) -> Result<Option<ObserveCursor>> {
        match self.recipient.task() {
            Some(task) => Ok(self
                .store
                .head(task)?
                .observe_cursor(&self.recipient, adapter, binding)?
                .map(|stream| stream.cursor)),
            None => Ok(self
                .standalone_state()?
                .inbox
                .observe_cursors
                .into_iter()
                .find(|stream| stream.adapter == adapter && &stream.binding == binding)
                .map(|stream| stream.cursor)),
        }
    }

    pub fn effect_applied(&self, seq: u64) -> Result<bool> {
        match self.recipient.task() {
            Some(task) => self.store.head(task)?.effect_applied(&self.recipient, seq),
            None => Ok(self
                .standalone_state()?
                .inbox
                .effects_applied
                .contains(&seq)),
        }
    }

    pub(super) fn standalone_record<T: DeserializeOwned>(
        &self,
        area: &str,
        key: &str,
    ) -> Result<Option<T>> {
        let _cost = crate::work::followup("T-669", "standalone_inbox_load");
        self.store.record(area, key)
    }

    pub(super) fn save_standalone_record(
        &self,
        area: &str,
        key: &str,
        value: &impl Serialize,
    ) -> Result<()> {
        let _cost = crate::work::followup("T-669", "standalone_inbox_save");
        self.store.save_record(area, key, value)
    }

    pub(super) fn standalone_state(&self) -> Result<PmInbox> {
        let (area, key) = self
            .standalone_state_key()
            .context("standalone inbox required")?;
        Ok(self.standalone_record(area, key)?.unwrap_or_default())
    }

    pub fn record<T: DeserializeOwned>(&self, kind: &str, key: &str) -> Result<Option<T>> {
        self.store.record(&self.area(kind), key)
    }

    pub fn save_record(&self, kind: &str, key: &str, value: &impl Serialize) -> Result<()> {
        self.store.save_record(&self.area(kind), key, value)
    }

    /// Retry identity must survive a subsequent standalone reservation failure.
    pub(super) fn save_record_intent(
        &self,
        kind: &str,
        key: &str,
        value: &impl Serialize,
    ) -> Result<()> {
        self.store
            .commit_record_intent(&self.area(kind), key, value)
    }

    pub fn delete_record(&self, kind: &str, key: &str) -> Result<()> {
        self.store.delete_record(&self.area(kind), key)
    }

    pub fn records(&self, kind: &str) -> Result<Vec<StoredRecord>> {
        self.store.record_rows(&self.area(kind))
    }

    fn area(&self, kind: &str) -> String {
        format!("inbox/{}/{kind}", self.key())
    }

    pub(super) fn write_message(&self, message: &Message) -> Result<()> {
        self.save_record(
            "messages",
            &format!("{:020}", message.seq),
            &MessageRecord {
                message: message.clone(),
                body: message.body.clone(),
            },
        )?;
        self.save_record("pending", &format!("{:020}", message.seq), &true)?;
        self.store
            .save_record("inbox-recipients", self.key(), &self.recipient)
    }

    /// Only messages eligible for ordinary delivery. Finished history is not decoded.
    pub fn pending_messages(&self) -> Result<Vec<Message>> {
        self.indexed_messages(&["pending"])
    }

    /// Exactly the union needed by delivery settlement: pending delivery, unfinished observed
    /// effects, and late evidence for an attempted envelope (even after cancellation).
    pub fn unfinished_messages(&self) -> Result<Vec<Message>> {
        self.indexed_messages(&["pending", "effects-pending", "late-evidence"])
    }

    fn indexed_messages(&self, indexes: &[&str]) -> Result<Vec<Message>> {
        let mut sequences = BTreeSet::new();
        for index in indexes {
            for row in self.records(index)? {
                sequences.insert(row.key.parse::<u64>().context("invalid inbox index key")?);
            }
        }
        sequences.into_iter().map(|seq| self.message(seq)).collect()
    }

    /// The caller holds the inbox lease and an open records or task unit, so these derived
    /// indexes commit with the receipt/effect/entry that changes them.
    pub fn refresh_message_index(&self, seq: u64) -> Result<()> {
        let key = format!("{seq:020}");
        let observed = self.receipt(seq, ReceiptLevel::Observed)?.is_some();
        if observed || self.terminal(seq)?.is_some() {
            self.delete_record("pending", &key)?;
        } else {
            self.save_record("pending", &key, &true)?;
        }
        if observed {
            self.delete_record("late-evidence", &key)?;
            let owners = format!("observation-owners/{seq:020}");
            for row in self.records(&owners)? {
                let envelope =
                    self.envelope(row.key.parse().context("invalid observation owner")?)?;
                if self.has_unobserved_members(&envelope)? {
                    self.delete_record(&owners, &row.key)?;
                } else {
                    self.clear_observation_index(&envelope)?;
                }
            }
        }
        let needs_effect = observed
            && self
                .record::<Effect>("effects", &key)?
                .is_none_or(|effect| effect.status == EffectStatus::Started);
        if needs_effect {
            self.save_record("effects-pending", &key, &true)?;
        } else {
            self.delete_record("effects-pending", &key)?;
        }
        Ok(())
    }
}
